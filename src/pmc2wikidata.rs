use crate::generic_author_info::GenericAuthorInfo;
use crate::scientific_publication_adapter::ScientificPublicationAdapter;
use crate::*;
use async_trait::async_trait;
use regex::Regex;
use reqwest;
use std::collections::HashMap;

use self::identifiers::{is_pubmed_id, GenericWorkIdentifier, GenericWorkType, IdProp};
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

    async fn publication_id_from_pubmed(&mut self, pubmed_id: &str) -> Option<String> {
        if !is_pubmed_id(pubmed_id) {
            return None;
        }
        let mut publication_id = pubmed_id.to_string(); // Fallback
        if !self.work_cache.contains_key(pubmed_id) {
            let url = format!("https://www.ebi.ac.uk/europepmc/webservices/rest/search?query=EXT_ID:{}%20AND%20SRC:MED&resulttype=core&format=json",pubmed_id) ;
            let json: serde_json::Value =
                reqwest::get(url.as_str()).await.ok()?.json().await.ok()?;
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

    async fn publication_id_from_pmcid(&mut self, pmc_id: &str) -> Option<String> {
        if !self.is_pmcid(pmc_id) {
            return None;
        }
        if !self.work_cache.contains_key(pmc_id) {
            let url = format!("https://www.ebi.ac.uk/europepmc/webservices/rest/search?query={}&resulttype=core&format=json",pmc_id) ;
            let json: serde_json::Value =
                reqwest::get(url.as_str()).await.ok()?.json().await.ok()?;
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
                        Some(pmid) => self.publication_id_from_pubmed(&pmid).await,
                        None => None,
                    };
                }
            };
        self.publication_id_from_pmcid(&pmcid).await
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
        let journal_info = &self.get_cached_publication_from_id(publication_id)?["journalInfo"];
        let year = journal_info["yearOfPublication"].as_u64()? as u32;
        Some((
            year,
            journal_info["monthOfPublication"].as_u64().map(|x| x as u8),
            journal_info["dayOfPublication"].as_u64().map(|x| x as u8),
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
                        if let Some(publication_id) = self.publication_id_from_pubmed(id.id()).await
                        {
                            self.add_identifiers_from_cached_publication(&publication_id, &mut ret);
                        }
                    }
                    IdProp::PMCID => {
                        if let Some(publication_id) = self.publication_id_from_pmcid(id.id()).await
                        {
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

    async fn get_author_list(&mut self, publication_id: &str) -> Vec<GenericAuthorInfo> {
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
    use super::*;
    use serde_json::json;

    fn make_pmc(id: &str, work: serde_json::Value) -> PMC2Wikidata {
        let mut pmc = PMC2Wikidata::new();
        pmc.work_cache.insert(id.to_string(), work);
        pmc
    }

    // === is_pubmed_id ===

    #[test]
    fn is_pubmed_id_accepts_digits() {
        assert!(is_pubmed_id("12345"));
        assert!(is_pubmed_id("1"));
    }

    #[test]
    fn is_pubmed_id_rejects_non_digits() {
        assert!(!is_pubmed_id("PMC123"));
        assert!(!is_pubmed_id("abc"));
        assert!(!is_pubmed_id(""));
        assert!(!is_pubmed_id("12 34"));
        assert!(!is_pubmed_id("12.34"));
    }

    // === is_pmcid ===

    #[test]
    fn is_pmcid_accepts_pmc_prefix_with_digits() {
        let pmc = PMC2Wikidata::new();
        assert!(pmc.is_pmcid("PMC12345"));
        assert!(pmc.is_pmcid("PMC1"));
    }

    #[test]
    fn is_pmcid_rejects_invalid_formats() {
        let pmc = PMC2Wikidata::new();
        assert!(!pmc.is_pmcid("12345"));
        assert!(!pmc.is_pmcid("pmc123")); // lowercase
        assert!(!pmc.is_pmcid("PMC"));    // no digits
        assert!(!pmc.is_pmcid(""));
        assert!(!pmc.is_pmcid("PMC12 34"));
    }

    // === publication_id_for_statement ===

    #[test]
    fn publication_id_for_statement_strips_pmc_prefix() {
        let pmc = PMC2Wikidata::new();
        assert_eq!(
            pmc.publication_id_for_statement("PMC12345"),
            Some("12345".to_string())
        );
    }

    #[test]
    fn publication_id_for_statement_returns_none_for_non_pmcid() {
        let pmc = PMC2Wikidata::new();
        assert_eq!(pmc.publication_id_for_statement("12345"), None);
        assert_eq!(pmc.publication_id_for_statement("pmc123"), None);
        assert_eq!(pmc.publication_id_for_statement(""), None);
    }

    // === get_pmcid_from_work / get_pmid_from_work ===

    #[test]
    fn get_pmcid_from_work_extracts_pmcid() {
        let pmc = PMC2Wikidata::new();
        let work = json!({"pmcid": "PMC12345"});
        assert_eq!(pmc.get_pmcid_from_work(&work), Some("PMC12345".to_string()));
    }

    #[test]
    fn get_pmcid_from_work_returns_none_when_missing() {
        let pmc = PMC2Wikidata::new();
        let work = json!({"pmid": "99999"});
        assert_eq!(pmc.get_pmcid_from_work(&work), None);
    }

    #[test]
    fn get_pmid_from_work_extracts_pmid() {
        let pmc = PMC2Wikidata::new();
        let work = json!({"pmid": "12345"});
        assert_eq!(pmc.get_pmid_from_work(&work), Some("12345".to_string()));
    }

    #[test]
    fn get_pmid_from_work_returns_none_when_missing() {
        let pmc = PMC2Wikidata::new();
        let work = json!({"pmcid": "PMC99"});
        assert_eq!(pmc.get_pmid_from_work(&work), None);
    }

    // === get_work_titles ===

    #[test]
    fn get_work_titles_returns_english_title() {
        let pmc = make_pmc("PMC1", json!({"title": "A great paper"}));
        let titles = pmc.get_work_titles("PMC1");
        assert_eq!(titles.len(), 1);
        assert_eq!(titles[0].value(), "A great paper");
        assert_eq!(titles[0].language(), "en");
    }

    #[test]
    fn get_work_titles_returns_empty_when_no_title() {
        let pmc = make_pmc("PMC1", json!({}));
        assert!(pmc.get_work_titles("PMC1").is_empty());
    }

    #[test]
    fn get_work_titles_returns_empty_for_missing_publication() {
        let pmc = PMC2Wikidata::new();
        assert!(pmc.get_work_titles("PMC999").is_empty());
    }

    // === get_volume / get_issue / get_work_issn ===

    #[test]
    fn get_volume_extracts_from_journal_info() {
        let pmc = make_pmc(
            "PMC1",
            json!({"journalInfo": {"volume": "12"}}),
        );
        assert_eq!(pmc.get_volume("PMC1"), Some("12".to_string()));
    }

    #[test]
    fn get_volume_returns_none_when_missing() {
        let pmc = make_pmc("PMC1", json!({"journalInfo": {}}));
        assert_eq!(pmc.get_volume("PMC1"), None);
    }

    #[test]
    fn get_issue_extracts_from_journal_info() {
        let pmc = make_pmc(
            "PMC1",
            json!({"journalInfo": {"issue": "3"}}),
        );
        assert_eq!(pmc.get_issue("PMC1"), Some("3".to_string()));
    }

    #[test]
    fn get_work_issn_extracts_from_journal_info() {
        let pmc = make_pmc(
            "PMC1",
            json!({"journalInfo": {"journal": {"issn": "1234-5678"}}}),
        );
        assert_eq!(pmc.get_work_issn("PMC1"), Some("1234-5678".to_string()));
    }

    #[test]
    fn get_work_issn_returns_none_when_missing() {
        let pmc = make_pmc("PMC1", json!({"journalInfo": {"journal": {}}}));
        assert_eq!(pmc.get_work_issn("PMC1"), None);
    }

    // === get_publication_date ===

    #[test]
    fn get_publication_date_year_only() {
        let pmc = make_pmc(
            "PMC1",
            json!({"journalInfo": {"yearOfPublication": 2021}}),
        );
        assert_eq!(pmc.get_publication_date("PMC1"), Some((2021, None, None)));
    }

    #[test]
    fn get_publication_date_year_and_month() {
        let pmc = make_pmc(
            "PMC1",
            json!({"journalInfo": {"yearOfPublication": 2021, "monthOfPublication": 6}}),
        );
        assert_eq!(
            pmc.get_publication_date("PMC1"),
            Some((2021, Some(6), None))
        );
    }

    #[test]
    fn get_publication_date_full_date() {
        let pmc = make_pmc(
            "PMC1",
            json!({"journalInfo": {
                "yearOfPublication": 2021,
                "monthOfPublication": 3,
                "dayOfPublication": 15
            }}),
        );
        assert_eq!(
            pmc.get_publication_date("PMC1"),
            Some((2021, Some(3), Some(15)))
        );
    }

    #[test]
    fn get_publication_date_returns_none_without_year() {
        let pmc = make_pmc("PMC1", json!({"journalInfo": {}}));
        assert_eq!(pmc.get_publication_date("PMC1"), None);
    }

    #[test]
    fn get_publication_date_returns_none_for_missing_publication() {
        let pmc = PMC2Wikidata::new();
        assert_eq!(pmc.get_publication_date("PMC999"), None);
    }

    // === name / publication_property ===

    #[test]
    fn name_returns_expected_string() {
        let pmc = PMC2Wikidata::new();
        assert_eq!(pmc.name(), "PMC2Wikidata");
    }

    #[test]
    fn publication_property_returns_pmcid() {
        let pmc = PMC2Wikidata::new();
        assert_eq!(pmc.publication_property(), Some(IdProp::PMCID));
    }
}
