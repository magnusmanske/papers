use crate::wikidata_interaction::WikidataInteraction;
use crate::wikidata_papers::WikidataPapers;
use crate::{generic_author_info::GenericAuthorInfo, wikidata_string_cache::WikidataStringCache};
use futures::prelude::*;
use regex::Regex;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use wikibase::mediawiki::api::Api;

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
    fn log(&self, level: u8, msg: &str) {
        if self.logging_level <= level {
            println!("{}", msg);
        }
    }

    async fn process_papers_for_ans(
        &self,
        ans: &String,
        cache: &Arc<WikidataStringCache>,
        mw_api: &Arc<RwLock<Api>>,
        paper_qs: &Vec<String>,
        name2author_qs: &HashMap<String, Vec<String>>,
    ) {
        let simple_name = GenericAuthorInfo::simplify_name(ans);
        let res = cache
            .search_wikibase(
                &format!("{simple_name} haswbstatement:P31=Q5"),
                mw_api.clone(),
            )
            .await
            .unwrap();
        let author_q = if res.is_empty() {
            Some(self.create_new_author(ans, mw_api, cache).await)
        } else if res.len() == 1 {
            if let Some(author_qs) = name2author_qs.get(ans) {
                if author_qs.len() == 1 {
                    let author_q = &author_qs[0];
                    println!("MATCHED AUTHOR {ans}: {simple_name} => {author_q}");
                    Some(author_q.to_owned())
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            self.log(
                2,
                &format!("MULTIPLE POSSIBLE MATCHES FOR {ans}: {simple_name} => {res:?}"),
            );
            None
        };

        let author_q = match author_q {
            Some(q) => q,
            None => return,
        };
        println!("AUTHOR {author_q}");

        let mut author = GenericAuthorInfo::new();
        author.name = Some(ans.clone());
        author.wikidata_item = Some(author_q.clone());
        let mut papers = WikidataPapers::new(cache.clone());
        let api = mw_api.read().await;
        if let Err(_e) = papers.entities_mut().load_entities(&api, paper_qs).await {
            panic!("Could not load paper items {paper_qs:?}");
        }
        drop(api);

        // Create P50 statements
        let mut edited_qs = vec![];
        for paper_q in paper_qs {
            let mut item = match papers.entities_mut().get_entity(paper_q) {
                Some(item) => item,
                None => continue,
            };
            let original_item = item.clone();
            papers.update_author_name_statement(ans, &author, &mut item);
            println!("EDITING PAPER {paper_q}: {ans} => {author_q}");
            papers.set_edit_summary(Some(format!(
                "Changing {ans} to {author_q} [#Papers (was: SourceMD)]"
            )));
            match papers
                .apply_diff_for_item(original_item, item, mw_api.clone())
                .await
            {
                Some(er) => {
                    if er.edited {
                        println!("Created or updated https://www.wikidata.org/wiki/{}", &er.q);
                        edited_qs.push(er.q.clone());
                    } else {
                        println!("https://www.wikidata.org/wiki/{}, no changes ", &er.q);
                    }
                }
                None => println!("No item ID!"),
            }
            // papers.set_edit_summary(None);
            // save_item_changes(&mut papers, mw_api.clone(), paper_q).await;
        }

        if !edited_qs.is_empty() {
            let api = mw_api.read().await;
            let _ = papers
                .entities_mut()
                .reload_entities(&api, &edited_qs)
                .await;
            drop(api);
        }
    }

    pub async fn process_author_q(
        &self,
        root_author_q: String,
        mw_api: &Arc<RwLock<Api>>,
        cache: &Arc<WikidataStringCache>,
    ) {
        self.log(1, &format!("Processing {}", &root_author_q));
        let mut author = GenericAuthorInfo::new();
        author.wikidata_item = Some(root_author_q.to_owned());
        let api = mw_api.read().await;
        let ans2paper_qs = self.get_coauthor_ans(&root_author_q, &api).await;
        let name2author_qs = self.get_coauthor_qs(&root_author_q, &api).await;
        drop(api);

        let mut futures = vec![];
        for (ans, paper_qs) in ans2paper_qs.iter() {
            let future = self.process_papers_for_ans(ans, cache, mw_api, paper_qs, &name2author_qs);
            futures.push(future);
        }
        // futures::future::join_all(futures).await;

        let stream =
            futures::stream::iter(futures).buffer_unordered(MAX_PROCESS_PAPERS_CONCURRENCY);
        stream.collect::<Vec<_>>().await;
    }

    /// Get coauthors of a given author, author name string to paper Qids
    async fn get_coauthor_ans(
        &self,
        root_author_q: &str,
        api: &Api,
    ) -> HashMap<String, Vec<String>> {
        // (paper Qid, author name string)
        let result_ans: Vec<(String, String)> = api
            .sparql_query(&format!(
                "SELECT ?paper ?ans {{ ?paper wdt:P50 wd:{root_author_q} ; wdt:P2093 ?ans }}"
            ))
            .await
            .unwrap()["results"]["bindings"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|j| {
                let ans = j["ans"]["value"].as_str()?;
                let q = api
                    .extract_entity_from_uri(j["paper"]["value"].as_str()?)
                    .ok()?;
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
        ans2paper_qs
    }

    async fn get_coauthor_qs(
        &self,
        root_author_q: &str,
        api: &Api,
    ) -> HashMap<String, Vec<String>> {
        // (Qid, name)
        let result_coauthors: Vec<(String, String)> = api
        .sparql_query(&format!("select DISTINCT ?coauthor ?coauthorLabel {{ ?paper wdt:P50 wd:{root_author_q} ; wdt:P50 ?coauthor . SERVICE wikibase:label {{ bd:serviceParam wikibase:language \"[AUTO_LANGUAGE],en\" }} }}"))
        .await
        .unwrap()["results"]["bindings"]
        .as_array()
        .unwrap()
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
        name2qs
    }

    async fn create_new_author(
        &self,
        ans: &String,
        mw_api: &Arc<RwLock<Api>>,
        cache: &Arc<WikidataStringCache>,
    ) -> String {
        self.log(1, &format!("CREATING AUTHOR {ans}"));
        let mut author = GenericAuthorInfo::new();
        author.name = Some(ans.clone());
        let author = author
            .get_or_create_author_item(mw_api.clone(), cache.clone(), true)
            .await;
        self.log(1, &format!("CREATED AUTHOR {ans} => {author:?}"));
        author.wikidata_item.as_ref().unwrap().to_owned()
    }
}
