use self::identifiers::IdProp;
use crate::generic_author_info::GenericAuthorInfo;
use crate::scientific_publication_adapter::ScientificPublicationAdapter;
use crate::*;
use async_trait::async_trait;
use orcid::*;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Debug, Clone, Default)]
pub struct PseudoWork {
    pub author_ids: Vec<String>,
}

impl PseudoWork {
    pub fn new() -> Self {
        Self::default()
    }
}

#[derive(Debug, Clone)]
pub struct Orcid2Wikidata {
    author_cache: HashMap<String, String>,
    work_cache: HashMap<String, PseudoWork>,
    client: Client,
    author_data: Arc<Mutex<HashMap<String, Option<Author>>>>,
}

impl Default for Orcid2Wikidata {
    fn default() -> Self {
        Self::new()
    }
}

impl Orcid2Wikidata {
    pub fn new() -> Self {
        Orcid2Wikidata {
            author_cache: HashMap::new(),
            work_cache: HashMap::new(),
            client: Client::new(),
            author_data: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn get_cached_publication_from_id(&self, publication_id: &str) -> Option<&PseudoWork> {
        self.work_cache.get(publication_id)
    }

    pub async fn get_or_load_author_data(&self, orcid_author_id: &str) -> Option<Author> {
        if !self.author_data.lock().await.contains_key(orcid_author_id) {
            let data = self.client.author(&orcid_author_id.to_string()).await.ok();
            self.author_data
                .lock()
                .await
                .insert(orcid_author_id.to_string(), data);
        }
        self.author_data
            .lock()
            .await
            .get(orcid_author_id)
            .and_then(|r| r.clone())
    }

    async fn get_author_data(
        &self,
        orcid_author_id: &str,
        author_property: &str,
    ) -> Option<GenericAuthorInfo> {
        if let Some(author) = self.get_or_load_author_data(orcid_author_id).await {
            let mut gai = GenericAuthorInfo {
                name: None,
                prop2id: HashMap::new(),
                wikidata_item: None,
                list_number: None,
                alternative_names: vec![],
            };
            match author.credit_name() {
                Some(name) => gai.name = Some(name.to_string()),
                None => {
                    let j = author.json();
                    let last_name = j["person"]["name"]["family-name"]["value"].as_str();
                    let given_names = j["person"]["name"]["given-names"]["value"].as_str();
                    match (given_names, last_name) {
                        (Some(f), Some(l)) => gai.name = Some(format!("{} {}", &f, &l)),
                        (None, Some(l)) => gai.name = Some(l.to_string()),
                        _ => {}
                    }
                }
            }
            if let Some(id) = author.orcid_id() {
                gai.prop2id
                    .insert(author_property.to_string(), id.to_string());
            }
            let ext_ids = author.external_ids();
            for id in ext_ids {
                match id.0.as_str() {
                    "ResearcherID" => {
                        gai.prop2id.insert("P1053".to_string(), id.1);
                    }
                    "Researcher ID" => {
                        gai.prop2id.insert("P1053".to_string(), id.1);
                    }
                    "Scopus Author ID" => {
                        gai.prop2id.insert("P1153".to_string(), id.1);
                    }
                    "Scopus ID" => {
                        gai.prop2id.insert("P1153".to_string(), id.1);
                    }
                    "Loop profile" => {
                        gai.prop2id.insert("P2798".to_string(), id.1);
                    }
                    "SciProfiles" => {
                        gai.prop2id.insert("P8159".to_string(), id.1);
                    }
                    "GitHub" => {
                        gai.prop2id.insert("P2037".to_string(), id.1);
                    }
                    "Ciência ID" => {
                        gai.prop2id.insert("P7893".to_string(), id.1);
                    }
                    // "Researcher Name Resolver ID" => {
                    //     gai.prop2id.insert("P9776".to_string(), id.1);
                    // }
                    "ISNI" => {
                        gai.prop2id
                            .insert("P213".to_string(), id.1.replace("-", ""));
                    }
                    other => {
                        self.warn(&format!(
                            "orcid2wikidata: Unknown ID '{}':'{}'",
                            &other, &id.1
                        ));
                    }
                }
            }
            return Some(gai);
        }
        None
    }
}

#[async_trait(?Send)]
impl ScientificPublicationAdapter for Orcid2Wikidata {
    fn name(&self) -> &str {
        "Orcid2Wikidata"
    }

    fn author_property(&self) -> Option<String> {
        Some("P496".to_string())
    }

    fn author_cache(&self) -> &HashMap<String, String> {
        &self.author_cache
    }

    fn author_cache_mut(&mut self) -> &mut HashMap<String, String> {
        &mut self.author_cache
    }

    async fn publication_id_from_item(&mut self, item: &Entity) -> Option<String> {
        // TODO other ID types than DOI?
        let doi = self.get_external_identifier_from_item(item, &IdProp::DOI)?;
        let author_ids = self.client.search_doi(&doi).await.ok()?;

        let work = PseudoWork { author_ids };
        let publication_id = doi;
        self.work_cache.insert(publication_id.clone(), work);
        Some(publication_id)
    }

    async fn update_statements_for_publication_id(&self, publication_id: &str, _item: &mut Entity) {
        let _work = match self.get_cached_publication_from_id(publication_id) {
            Some(w) => w,
            None => return,
        };
    }

    async fn get_author_list(&mut self, publication_id: &str) -> Vec<GenericAuthorInfo> {
        let work = match self.get_cached_publication_from_id(publication_id) {
            Some(w) => w.clone(),
            None => return vec![],
        };
        let author_property = match self.author_property() {
            Some(p) => p,
            None => return vec![],
        };

        let mut futures = Vec::new();
        for orcid_author_id in &work.author_ids {
            let future = self.get_author_data(orcid_author_id, &author_property);
            futures.push(future);
        }
        futures::future::join_all(futures)
            .await
            .into_iter()
            .flatten()
            .collect()
    }
}

#[cfg(test)]
mod tests {}
