extern crate lazy_static;

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
            None => match &author.initials {
                Some(s) => ret = s.to_string() + " " + &ret,
                None => {}
            },
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
        work_ids.iter().map(|s| s.to_string()).collect()
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

        match &work.pubmed_data {
            Some(pubmed_data) => match &pubmed_data.article_ids {
                Some(article_ids) => {
                    article_ids.ids.iter().for_each(|id| {
                        if let (Some(key), Some(id)) = (&id.id_type, &id.id) {
                            if let "doi" = key.as_str() {
                                ret.push(GenericWorkIdentifier::new_prop(IdProp::DOI, id));
                            }
                        }
                    });
                }
                None => {}
            },
            None => {}
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
            match self.get_external_identifier_from_item(item, &self.publication_property()?) {
                Some(s) => s,
                None => return None,
            };
        self.publication_id_from_pubmed(&publication_id).await
    }

    fn get_work_titles(&self, publication_id: &str) -> Vec<LocaleString> {
        match self.get_cached_publication_from_id(publication_id) {
            Some(work) => match &work.medline_citation {
                Some(citation) => match &citation.article {
                    Some(article) => match &article.title {
                        Some(title) => vec![LocaleString::new("en", title)],
                        None => vec![],
                    },
                    None => vec![],
                },
                None => vec![],
            },
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
            match &work.medline_citation {
                Some(medline_citation) => match &medline_citation.article {
                    Some(article) => match &article.language {
                        Some(language) => {
                            if let Some(q) = self.language2q(language).await {
                                let statement = Statement::new_normal(
                                    Snak::new_item("P407", &q),
                                    vec![],
                                    self.reference(),
                                );
                                item.add_claim(statement);
                            }
                        }
                        None => {}
                    },
                    None => {}
                },
                None => {}
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

    fn get_author_list(&mut self, publication_id: &str) -> Vec<GenericAuthorInfo> {
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
                            if let Some(id) = id.split('/').last() {
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
    //use super::*;
    //use wikibase::mediawiki::api::Api;

    /*
    TODO:
    pub fn new() -> Self {
    pub fn get_cached_publication_from_id(
    fn get_author_name_string(&self, author: &Author) -> Option<String> {
    fn publication_id_from_pubmed(&mut self, publication_id: &str) -> Option<String> {
    fn publication_ids_from_doi(&mut self, doi: &str) -> Vec<String> {
    fn add_identifiers_from_cached_publication(
    fn name(&self) -> &str {
    fn author_cache(&self) -> &HashMap<String, String> {
    fn author_cache_mut(&mut self) -> &mut HashMap<String, String> {
    fn publication_property(&self) -> Option<String> {
    fn get_work_titles(&self, publication_id: &str) -> Vec<LocaleString> {
    fn get_work_issn(&self, publication_id: &str) -> Option<String> {
    fn get_identifier_list(
    fn update_statements_for_publication_id(&self, publication_id: &str, item: &mut Entity) {
    fn do_cache_work(&mut self, publication_id: &str) -> Option<String> {
    fn get_author_list(&mut self, publication_id: &str) -> Vec<GenericAuthorInfo> {
    */
}
