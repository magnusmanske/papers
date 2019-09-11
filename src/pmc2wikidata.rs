extern crate lazy_static;

use crate::generic_author_info::GenericAuthorInfo;
use crate::scientific_publication_adapter::ScientificPublicationAdapter;
use crate::*;
use reqwest;
use std::collections::HashMap;
//use mediawiki::api::Api;

/*
Examples:
https://www.ebi.ac.uk/europepmc/webservices/rest/search?query=EXT_ID:13777676%20AND%20SRC:MED&resulttype=core&format=json (no PMCID)
https://www.ebi.ac.uk/europepmc/webservices/rest/search?query=EXT_ID:17246615%20AND%20SRC:MED&resulttype=core&format=json (with PMCID)
https://www.ebi.ac.uk/europepmc/webservices/rest/search?query=PMC1201091&resulttype=core&format=json (same as line above)
*/

#[derive(Debug, Clone)]
pub struct PMC2Wikidata {
    author_cache: HashMap<String, String>,
    work_cache: HashMap<String, serde_json::Value>,
    //query_cache: HashMap<String, Vec<u64>>,
    //client: Client,
}

impl PMC2Wikidata {
    pub fn new() -> Self {
        Self {
            author_cache: HashMap::new(),
            work_cache: HashMap::new(),
        }
    }

    fn publication_id_from_pubmed(&mut self, pubmed_id: &String) -> Option<String> {
        let pub_id_u64 = pubmed_id.parse::<u64>().ok()?;
        let pubmed_id = format!("PMID_{}", pubmed_id);
        let mut publication_id = pubmed_id.to_string(); // Fallback
        if !self.work_cache.contains_key(&pubmed_id) {
            // TODO same IDs as pubmed?
            let url = format!("https://www.ebi.ac.uk/europepmc/webservices/rest/search?query=EXT_ID:{}%20AND%20SRC:MED&resulttype=core&format=json",pub_id_u64) ;
            let json: serde_json::Value = reqwest::get(url.as_str()).ok()?.json().ok()?;
            let results = json["resultList"]["result"].as_array()?;
            if results.len() == 1 {
                match results.get(0) {
                    Some(json) => {
                        match json["pmcid"].as_str() {
                            Some(pmcid) => publication_id = pmcid.to_string()[3..].to_string(),
                            None => {}
                        }
                        self.work_cache.insert(publication_id.clone(), json.clone());
                    }
                    None => return None,
                }
            }
        }
        Some(publication_id)
    }

    fn publication_id_from_pmcid(&mut self, pmc_id: &String) -> Option<String> {
        let pmc_id = pmc_id.replace("PMC", "");
        if !self.work_cache.contains_key(&pmc_id) {
            // TODO same IDs as pubmed?
            let url = format!("https://www.ebi.ac.uk/europepmc/webservices/rest/search?query=PMC{}&resulttype=core&format=json",&pmc_id) ;
            let json: serde_json::Value = reqwest::get(url.as_str()).ok()?.json().ok()?;
            let results = json["resultList"]["result"].as_array()?;
            if results.len() == 1 {
                match results.get(0) {
                    Some(json) => {
                        self.work_cache.insert(pmc_id.clone(), json.clone());
                    }
                    None => return None,
                }
            }
        }
        Some(pmc_id)
    }

    pub fn get_cached_publication_from_id(
        &self,
        publication_id: &String,
    ) -> Option<&serde_json::Value> {
        self.work_cache.get(publication_id)
    }

    fn get_pmcid_from_work(&self, json: &serde_json::Value) -> Option<String> {
        json["pmcid"]
            .as_str()
            .map(|pmcid| pmcid.to_string()[3..].to_string())
    }

    fn get_pmid_from_work(&self, json: &serde_json::Value) -> Option<String> {
        json["pmid"].as_str().map(|pmid| pmid.to_string())
    }

    fn add_identifiers_from_cached_publication(
        &mut self,
        publication_id: &String,
        ret: &mut Vec<GenericWorkIdentifier>,
    ) {
        let work = match self.get_cached_publication_from_id(&publication_id) {
            Some(w) => w,
            None => return,
        };

        // PMCID (self)
        match self.publication_property() {
            Some(p) => {
                let my_prop = GenericWorkType::Property(p);
                match self.get_pmcid_from_work(work) {
                    Some(pmcid) => {
                        ret.push(GenericWorkIdentifier {
                            work_type: my_prop,
                            id: pmcid,
                        });
                    }
                    None => {}
                }
            }
            None => {}
        };

        // PubMed
        match self.get_pmid_from_work(work) {
            Some(pmid) => {
                ret.push(GenericWorkIdentifier {
                    work_type: GenericWorkType::Property("P698".to_string()),
                    id: pmid.clone(),
                });
            }
            None => {}
        }
    }
}

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

    fn publication_property(&self) -> Option<String> {
        Some("P932".to_string())
    }

    fn publication_id_from_item(&mut self, item: &Entity) -> Option<String> {
        let pmcid = match self
            .get_external_identifier_from_item(item, &self.publication_property().unwrap())
        {
            Some(s) => s,
            None => {
                // Attempt fallback to PubMed ID
                return match self.get_external_identifier_from_item(item, "P698") {
                    Some(pmid) => self.publication_id_from_pubmed(&pmid),
                    None => None,
                };
            }
        };
        self.publication_id_from_pmcid(&pmcid)
    }

    fn get_work_titles(&self, publication_id: &String) -> Vec<LocaleString> {
        match self.get_cached_publication_from_id(publication_id) {
            Some(json) => match json["title"].as_str() {
                Some(title) => vec![LocaleString::new("en", &title.to_string())],
                None => vec![],
            },
            None => vec![],
        }
    }

    fn get_volume(&self, publication_id: &String) -> Option<String> {
        self.get_cached_publication_from_id(publication_id)?["journalInfo"]["volume"]
            .as_str()
            .map(|s| s.to_string())
    }

    fn get_issue(&self, publication_id: &String) -> Option<String> {
        self.get_cached_publication_from_id(publication_id)?["journalInfo"]["issue"]
            .as_str()
            .map(|s| s.to_string())
    }

    fn get_work_issn(&self, publication_id: &String) -> Option<String> {
        self.get_cached_publication_from_id(publication_id)?["journalInfo"]["journal"]["issn"]
            .as_str()
            .map(|s| s.to_string())
    }

    fn get_publication_date(
        &self,
        publication_id: &String,
    ) -> Option<(u32, Option<u8>, Option<u8>)> {
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

    fn get_language_item(&self, publication_id: &String) -> Option<String> {
        self.language2q(self.get_cached_publication_from_id(publication_id)?["language"].as_str()?)
    }

    fn get_identifier_list(
        &mut self,
        ids: &Vec<GenericWorkIdentifier>,
    ) -> Vec<GenericWorkIdentifier> {
        let mut ret: Vec<GenericWorkIdentifier> = vec![];
        for id in ids {
            match &id.work_type {
                GenericWorkType::Property(prop) => match prop.as_str() {
                    PROP_PMID => match self.publication_id_from_pubmed(&id.id) {
                        Some(publication_id) => {
                            self.add_identifiers_from_cached_publication(&publication_id, &mut ret);
                        }
                        None => {}
                    },
                    PROP_PMCID => {
                        for publication_id in self.publication_id_from_pmcid(&id.id) {
                            self.add_identifiers_from_cached_publication(&publication_id, &mut ret);
                        }
                    }
                    _ => {}
                },
                _ => {}
            }
        }
        ret
    }

    fn update_statements_for_publication_id(&self, _publication_id: &String, _item: &mut Entity) {
        /*
        let work = match self.get_cached_publication_from_id(publication_id) {
            Some(w) => w,
            None => return,
        };

        // Work language
        if !item.has_claims_with_property("P407") {
            match &work.medline_citation {
                Some(medline_citation) => match &medline_citation.article {
                    Some(article) => match &article.language {
                        Some(language) => match self.language2q(&language) {
                            Some(q) => {
                                let statement = Statement::new_normal(
                                    Snak::new_item("P407", &q),
                                    vec![],
                                    vec![],
                                );
                                item.add_claim(statement);
                            }
                            None => {}
                        },
                        None => {}
                    },
                    None => {}
                },
                None => {}
            }
        }

        // Publication date
        if !item.has_claims_with_property("P577") {
            match &work.medline_citation {
                Some(medline_citation) => match &medline_citation.article {
                    Some(article) => match &article.journal {
                        Some(journal) => match &journal.journal_issue {
                            Some(journal_issue) => match &journal_issue.pub_date {
                                Some(pub_date) => {
                                    let month = match pub_date.month {
                                        0 => None,
                                        x => Some(x),
                                    };
                                    let day = match pub_date.day {
                                        0 => None,
                                        x => Some(x),
                                    };
                                    let statement = self.get_wb_time_from_partial(
                                        "P577".to_string(),
                                        pub_date.year as u32,
                                        month,
                                        day,
                                    );
                                    item.add_claim(statement);
                                }
                                None => {}
                            },
                            None => {}
                        },
                        None => {}
                    },
                    None => {}
                },
                None => {}
            };
        }

        */
    }

    // Not sure what this does
    fn do_cache_work(&mut self, _publication_id: &String) -> Option<String> {
        None
    }

    fn get_author_list(&mut self, publication_id: &String) -> Vec<GenericAuthorInfo> {
        match self.get_cached_publication_from_id(publication_id) {
            Some(work) => match work["authorList"]["author"].as_array() {
                Some(authors) => authors
                    .iter()
                    .filter_map(|author| match author["fullName"].as_str() {
                        Some(name) => Some(name),
                        None => None,
                    })
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
    //use mediawiki::api::Api;

    /*
    TODO:
    pub fn new() -> Self {
    pub fn get_cached_publication_from_id(
    fn get_author_name_string(&self, author: &Author) -> Option<String> {
    */
}
