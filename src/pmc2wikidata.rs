extern crate lazy_static;

use crate::generic_author_info::GenericAuthorInfo;
use crate::scientific_publication_adapter::ScientificPublicationAdapter;
use crate::*;
use async_trait::async_trait;
use regex::Regex;
use reqwest;
use std::collections::HashMap;

use self::identifiers::{GenericWorkIdentifier, GenericWorkType, IdProp};
//use wikibase::mediawiki::api::Api;

/*
Examples:
https://www.ebi.ac.uk/europepmc/webservices/rest/search?query=EXT_ID:13777676%20AND%20SRC:MED&resulttype=core&format=json (no PMCID)
https://www.ebi.ac.uk/europepmc/webservices/rest/search?query=EXT_ID:17246615%20AND%20SRC:MED&resulttype=core&format=json (with PMCID)
https://www.ebi.ac.uk/europepmc/webservices/rest/search?query=PMC1201091&resulttype=core&format=json (same as line above)
*/

#[derive(Debug, Clone, Default)]
pub struct PMC2Wikidata {
    author_cache: HashMap<String, String>,
    work_cache: HashMap<String, serde_json::Value>,
}

impl PMC2Wikidata {
    pub fn new() -> Self {
        Self::default()
    }

    fn is_pubmed_id(&self, id: &str) -> bool {
        lazy_static! {
            static ref RE_PMID: Regex = Regex::new(r#"^(\d+)$"#)
                .expect("PMC2Wikidata::is_pubmed_id: RE_PMID does not compile");
        }
        RE_PMID.is_match(id)
    }

    fn publication_id_from_pubmed(&mut self, pubmed_id: &str) -> Option<String> {
        if !self.is_pubmed_id(pubmed_id) {
            return None;
        }
        let mut publication_id = pubmed_id.to_string(); // Fallback
        if !self.work_cache.contains_key(pubmed_id) {
            let url = format!("https://www.ebi.ac.uk/europepmc/webservices/rest/search?query=EXT_ID:{}%20AND%20SRC:MED&resulttype=core&format=json",pubmed_id) ;
            let json: serde_json::Value = reqwest::blocking::get(url.as_str()).ok()?.json().ok()?;
            let results = json["resultList"]["result"].as_array()?;
            if results.len() == 1 {
                match results.first() {
                    Some(json) => {
                        if let Some(pmc_id) = self.get_pmcid_from_work(json) {
                            publication_id = pmc_id.to_string()
                        }
                        self.work_cache.insert(publication_id.clone(), json.clone());
                    }
                    None => return None,
                }
            }
        }
        Some(publication_id)
    }

    fn is_pmcid(&self, id: &str) -> bool {
        lazy_static! {
            static ref RE_PMCID: Regex = Regex::new(r#"^(PMC\d+)$"#)
                .expect("main.rs::paper_from_id: RE_PMCID does not compile");
        }
        RE_PMCID.is_match(id)
    }

    fn publication_id_from_pmcid(&mut self, pmc_id: &str) -> Option<String> {
        if !self.is_pmcid(pmc_id) {
            return None;
        }
        if !self.work_cache.contains_key(pmc_id) {
            let url = format!("https://www.ebi.ac.uk/europepmc/webservices/rest/search?query={}&resulttype=core&format=json",pmc_id) ;
            let json: serde_json::Value = reqwest::blocking::get(url.as_str()).ok()?.json().ok()?;
            let results = json["resultList"]["result"].as_array()?;
            if results.len() == 1 {
                match results.first() {
                    Some(json) => {
                        self.work_cache.insert(pmc_id.to_string(), json.clone());
                    }
                    None => return None,
                }
            }
        }
        Some(pmc_id.to_string())
    }

    pub fn get_cached_publication_from_id(
        &self,
        publication_id: &str,
    ) -> Option<&serde_json::Value> {
        self.work_cache.get(publication_id)
    }

    fn get_pmcid_from_work(&self, json: &serde_json::Value) -> Option<String> {
        json["pmcid"].as_str().map(|pmcid| pmcid.to_string())
    }

    fn get_pmid_from_work(&self, json: &serde_json::Value) -> Option<String> {
        json["pmid"].as_str().map(|pmid| pmid.to_string())
    }

    fn add_identifiers_from_cached_publication(
        &mut self,
        publication_id: &str,
        ret: &mut Vec<GenericWorkIdentifier>,
    ) {
        let work = match self.get_cached_publication_from_id(publication_id) {
            Some(w) => w,
            None => return,
        };

        // PMCID (self)
        if let Some(p) = self.publication_property() {
            if let Some(pmc_id) = self.get_pmcid_from_work(work) {
                if let Some(id) = self.publication_id_for_statement(&pmc_id) {
                    ret.push(GenericWorkIdentifier::new_prop(p, &id));
                }
            }
        };

        // PubMed
        if let Some(pmid) = self.get_pmid_from_work(work) {
            ret.push(GenericWorkIdentifier::new_prop(IdProp::PMID, &pmid));
        }
    }
}

#[async_trait(?Send)]
impl ScientificPublicationAdapter for PMC2Wikidata {
    fn name(&self) -> &str {
        "PMC2Wikidata"
    }

    fn author_cache(&self) -> &HashMap<String, String> {
        &self.author_cache
    }

    fn author_cache_mut(&mut self) -> &mut HashMap<String, String> {
        &mut self.author_cache
    }

    fn publication_property(&self) -> Option<IdProp> {
        Some(IdProp::PMCID)
    }

    fn publication_id_for_statement(&self, id: &str) -> Option<String> {
        if self.is_pmcid(id) {
            Some(id.replace("PMC", "").to_string())
        } else {
            None
        }
    }

    // Overriding default function
    fn update_work_item_with_property(&self, publication_id: &str, item: &mut Entity) {
        if publication_id[0..4].to_string() == "PMID_" {
            return;
        }
        if let Some(prop) = self.publication_property() {
            if !item.has_claims_with_property(prop.as_str()) {
                if let Some(id) = self.publication_id_for_statement(publication_id) {
                    item.add_claim(Statement::new_normal(
                        Snak::new_external_id(prop.as_str(), &id),
                        vec![],
                        self.reference(),
                    ))
                }
            }
        }
    }

    async fn publication_id_from_item(&mut self, item: &Entity) -> Option<String> {
        let pmcid =
            match self.get_external_identifier_from_item(item, &self.publication_property()?) {
                Some(s) => "PMC".to_owned() + &s,
                None => {
                    // Attempt fallback to PubMed ID
                    return match self.get_external_identifier_from_item(item, &IdProp::PMID) {
                        Some(pmid) => self.publication_id_from_pubmed(&pmid),
                        None => None,
                    };
                }
            };
        self.publication_id_from_pmcid(&pmcid)
    }

    fn get_work_titles(&self, publication_id: &str) -> Vec<LocaleString> {
        match self.get_cached_publication_from_id(publication_id) {
            Some(json) => match json["title"].as_str() {
                Some(title) => vec![LocaleString::new("en", title)],
                None => vec![],
            },
            None => vec![],
        }
    }

    fn get_volume(&self, publication_id: &str) -> Option<String> {
        self.get_cached_publication_from_id(publication_id)?["journalInfo"]["volume"]
            .as_str()
            .map(|s| s.to_string())
    }

    fn get_issue(&self, publication_id: &str) -> Option<String> {
        self.get_cached_publication_from_id(publication_id)?["journalInfo"]["issue"]
            .as_str()
            .map(|s| s.to_string())
    }

    fn get_work_issn(&self, publication_id: &str) -> Option<String> {
        self.get_cached_publication_from_id(publication_id)?["journalInfo"]["journal"]["issn"]
            .as_str()
            .map(|s| s.to_string())
    }

    fn get_publication_date(&self, publication_id: &str) -> Option<(u32, Option<u8>, Option<u8>)> {
        let year = match self.get_cached_publication_from_id(publication_id)?["journalInfo"]
            ["yearOfPublication"]
            .as_u64()
        {
            Some(year) => year as u32,
            None => return None,
        };
        Some((
            year,
            self.get_cached_publication_from_id(publication_id)?["journalInfo"]
                ["monthOfPublication"]
                .as_u64()
                .map(|x| x as u8),
            self.get_cached_publication_from_id(publication_id)?["journalInfo"]["dayOfPublication"]
                .as_u64()
                .map(|x| x as u8),
        ))
    }

    async fn get_language_item(&self, publication_id: &str) -> Option<String> {
        self.language2q(self.get_cached_publication_from_id(publication_id)?["language"].as_str()?)
            .await
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
                        if let Some(publication_id) = self.publication_id_from_pubmed(id.id()) {
                            self.add_identifiers_from_cached_publication(&publication_id, &mut ret);
                        }
                    }
                    IdProp::PMCID => {
                        if let Some(publication_id) = self.publication_id_from_pmcid(id.id()) {
                            self.add_identifiers_from_cached_publication(&publication_id, &mut ret);
                        }
                    }
                    _ => {}
                }
            }
        }
        ret
    }

    async fn update_statements_for_publication_id(&self, publication_id: &str, _item: &mut Entity) {
        let _work = match self.get_cached_publication_from_id(publication_id) {
            Some(w) => w,
            None => return,
        };
    }

    // Not sure what this does
    async fn do_cache_work(&mut self, _publication_id: &str) -> Option<String> {
        None
    }

    fn get_author_list(&mut self, publication_id: &str) -> Vec<GenericAuthorInfo> {
        match self.get_cached_publication_from_id(publication_id) {
            Some(work) => match work["authorList"]["author"].as_array() {
                Some(authors) => authors
                    .iter()
                    .filter_map(|author| author["fullName"].as_str())
                    .enumerate()
                    .map(|(num, name)| GenericAuthorInfo::new_from_name_num(name, num + 1))
                    .collect(),
                None => vec![],
            },
            None => vec![],
        }
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
    */
}
