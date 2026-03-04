use crate::generic_author_info::GenericAuthorInfo;
use crate::identifiers::{GenericWorkIdentifier, GenericWorkType, IdProp};
use crate::scientific_publication_adapter::ScientificPublicationAdapter;
use crate::*;
use async_trait::async_trait;
use pubmed::*;
use regex::Regex;
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct Pubmed2Wikidata {
    author_cache: HashMap<String, String>,
    work_cache: HashMap<String, PubmedArticle>,
    query_cache: HashMap<String, Vec<u64>>,
    client: Client,
}

impl Default for Pubmed2Wikidata {
    fn default() -> Self {
        Self::new()
    }
}

impl Pubmed2Wikidata {
    pub fn new() -> Self {
        Pubmed2Wikidata {
            author_cache: HashMap::new(),
            work_cache: HashMap::new(),
            query_cache: HashMap::new(),
            client: Client::new(),
        }
    }

    pub fn get_cached_publication_from_id(&self, publication_id: &str) -> Option<&PubmedArticle> {
        self.work_cache.get(publication_id)
    }

    fn get_author_name_string(&self, author: &Author) -> Option<String> {
        let mut ret: String = match &author.last_name {
            Some(s) => s.to_string(),
            None => return None,
        };
        match &author.fore_name {
            Some(s) => ret = s.to_string() + " " + &ret,
            None => {
                if let Some(s) = &author.initials {
                    ret = s.to_string() + " " + &ret
                }
            }
        }
        Some(self.sanitize_author_name(&ret))
    }

    fn is_pubmed_id(&self, id: &str) -> bool {
        lazy_static! {
            static ref RE_PMID: Regex = Regex::new(r#"^(\d+)$"#)
                .expect("Pubmed2Wikidata::is_pubmed_id: RE_PMID does not compile");
        }
        RE_PMID.is_match(id)
    }

    async fn publication_id_from_pubmed(&mut self, publication_id: &str) -> Option<String> {
        if !self.is_pubmed_id(publication_id) {
            return None;
        }
        if !self.work_cache.contains_key(publication_id) {
            let pub_id_u64 = publication_id.parse::<u64>().ok()?;
            let work = self.client.article(pub_id_u64).await.ok()?;
            self.work_cache.insert(publication_id.to_string(), work);
        }
        Some(publication_id.to_string())
    }

    async fn publication_ids_from_doi(&mut self, doi: &str) -> Vec<String> {
        let query = doi.to_string();
        let work_ids: Vec<u64> = match self.query_cache.get(&query) {
            Some(work_ids) => work_ids.clone(),
            None => self
                .client
                .article_ids_from_query(&query, 10)
                .await
                .unwrap_or_default(),
        };
        self.query_cache.insert(query, work_ids.clone());
        for publication_id in &work_ids {
            if let std::collections::hash_map::Entry::Vacant(e) =
                self.work_cache.entry(publication_id.to_string())
            {
                match self.client.article(*publication_id).await {
                    Ok(work) => {
                        e.insert(work);
                    }
                    Err(e) => self.warn(&format!("pubmed::publication_ids_from_doi: {:?}", &e)),
                }
            }
        }
        // Filter to only include articles that actually contain the queried DOI.
        // PubMed's text search can return unrelated articles that happen to match
        // the query string but have a different DOI (or no DOI at all).
        let doi_upper = doi.to_uppercase();
        work_ids
            .iter()
            .map(|s| s.to_string())
            .filter(|pub_id| {
                self.get_dois_from_cached_publication(pub_id)
                    .contains(&doi_upper)
            })
            .collect()
    }

    fn get_dois_from_cached_publication(&self, publication_id: &str) -> Vec<String> {
        let work = match self.get_cached_publication_from_id(publication_id) {
            Some(w) => w,
            None => return vec![],
        };
        let mut dois = vec![];
        if let Some(pubmed_data) = &work.pubmed_data {
            if let Some(article_ids) = &pubmed_data.article_ids {
                for id in &article_ids.ids {
                    if let (Some(key), Some(id)) = (&id.id_type, &id.id) {
                        if key == "doi" {
                            dois.push(id.to_uppercase());
                        }
                    }
                }
            }
        }
        if let Some(medline_citation) = &work.medline_citation {
            if let Some(article) = &medline_citation.article {
                for elid in &article.e_location_ids {
                    if elid.valid {
                        if let (Some(id_type), Some(id)) = (&elid.e_id_type, &elid.id) {
                            if id_type == "doi" {
                                dois.push(id.to_uppercase());
                            }
                        }
                    }
                }
            }
        }
        dois.sort();
        dois.dedup();
        dois
    }

    fn add_identifiers_from_cached_publication(
        &mut self,
        publication_id: &str,
        ret: &mut Vec<GenericWorkIdentifier>,
    ) -> Option<()> {
        let my_prop = self.publication_property()?;

        let work = self.get_cached_publication_from_id(publication_id)?;

        ret.push(GenericWorkIdentifier::new_prop(my_prop, publication_id));

        let medline_citation = work.medline_citation.to_owned()?;
        let article = medline_citation.article?;

        if let Some(pubmed_data) = &work.pubmed_data {
            if let Some(article_ids) = &pubmed_data.article_ids {
                article_ids.ids.iter().for_each(|id| {
                    if let (Some(key), Some(id)) = (&id.id_type, &id.id) {
                        if let "doi" = key.as_str() {
                            ret.push(GenericWorkIdentifier::new_prop(IdProp::DOI, id));
                        }
                    }
                });
            }
        }

        // ???
        for elid in &article.e_location_ids {
            if !elid.valid {
                continue;
            }
            match (&elid.e_id_type, &elid.id) {
                (Some(id_type), Some(id)) => {
                    match id_type.as_str() {
                        "doi" => ret.push(GenericWorkIdentifier::new_prop(IdProp::DOI, id)),
                        other => {
                            self.warn(&format!("pubmed2wikidata::get_identifier_list unknown paper ID type '{other}'"));
                        }
                    }
                }
                _ => continue,
            }
        }
        Some(())
    }
}

#[async_trait(?Send)]
impl ScientificPublicationAdapter for Pubmed2Wikidata {
    fn name(&self) -> &str {
        "Pubmed2Wikidata"
    }

    fn author_cache(&self) -> &HashMap<String, String> {
        &self.author_cache
    }

    fn author_cache_mut(&mut self) -> &mut HashMap<String, String> {
        &mut self.author_cache
    }

    fn publication_property(&self) -> Option<IdProp> {
        Some(IdProp::PMID)
    }

    async fn publication_id_from_item(&mut self, item: &Entity) -> Option<String> {
        let publication_id =
            self.get_external_identifier_from_item(item, &self.publication_property()?)?;
        self.publication_id_from_pubmed(&publication_id).await
    }

    fn get_work_titles(&self, publication_id: &str) -> Vec<LocaleString> {
        let title = self
            .get_cached_publication_from_id(publication_id)
            .and_then(|w| w.medline_citation.as_ref())
            .and_then(|c| c.article.as_ref())
            .and_then(|a| a.title.as_ref());
        match title {
            Some(t) => vec![LocaleString::new("en", t)],
            None => vec![],
        }
    }

    fn get_work_issn(&self, publication_id: &str) -> Option<String> {
        let work = self
            .get_cached_publication_from_id(publication_id)?
            .to_owned();
        let medline_citation = work.medline_citation?.to_owned();
        let article = medline_citation.article?.to_owned();
        let journal = article.journal?.to_owned();
        let issn = journal.issn?.to_owned();
        Some(issn)
    }

    async fn get_identifier_list(
        &mut self,
        ids: &[GenericWorkIdentifier],
    ) -> Vec<GenericWorkIdentifier> {
        let mut ret: Vec<GenericWorkIdentifier> = vec![];
        for id in ids {
            if let GenericWorkType::Property(prop) = id.work_type() {
                match prop {
                    IdProp::PMID => {
                        if let Some(publication_id) = self.publication_id_from_pubmed(id.id()).await
                        {
                            self.add_identifiers_from_cached_publication(&publication_id, &mut ret);
                        }
                    }
                    IdProp::DOI => {
                        for publication_id in self.publication_ids_from_doi(id.id()).await {
                            self.add_identifiers_from_cached_publication(&publication_id, &mut ret);
                        }
                    }
                    _ => {}
                }
            }
        }
        ret
    }

    async fn update_statements_for_publication_id(&self, publication_id: &str, item: &mut Entity) {
        let work = match self.get_cached_publication_from_id(publication_id) {
            Some(w) => w,
            None => return,
        };

        // Work language
        if !item.has_claims_with_property("P407") {
            if let Some(medline_citation) = &work.medline_citation {
                if let Some(article) = &medline_citation.article {
                    if let Some(language) = &article.language {
                        if let Some(q) = self.language2q(language).await {
                            let statement = Statement::new_normal(
                                Snak::new_item("P407", &q),
                                vec![],
                                self.reference(),
                            );
                            item.add_claim(statement);
                        }
                    }
                }
            }
        }
    }

    async fn get_language_item(&self, publication_id: &str) -> Option<String> {
        self.language2q(
            self.get_cached_publication_from_id(publication_id)?
                .medline_citation
                .as_ref()?
                .article
                .as_ref()?
                .language
                .as_ref()?,
        )
        .await
    }

    fn get_publication_date(&self, publication_id: &str) -> Option<(u32, Option<u8>, Option<u8>)> {
        let pub_date = self
            .get_cached_publication_from_id(publication_id)?
            .medline_citation
            .as_ref()?
            .article
            .as_ref()?
            .journal
            .as_ref()?
            .journal_issue
            .as_ref()?
            .pub_date
            .as_ref()?;
        let month = match pub_date.month {
            0 => None,
            x => Some(x),
        };
        let day = match pub_date.day {
            0 => None,
            x => Some(x),
        };
        Some((pub_date.year, month, day))
    }

    fn get_volume(&self, publication_id: &str) -> Option<String> {
        self.get_cached_publication_from_id(publication_id)?
            .medline_citation
            .as_ref()?
            .article
            .as_ref()?
            .journal
            .as_ref()?
            .journal_issue
            .as_ref()?
            .volume
            .as_ref()
            .map(|s| s.to_string())
    }

    fn get_issue(&self, publication_id: &str) -> Option<String> {
        self.get_cached_publication_from_id(publication_id)?
            .medline_citation
            .as_ref()?
            .article
            .as_ref()?
            .journal
            .as_ref()?
            .journal_issue
            .as_ref()?
            .issue
            .as_ref()
            .map(|s| s.to_string())
    }

    async fn do_cache_work(&mut self, publication_id: &str) -> Option<String> {
        let pub_id_u64 = publication_id.parse::<u64>().ok()?;
        let work = self.client.article(pub_id_u64).await.ok()?;
        self.work_cache.insert(publication_id.to_string(), work);
        Some(publication_id.to_string())
    }

    async fn get_author_list(&mut self, publication_id: &str) -> Vec<GenericAuthorInfo> {
        let work = match self.get_cached_publication_from_id(publication_id) {
            Some(w) => w,
            None => return vec![],
        };
        let mut ret: Vec<GenericAuthorInfo> = vec![];

        let medline_citation = match &work.medline_citation {
            Some(x) => x,
            None => return ret,
        };
        let article = match &medline_citation.article {
            Some(x) => x,
            None => return ret,
        };
        let author_list = match &article.author_list {
            Some(x) => x,
            None => return ret,
        };

        if !author_list.complete {
            return ret;
        }

        let mut list_num = 1;
        for author in &author_list.authors {
            let mut prop2id: HashMap<String, String> = HashMap::new();
            for aid in &author.identifiers {
                if let (Some(source), Some(id)) = (&aid.source, &aid.id) {
                    match source.as_str() {
                        "ORCID" => {
                            // URL => ID
                            if let Some(id) = id.split('/').next_back() {
                                prop2id.insert("P496".to_string(), id.to_string());
                            }
                        }
                        other => self.warn(&format!("Unknown author source: {other}:{id}")),
                    }
                }
            }
            ret.push(GenericAuthorInfo {
                name: self.get_author_name_string(author),
                prop2id,
                wikidata_item: None,
                list_number: Some(list_num.to_string()),
                alternative_names: vec![],
            });
            list_num += 1;
        }

        ret
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: creates a PubmedArticle with the given PMID and DOI (in both
    /// pubmed_data.article_ids and article.e_location_ids).
    fn make_article(pmid: u64, doi: Option<&str>) -> PubmedArticle {
        let article_ids = doi.map(|d| ArticleIdList {
            ids: vec![ArticleId {
                id_type: Some("doi".to_string()),
                id: Some(d.to_string()),
            }],
        });
        let e_location_ids = match doi {
            Some(d) => vec![ELocationID {
                e_id_type: Some("doi".to_string()),
                valid: true,
                id: Some(d.to_string()),
            }],
            None => vec![],
        };
        PubmedArticle {
            medline_citation: Some(MedlineCitation {
                pmid,
                article: Some(Article {
                    e_location_ids,
                    ..Article::new()
                }),
                ..MedlineCitation::new()
            }),
            pubmed_data: Some(PubmedData {
                article_ids,
                history: vec![],
                references: vec![],
                publication_status: None,
            }),
        }
    }

    #[test]
    fn test_get_dois_from_cached_publication_with_doi() {
        let mut pm = Pubmed2Wikidata::new();
        pm.work_cache.insert(
            "12345".to_string(),
            make_article(12345, Some("10.1234/test")),
        );
        let dois = pm.get_dois_from_cached_publication("12345");
        assert!(dois.contains(&"10.1234/TEST".to_string()));
    }

    #[test]
    fn test_get_dois_from_cached_publication_no_doi() {
        let mut pm = Pubmed2Wikidata::new();
        pm.work_cache
            .insert("99999".to_string(), make_article(99999, None));
        let dois = pm.get_dois_from_cached_publication("99999");
        assert!(dois.is_empty());
    }

    #[test]
    fn test_get_dois_from_cached_publication_missing_article() {
        let pm = Pubmed2Wikidata::new();
        let dois = pm.get_dois_from_cached_publication("nonexistent");
        assert!(dois.is_empty());
    }

    #[tokio::test]
    async fn test_publication_ids_from_doi_filters_wrong_articles() {
        // Simulate PubMed text search returning two articles for a DOI query,
        // but only one of them actually has that DOI.
        let target_doi = "10.3390/math10050822";
        let wrong_doi = "10.9999/unrelated";

        let mut pm = Pubmed2Wikidata::new();
        // Pre-populate query_cache so no network call is made
        pm.query_cache
            .insert(target_doi.to_string(), vec![11111, 22222]);
        // Pre-populate work_cache: article 11111 has the correct DOI,
        // article 22222 has a different DOI
        pm.work_cache
            .insert("11111".to_string(), make_article(11111, Some(target_doi)));
        pm.work_cache
            .insert("22222".to_string(), make_article(22222, Some(wrong_doi)));

        let result = pm.publication_ids_from_doi(target_doi).await;
        assert_eq!(result, vec!["11111".to_string()]);
        assert!(!result.contains(&"22222".to_string()));
    }

    #[tokio::test]
    async fn test_publication_ids_from_doi_filters_all_when_none_match() {
        let target_doi = "10.3390/math10050822";

        let mut pm = Pubmed2Wikidata::new();
        pm.query_cache.insert(target_doi.to_string(), vec![33333]);
        pm.work_cache.insert(
            "33333".to_string(),
            make_article(33333, Some("10.9999/different")),
        );

        let result = pm.publication_ids_from_doi(target_doi).await;
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn test_publication_ids_from_doi_case_insensitive() {
        // DOI comparison should be case-insensitive
        let target_doi = "10.3390/Math10050822"; // mixed case input
        let article_doi = "10.3390/math10050822"; // lowercase in article

        let mut pm = Pubmed2Wikidata::new();
        pm.query_cache.insert(target_doi.to_string(), vec![44444]);
        pm.work_cache
            .insert("44444".to_string(), make_article(44444, Some(article_doi)));

        let result = pm.publication_ids_from_doi(target_doi).await;
        assert_eq!(result, vec!["44444".to_string()]);
    }
}
