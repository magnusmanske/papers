use std::{collections::HashMap, sync::Arc};

use anyhow::Result;
use futures::prelude::*;
use regex::Regex;
use tokio::sync::RwLock;
use wikibase::mediawiki::api::Api;

use crate::{
    generic_author_info::GenericAuthorInfo, wikidata_interaction::WikidataInteraction,
    wikidata_papers::WikidataPapers, wikidata_string_cache::WikidataStringCache,
};

lazy_static! {
    static ref RE_WD: Regex =
        Regex::new(r#"^(Q\d+)$"#).expect("SourceMDbot::process_author: RE_WD does not compile");
}

const MIN_PAPERS_PER_AUTHOR: usize = 2;
const MIN_SPACES_FOR_NAME: usize = 1;
const MAX_PROCESS_PAPERS_CONCURRENCY: usize = 5;

#[derive(Default)]
pub struct AuthorNameString {
    pub logging_level: u8,
}

impl AuthorNameString {
    fn log<S: Into<String>>(&self, level: u8, msg: S) {
        if level <= self.logging_level {
            println!("{}", msg.into());
        }
    }

    /// Splits a name into atomic parts, handling dots as separators.
    /// E.g., "C.K. Clarke" → ["C", "K", "Clarke"], "Ck Clarke" → ["Ck",
    /// "Clarke"]
    fn split_name_parts(name: &str) -> Vec<String> {
        let mut parts = Vec::new();
        for word in name.split_whitespace() {
            for sub in word.split('.') {
                let trimmed = sub.trim();
                if !trimmed.is_empty() {
                    parts.push(trimmed.to_string());
                }
            }
        }
        parts
    }

    /// Returns true if the name contains any word-parts shorter than 3
    /// characters (i.e., initials or two-letter abbreviations that
    /// `simplify_name` would drop).
    pub fn has_short_name_parts(name: &str) -> bool {
        Self::split_name_parts(name).iter().any(|p| p.len() < 3)
    }

    /// Converts a name like "Ck Clarke" or "C K Clarke" or "C.K. Clarke"
    /// into the format expected by the initial_search API: "C.K.Clarke"
    pub fn format_name_for_initial_search(name: &str) -> String {
        let mut result = Vec::new();
        for part in Self::split_name_parts(name) {
            if part.len() < 3 {
                // Treat as initials: each character becomes a separate initial
                for c in part.chars() {
                    result.push(format!("{}.", c.to_uppercase()));
                }
            } else {
                result.push(part.to_string());
            }
        }
        result.join("")
    }

    /// Calls the wd-infernal initial_search API to find Wikidata items
    /// matching a name with initials. Returns Q-IDs on success, None on error.
    async fn search_by_initials(name: &str) -> Option<Vec<String>> {
        let formatted = Self::format_name_for_initial_search(name);
        if formatted.is_empty() {
            return None;
        }
        let url = format!("https://wd-infernal.toolforge.org/initial_search/{}", formatted);
        let json = crate::http_client::fetch_json(&url).await?;
        serde_json::from_value(json).ok()
    }

    async fn process_papers_for_ans(
        &self,
        ans: &String,
        cache: &Arc<WikidataStringCache>,
        mw_api: &Arc<RwLock<Api>>,
        paper_qs: &Vec<String>,
        name2author_qs: &HashMap<String, Vec<String>>,
    ) -> Result<()> {
        let author_q = match self.get_or_create_author(ans, cache, mw_api, name2author_qs).await {
            Some(q) => q,
            None => return Ok(()),
        };

        let mut author = GenericAuthorInfo::new();
        author.set_name(Some(ans.clone()));
        author.set_wikidata_item(Some(author_q.clone()));
        let mut papers = WikidataPapers::new(cache.clone());
        let api = mw_api.read().await;
        papers.entities_mut().load_entities(&api, paper_qs).await?;
        drop(api);

        let edited_qs = self
            .create_p50_statements(ans, mw_api, paper_qs, author_q, author, &mut papers)
            .await;
        if !edited_qs.is_empty() {
            let api = mw_api.read().await;
            papers.entities_mut().reload_entities(&api, &edited_qs).await?;
            drop(api);
        }
        Ok(())
    }

    async fn create_p50_statements(
        &self,
        ans: &String,
        mw_api: &Arc<RwLock<Api>>,
        paper_qs: &Vec<String>,
        author_q: String,
        author: GenericAuthorInfo,
        papers: &mut WikidataPapers,
    ) -> Vec<String> {
        // Create P50 statements
        let mut edited_qs = vec![];
        for paper_q in paper_qs {
            let mut item = match papers.entities_mut().get_entity(paper_q) {
                Some(item) => item,
                None => continue,
            };
            let original_item = item.clone();
            papers.update_author_name_statement(ans, &author, &mut item);
            self.log(3, format!("EDITING PAPER {paper_q}: {ans} => {author_q}"));
            papers.set_edit_summary(Some(format!(
                "Changing {ans} to {} [#Papers ANS (was: SourceMD)]",
                "[[".to_string() + &author_q + "]]"
            )));
            match papers.apply_diff_for_item(original_item, item, mw_api.clone()).await {
                Some(er) => {
                    if er.edited() {
                        self.log(
                            1,
                            format!("Created or updated https://www.wikidata.org/wiki/{}", er.q()),
                        );
                        edited_qs.push(er.q().to_string());
                    } else {
                        self.log(
                            3,
                            format!("https://www.wikidata.org/wiki/{}, no changes ", er.q()),
                        );
                    }
                },
                None => self.log(1, "No paper item ID!"),
            }
            // papers.set_edit_summary(None);
            // save_item_changes(&mut papers, mw_api.clone(), paper_q).await;
        }
        edited_qs
    }

    async fn get_or_create_author(
        &self,
        ans: &String,
        cache: &Arc<WikidataStringCache>,
        mw_api: &Arc<RwLock<Api>>,
        name2author_qs: &HashMap<String, Vec<String>>,
    ) -> Option<String> {
        // If the name has initials, try the specialized initial_search API first
        if Self::has_short_name_parts(ans) {
            if let Some(initial_results) = Self::search_by_initials(ans).await {
                if initial_results.len() == 1 {
                    let candidate = &initial_results[0];
                    // Cross-check: the candidate must be a known coauthor
                    for qs in name2author_qs.values() {
                        if qs.contains(candidate) {
                            self.log(
                                1,
                                format!("MATCHED AUTHOR VIA INITIALS {ans}: => {candidate}"),
                            );
                            return Some(candidate.clone());
                        }
                    }
                }
                // Multiple results or no coauthor match: fall through to
                // existing logic
            }
        }

        let simple_name = GenericAuthorInfo::simplify_name(ans);
        let res = cache
            .search_wikibase(&format!("{simple_name} haswbstatement:P31=Q5"), mw_api.clone())
            .await
            .ok()?;
        let author_q = if res.is_empty() {
            self.create_new_author(ans, mw_api, cache).await
        } else if res.len() == 1 {
            if let Some(author_qs) = name2author_qs.get(ans) {
                if author_qs.len() == 1 {
                    let author_q = &author_qs[0];
                    self.log(1, format!("MATCHED AUTHOR {ans}: {simple_name} => {author_q}"));
                    Some(author_q.to_owned())
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            self.log(2, format!("MULTIPLE POSSIBLE MATCHES FOR {ans}: {simple_name} => {res:?}"));
            None
        };
        author_q
    }

    pub async fn process_author_q(
        &self,
        root_author_q: String,
        mw_api: &Arc<RwLock<Api>>,
        cache: &Arc<WikidataStringCache>,
    ) -> Result<()> {
        self.log(1, format!("Processing {}", root_author_q));
        let mut author = GenericAuthorInfo::new();
        author.set_wikidata_item(Some(root_author_q.to_owned()));
        let api = mw_api.read().await;
        let ans2paper_qs = self.get_coauthor_ans(&root_author_q, &api).await?;
        let name2author_qs = self.get_coauthor_qs(&root_author_q, &api).await?;
        drop(api);

        let mut futures = vec![];
        for (ans, paper_qs) in ans2paper_qs.iter() {
            let future = self.process_papers_for_ans(ans, cache, mw_api, paper_qs, &name2author_qs);
            futures.push(future);
        }

        let _ = futures::stream::iter(futures)
            .buffer_unordered(MAX_PROCESS_PAPERS_CONCURRENCY)
            .collect::<Vec<_>>()
            .await;
        Ok(())
    }

    /// Get coauthors of a given author, author name string to paper Qids
    async fn get_coauthor_ans(
        &self,
        root_author_q: &str,
        api: &Api,
    ) -> Result<HashMap<String, Vec<String>>> {
        // (paper Qid, author name string)
        let result_ans: Vec<(String, String)> = api
            .sparql_query(&format!(
                "SELECT ?paper ?ans {{ ?paper wdt:P50 wd:{root_author_q} ; wdt:P2093 ?ans }}"
            ))
            .await?["results"]["bindings"]
            .as_array()
            .ok_or(anyhow::anyhow!("get_coauthor_ans: Not an array"))?
            .iter()
            .filter_map(|j| {
                let ans = j["ans"]["value"].as_str()?;
                let q = api.extract_entity_from_uri(j["paper"]["value"].as_str()?).ok()?;
                Some((q.to_string(), ans.to_string()))
            })
            .collect();

        let mut ans2paper_qs: HashMap<String, Vec<String>> = HashMap::new();
        for (paper_q, ans) in result_ans {
            if RE_WD.is_match(&paper_q) {
                let paper_qs = ans2paper_qs.entry(ans).or_default();
                paper_qs.push(paper_q);
            }
        }
        ans2paper_qs.retain(|_, v| v.len() >= MIN_PAPERS_PER_AUTHOR);
        ans2paper_qs
            .retain(|ans, _| ans.chars().filter(|c| *c == ' ').count() >= MIN_SPACES_FOR_NAME);
        Ok(ans2paper_qs)
    }

    async fn get_coauthor_qs(
        &self,
        root_author_q: &str,
        api: &Api,
    ) -> Result<HashMap<String, Vec<String>>> {
        // (Qid, name)
        let result_coauthors: Vec<(String, String)> = api
        .sparql_query(&format!("select DISTINCT ?coauthor ?coauthorLabel {{ ?paper wdt:P50 wd:{root_author_q} ; wdt:P50 ?coauthor . SERVICE wikibase:label {{ bd:serviceParam wikibase:language \"[AUTO_LANGUAGE],en\" }} }}"))
        .await?["results"]["bindings"]
        .as_array()
        .ok_or(anyhow::anyhow!("get_coauthor_qs: Not an array"))?
        .iter()
        .filter_map(|j| {
            let name = j["coauthorLabel"]["value"].as_str()?;
            let q = api
                .extract_entity_from_uri(j["coauthor"]["value"].as_str()?)
                .ok()?;
            Some((q.to_string(), name.to_string()))
        })
        .collect();

        let mut name2qs: HashMap<String, Vec<String>> = HashMap::new();
        for (author_q, name) in result_coauthors {
            if RE_WD.is_match(&author_q) {
                let author_qs = name2qs.entry(name).or_default();
                author_qs.push(author_q);
            }
        }
        Ok(name2qs)
    }

    async fn create_new_author(
        &self,
        ans: &String,
        mw_api: &Arc<RwLock<Api>>,
        cache: &Arc<WikidataStringCache>,
    ) -> Option<String> {
        self.log(1, format!("CREATING AUTHOR {ans}"));
        let mut author = GenericAuthorInfo::new();
        author.set_name(Some(ans.clone()));
        let author = author.get_or_create_author_item(mw_api.clone(), cache.clone(), true).await;
        self.log(1, format!("CREATED AUTHOR {ans} => {author:?}"));
        Some(author.wikidata_item()?.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn has_short_name_parts_detects_initials() {
        assert!(AuthorNameString::has_short_name_parts("Ck Clarke"));
        assert!(AuthorNameString::has_short_name_parts("C K Clarke"));
        assert!(AuthorNameString::has_short_name_parts("C.K. Clarke"));
        assert!(AuthorNameString::has_short_name_parts("J Smith"));
        assert!(AuthorNameString::has_short_name_parts("H.M. Manske"));
    }

    #[test]
    fn has_short_name_parts_false_for_full_names() {
        assert!(!AuthorNameString::has_short_name_parts("Jenny Clarke"));
        assert!(!AuthorNameString::has_short_name_parts("John Smith"));
        assert!(!AuthorNameString::has_short_name_parts("Heinrich Magnus Manske"));
    }

    #[test]
    fn format_name_for_initial_search_basic() {
        assert_eq!(AuthorNameString::format_name_for_initial_search("Ck Clarke"), "C.K.Clarke");
        assert_eq!(AuthorNameString::format_name_for_initial_search("C K Clarke"), "C.K.Clarke");
    }

    #[test]
    fn format_name_for_initial_search_dotted() {
        assert_eq!(AuthorNameString::format_name_for_initial_search("C.K. Clarke"), "C.K.Clarke");
        assert_eq!(AuthorNameString::format_name_for_initial_search("H.M. Manske"), "H.M.Manske");
    }

    #[test]
    fn format_name_for_initial_search_single_initial() {
        assert_eq!(AuthorNameString::format_name_for_initial_search("J Smith"), "J.Smith");
    }

    #[test]
    fn format_name_for_initial_search_full_name() {
        // Full names without initials: no dots added
        assert_eq!(AuthorNameString::format_name_for_initial_search("Jenny Clarke"), "JennyClarke");
    }
}
