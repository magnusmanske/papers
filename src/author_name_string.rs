use std::{collections::HashMap, sync::Arc};

use anyhow::Result;
use futures::prelude::*;
use tokio::sync::RwLock;
use wikibase::mediawiki::api::Api;

use crate::{
    generic_author_info::GenericAuthorInfo,
    http_client::{HttpJsonFetcher, JsonFetcher},
    wikidata_interaction::WikidataInteraction,
    wikidata_papers::WikidataPapers,
    wikidata_string_cache::WikidataStringCache,
};

const MIN_PAPERS_PER_AUTHOR: usize = 2;
const MIN_SPACES_FOR_NAME: usize = 1;
const MAX_PROCESS_PAPERS_CONCURRENCY: usize = 5;

/// Extracts a Wikidata Q-id from a URI like `"http://www.wikidata.org/entity/Q12345"`.
///
/// `mediawiki::Api::extract_entity_from_uri` does the same; we duplicate the
/// trivial logic here so the SPARQL-response parsers below don't have to
/// borrow an `Api`. Returns `None` if the suffix isn't a valid Q-id.
fn q_from_entity_uri(uri: &str) -> Option<String> {
    let last = uri.rsplit('/').next()?;
    if crate::identifiers::is_qid(last) {
        Some(last.to_string())
    } else {
        None
    }
}

/// Parses the `?paper ?ans` SPARQL bindings into a map from author-name
/// string to paper Q-ids, applying the configured filters
/// ([`MIN_PAPERS_PER_AUTHOR`], [`MIN_SPACES_FOR_NAME`]). Pure function so
/// the filtering logic is unit-testable without a live SPARQL endpoint.
fn parse_coauthor_ans_bindings(
    bindings: &[serde_json::Value],
) -> HashMap<String, Vec<String>> {
    let pairs: Vec<(String, String)> = bindings
        .iter()
        .filter_map(|j| {
            let ans = j["ans"]["value"].as_str()?;
            let q = q_from_entity_uri(j["paper"]["value"].as_str()?)?;
            Some((q, ans.to_string()))
        })
        .collect();

    let mut ans2paper_qs: HashMap<String, Vec<String>> = HashMap::new();
    for (paper_q, ans) in pairs {
        // Already filtered to QIDs by q_from_entity_uri, but defensive:
        if crate::identifiers::is_qid(&paper_q) {
            ans2paper_qs.entry(ans).or_default().push(paper_q);
        }
    }
    ans2paper_qs.retain(|_, v| v.len() >= MIN_PAPERS_PER_AUTHOR);
    ans2paper_qs.retain(|ans, _| ans.chars().filter(|c| *c == ' ').count() >= MIN_SPACES_FOR_NAME);
    ans2paper_qs
}

/// Parses the `?coauthor ?coauthorLabel` SPARQL bindings into a map from
/// label-string to coauthor Q-ids. Pure function for unit testability.
fn parse_coauthor_qs_bindings(
    bindings: &[serde_json::Value],
) -> HashMap<String, Vec<String>> {
    let pairs: Vec<(String, String)> = bindings
        .iter()
        .filter_map(|j| {
            let name = j["coauthorLabel"]["value"].as_str()?;
            let q = q_from_entity_uri(j["coauthor"]["value"].as_str()?)?;
            Some((q, name.to_string()))
        })
        .collect();

    let mut name2qs: HashMap<String, Vec<String>> = HashMap::new();
    for (author_q, name) in pairs {
        if crate::identifiers::is_qid(&author_q) {
            name2qs.entry(name).or_default().push(author_q);
        }
    }
    name2qs
}

pub struct AuthorNameString {
    pub logging_level: u8,
    fetcher: Arc<dyn JsonFetcher>,
}

impl Default for AuthorNameString {
    fn default() -> Self {
        Self { logging_level: 0, fetcher: Arc::new(HttpJsonFetcher::default()) }
    }
}

impl AuthorNameString {
    /// New with explicit logging verbosity and JSON fetcher. Production
    /// callers use `Self::new(level, Arc::new(HttpJsonFetcher::default()))`;
    /// tests inject a `MockJsonFetcher`.
    pub fn new(logging_level: u8, fetcher: Arc<dyn JsonFetcher>) -> Self {
        Self { logging_level, fetcher }
    }

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
    async fn search_by_initials(&self, name: &str) -> Option<Vec<String>> {
        let formatted = Self::format_name_for_initial_search(name);
        if formatted.is_empty() {
            return None;
        }
        let url = format!("https://wd-infernal.toolforge.org/initial_search/{}", formatted);
        let json = self.fetcher.fetch_json(&url).await?;
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
                Ok(Some(er)) => {
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
                Ok(None) => self.log(1, "No paper item ID!"),
                Err(e) => self.log(1, format!("Error editing paper {paper_q}: {e:#}")),
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
            if let Some(initial_results) = self.search_by_initials(ans).await {
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
        let resp = api
            .sparql_query(&format!(
                "SELECT ?paper ?ans {{ ?paper wdt:P50 wd:{root_author_q} ; wdt:P2093 ?ans }}"
            ))
            .await?;
        let bindings = resp["results"]["bindings"]
            .as_array()
            .ok_or(anyhow::anyhow!("get_coauthor_ans: Not an array"))?;
        Ok(parse_coauthor_ans_bindings(bindings))
    }

    async fn get_coauthor_qs(
        &self,
        root_author_q: &str,
        api: &Api,
    ) -> Result<HashMap<String, Vec<String>>> {
        let resp = api
            .sparql_query(&format!("select DISTINCT ?coauthor ?coauthorLabel {{ ?paper wdt:P50 wd:{root_author_q} ; wdt:P50 ?coauthor . SERVICE wikibase:label {{ bd:serviceParam wikibase:language \"[AUTO_LANGUAGE],en\" }} }}"))
            .await?;
        let bindings = resp["results"]["bindings"]
            .as_array()
            .ok_or(anyhow::anyhow!("get_coauthor_qs: Not an array"))?;
        Ok(parse_coauthor_qs_bindings(bindings))
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

    // === q_from_entity_uri =================================================

    #[test]
    fn q_from_entity_uri_extracts_q_id() {
        assert_eq!(
            q_from_entity_uri("http://www.wikidata.org/entity/Q12345"),
            Some("Q12345".to_string())
        );
    }

    #[test]
    fn q_from_entity_uri_rejects_non_qid_suffix() {
        assert_eq!(q_from_entity_uri("http://example.com/P31"), None);
        assert_eq!(q_from_entity_uri("http://example.com/not-a-qid"), None);
        assert_eq!(q_from_entity_uri(""), None);
    }

    // === parse_coauthor_ans_bindings =======================================
    //
    // Filters: MIN_PAPERS_PER_AUTHOR (= 2) and MIN_SPACES_FOR_NAME (= 1).
    // Each `?paper` is a URI; `?ans` is a string. We group by ans → paper_qs,
    // then drop anything below the thresholds.

    fn ans_binding(paper_qid: &str, ans: &str) -> serde_json::Value {
        serde_json::json!({
            "paper": { "type": "uri", "value": format!("http://www.wikidata.org/entity/{paper_qid}") },
            "ans":   { "type": "literal", "value": ans },
        })
    }

    #[test]
    fn parse_coauthor_ans_bindings_groups_by_ans() {
        let bindings = vec![
            ans_binding("Q1", "John Smith"),
            ans_binding("Q2", "John Smith"),
            ans_binding("Q3", "John Smith"),
        ];
        let out = parse_coauthor_ans_bindings(&bindings);
        assert_eq!(out.len(), 1);
        let papers = out.get("John Smith").unwrap();
        assert_eq!(papers.len(), 3);
    }

    #[test]
    fn parse_coauthor_ans_bindings_filters_below_min_papers() {
        // "John Smith" has only 1 paper → below MIN_PAPERS_PER_AUTHOR (2); drop.
        // "Jane Doe" has 2 → keep.
        let bindings = vec![
            ans_binding("Q1", "John Smith"),
            ans_binding("Q2", "Jane Doe"),
            ans_binding("Q3", "Jane Doe"),
        ];
        let out = parse_coauthor_ans_bindings(&bindings);
        assert!(!out.contains_key("John Smith"), "below MIN_PAPERS_PER_AUTHOR should be dropped");
        assert_eq!(out.get("Jane Doe").map(Vec::len), Some(2));
    }

    #[test]
    fn parse_coauthor_ans_bindings_filters_names_without_spaces() {
        // "Smith" has no space → below MIN_SPACES_FOR_NAME; drop even with
        // enough papers.
        let bindings = vec![
            ans_binding("Q1", "Smith"),
            ans_binding("Q2", "Smith"),
            ans_binding("Q3", "John Smith"),
            ans_binding("Q4", "John Smith"),
        ];
        let out = parse_coauthor_ans_bindings(&bindings);
        assert!(!out.contains_key("Smith"), "single-token name should be dropped");
        assert!(out.contains_key("John Smith"));
    }

    #[test]
    fn parse_coauthor_ans_bindings_skips_non_qid_paper_uris() {
        // A URI whose suffix isn't a Q-id must be ignored — guards against
        // accidentally treating properties or files as papers.
        let bindings = vec![
            serde_json::json!({
                "paper": { "type": "uri", "value": "http://www.wikidata.org/entity/P31" },
                "ans":   { "type": "literal", "value": "John Smith" },
            }),
            ans_binding("Q1", "John Smith"),
            ans_binding("Q2", "John Smith"),
        ];
        let out = parse_coauthor_ans_bindings(&bindings);
        let papers = out.get("John Smith").unwrap();
        assert_eq!(papers.len(), 2, "P31 binding should be filtered out");
        assert!(papers.iter().all(|p| crate::identifiers::is_qid(p)));
    }

    #[test]
    fn parse_coauthor_ans_bindings_returns_empty_for_no_bindings() {
        let out = parse_coauthor_ans_bindings(&[]);
        assert!(out.is_empty());
    }

    // === parse_coauthor_qs_bindings ========================================

    fn qs_binding(coauthor_qid: &str, label: &str) -> serde_json::Value {
        serde_json::json!({
            "coauthor":      { "type": "uri", "value": format!("http://www.wikidata.org/entity/{coauthor_qid}") },
            "coauthorLabel": { "type": "literal", "value": label },
        })
    }

    #[test]
    fn parse_coauthor_qs_bindings_groups_by_label() {
        let bindings = vec![
            qs_binding("Q1", "John Smith"),
            qs_binding("Q2", "John Smith"), // same label, different Q
            qs_binding("Q3", "Jane Doe"),
        ];
        let out = parse_coauthor_qs_bindings(&bindings);
        assert_eq!(out.get("John Smith").map(Vec::len), Some(2));
        assert_eq!(out.get("Jane Doe").map(Vec::len), Some(1));
    }

    #[test]
    fn parse_coauthor_qs_bindings_skips_non_qid_uris() {
        let bindings = vec![
            serde_json::json!({
                "coauthor":      { "type": "uri", "value": "http://www.wikidata.org/entity/P31" },
                "coauthorLabel": { "type": "literal", "value": "John Smith" },
            }),
            qs_binding("Q5", "John Smith"),
        ];
        let out = parse_coauthor_qs_bindings(&bindings);
        let qs = out.get("John Smith").unwrap();
        assert_eq!(qs, &vec!["Q5".to_string()]);
    }

    #[test]
    fn parse_coauthor_qs_bindings_does_not_apply_paper_count_filter() {
        // Unlike parse_coauthor_ans_bindings, this one keeps single-entry
        // labels — the threshold is only applied to the ans→papers map.
        let bindings = vec![qs_binding("Q1", "John Smith")];
        let out = parse_coauthor_qs_bindings(&bindings);
        assert_eq!(out.len(), 1);
    }

    // === search_by_initials ================================================

    use crate::http_client::MockJsonFetcher;

    #[tokio::test]
    async fn search_by_initials_hits_expected_url_and_returns_q_ids() {
        let fetcher = Arc::new(MockJsonFetcher::new());
        let url = "https://wd-infernal.toolforge.org/initial_search/C.K.Clarke";
        fetcher.add_response(url, serde_json::json!(["Q1", "Q2"]));
        let ans = AuthorNameString::new(0, fetcher.clone());

        let result = ans.search_by_initials("C K Clarke").await;
        assert_eq!(result, Some(vec!["Q1".to_string(), "Q2".to_string()]));
        assert_eq!(fetcher.captured_urls(), vec![url.to_string()]);
    }

    #[tokio::test]
    async fn search_by_initials_returns_none_on_fetch_failure() {
        let fetcher = Arc::new(MockJsonFetcher::new());
        let url = "https://wd-infernal.toolforge.org/initial_search/C.K.Clarke";
        fetcher.add_failure(url);
        let ans = AuthorNameString::new(0, fetcher);
        assert!(ans.search_by_initials("C K Clarke").await.is_none());
    }

    #[tokio::test]
    async fn search_by_initials_returns_none_on_empty_formatted_name() {
        // An empty-string input formats to "" → guard returns None before
        // any HTTP call.
        let fetcher = Arc::new(MockJsonFetcher::new());
        let ans = AuthorNameString::new(0, fetcher.clone());
        assert!(ans.search_by_initials("").await.is_none());
        assert!(
            fetcher.captured_urls().is_empty(),
            "should not hit the fetcher for empty input, got {:?}",
            fetcher.captured_urls()
        );
    }

    #[tokio::test]
    async fn search_by_initials_returns_none_on_non_array_response() {
        // The endpoint contract is "JSON array of Q-id strings". If the
        // server hands us something else (object, number, etc.), parsing
        // should fail closed.
        let fetcher = Arc::new(MockJsonFetcher::new());
        let url = "https://wd-infernal.toolforge.org/initial_search/C.K.Clarke";
        fetcher.add_response(url, serde_json::json!({"oops": "not an array"}));
        let ans = AuthorNameString::new(0, fetcher);
        assert!(ans.search_by_initials("C K Clarke").await.is_none());
    }
}
