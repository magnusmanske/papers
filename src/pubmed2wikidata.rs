extern crate lazy_static;

use crate::generic_author_info::GenericAuthorInfo;
use crate::scientific_publication_adapter::ScientificPublicationAdapter;
use crate::*;
use mediawiki::api::Api;
use pubmed::*;
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct Pubmed2Wikidata {
    author_cache: HashMap<String, String>,
    work_cache: HashMap<String, PubmedArticle>,
    query_cache: HashMap<String, Vec<u64>>,
    client: Client,
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

    pub fn get_cached_publication_from_id(
        &self,
        publication_id: &String,
    ) -> Option<&PubmedArticle> {
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

    fn language2q(&self, language: &str) -> Option<String> {
        lazy_static! {
            static ref MW_API: Api = Api::new("https://www.wikidata.org/w/api.php").unwrap();
            static ref L2Q: HashMap<String, String> = MW_API
                .sparql_query("SELECT DISTINCT ?l ?q { ?q wdt:P31/wdt:P279* wd:Q20162172; (wdt:P219|wdt:P220) ?l }")
                .unwrap()["results"]["bindings"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|j| {
                    let l = j["l"]["value"].as_str()?;
                    let q = MW_API
                        .extract_entity_from_uri(j["q"]["value"].as_str()?)
                        .ok()?;
                    Some((l.to_string(), q.to_string()))
                })
                .collect();
        }
        L2Q.get(language).map(|s| s.to_string())
    }

    fn publication_id_from_pubmed(&mut self, publication_id: &String) -> Option<String> {
        if !self.work_cache.contains_key(publication_id) {
            let pub_id_u64 = publication_id.parse::<u64>().ok()?;
            let work = self.client.article(pub_id_u64).ok()?;
            self.work_cache.insert(publication_id.clone(), work);
        }
        return Some(publication_id.to_string());
    }

    fn publication_ids_from_doi(&mut self, doi: &String) -> Vec<String> {
        let query = "".to_string() + &doi + "";
        let work_ids: Vec<u64> = match self.query_cache.get(&query) {
            Some(work_ids) => work_ids.clone(),
            None => match self.client.article_ids_from_query(&query, 10) {
                Ok(work_ids) => work_ids,
                _ => vec![],
            },
        };
        self.query_cache.insert(query, work_ids.clone());
        for publication_id in &work_ids {
            if !self.work_cache.contains_key(&publication_id.to_string()) {
                match self.client.article(*publication_id) {
                    Ok(work) => {
                        self.work_cache.insert(publication_id.to_string(), work);
                    }
                    Err(e) => println!("pubmed::publication_ids_from_doi: {:?}", &e),
                }
            }
        }
        work_ids.iter().map(|s| s.to_string()).collect()
    }

    fn add_identifiers_from_cached_publication(
        &mut self,
        publication_id: &String,
        ret: &mut Vec<GenericWorkIdentifier>,
    ) {
        let my_prop = match self.publication_property() {
            Some(p) => p,
            None => return,
        };
        let my_prop = GenericWorkType::Property(my_prop);

        let work = match self.get_cached_publication_from_id(&publication_id) {
            Some(w) => w,
            None => return,
        };

        ret.push(GenericWorkIdentifier {
            work_type: my_prop.clone(),
            id: publication_id.clone(),
        });

        let medline_citation = match &work.medline_citation {
            Some(x) => x,
            None => return,
        };
        let article = match &medline_citation.article {
            Some(x) => x,
            None => return,
        };

        match &work.pubmed_data {
            Some(pubmed_data) => match &pubmed_data.article_ids {
                Some(article_ids) => {
                    article_ids
                        .ids
                        .iter()
                        .for_each(|id| match (&id.id_type, &id.id) {
                            (Some(key), Some(id)) => match key.as_str() {
                                "doi" => ret.push(GenericWorkIdentifier {
                                    work_type: GenericWorkType::Property("P356".to_string()),
                                    id: id.clone(),
                                }),
                                _ => {}
                            },
                            _ => {}
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
                (Some(id_type), Some(id)) => match id_type.as_str() {
                    "doi" => ret.push(GenericWorkIdentifier {
                        work_type: GenericWorkType::Property("P356".to_string()),
                        id: id.clone(),
                    }),
                    other => {
                        println!(
                            "pubmed2wikidata::get_identifier_list unknown paper ID type '{}'",
                            &other
                        );
                    }
                },
                _ => continue,
            }
        }
    }
}

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

    fn publication_property(&self) -> Option<String> {
        Some("P698".to_string())
    }

    fn publication_id_from_item(&mut self, item: &Entity) -> Option<String> {
        let publication_id = match self
            .get_external_identifier_from_item(item, &self.publication_property().unwrap())
        {
            Some(s) => s,
            None => return None,
        };
        self.publication_id_from_pubmed(&publication_id)
    }

    fn get_work_titles(&self, publication_id: &String) -> Vec<LocaleString> {
        match self.get_cached_publication_from_id(publication_id) {
            Some(work) => match &work.medline_citation {
                Some(citation) => match &citation.article {
                    Some(article) => match &article.title {
                        Some(title) => vec![LocaleString::new("en", &title)],
                        None => vec![],
                    },
                    None => vec![],
                },
                None => vec![],
            },
            None => vec![],
        }
    }

    fn get_work_issn(&self, publication_id: &String) -> Option<String> {
        let work = self
            .get_cached_publication_from_id(publication_id)?
            .to_owned();
        let medline_citation = work.medline_citation?.to_owned();
        let article = medline_citation.article?.to_owned();
        let journal = article.journal?.to_owned();
        let issn = journal.issn?.to_owned();
        Some(issn)
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
                        for publication_id in self.publication_ids_from_doi(&id.id) {
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

    fn update_statements_for_publication_id(&self, publication_id: &String, item: &mut Entity) {
        let work = match self.get_cached_publication_from_id(publication_id) {
            Some(w) => w,
            None => return,
        };

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

        if !item.has_claims_with_property("P478") {
            match &work.medline_citation {
                Some(medline_citation) => match &medline_citation.article {
                    Some(article) => match &article.journal {
                        Some(journal) => match &journal.journal_issue {
                            Some(journal_issue) => match &journal_issue.volume {
                                Some(volume) => {
                                    let statement = Statement::new_normal(
                                        Snak::new_string("P478", volume),
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
                },
                None => {}
            };
        }
    }

    fn do_cache_work(&mut self, publication_id: &String) -> Option<String> {
        let pub_id_u64 = match publication_id.parse::<u64>() {
            Ok(x) => x,
            _ => return None,
        };
        let work = match self.client.article(pub_id_u64) {
            Ok(x) => x,
            _ => return None,
        };
        self.work_cache.insert(publication_id.clone(), work);
        Some(publication_id.to_string())
    }

    fn get_author_list(&mut self, publication_id: &String) -> Vec<GenericAuthorInfo> {
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
                match (&aid.source, &aid.id) {
                    (Some(source), Some(id)) => match source.as_str() {
                        "ORCID" => {
                            // URL => ID
                            match id.split('/').last() {
                                Some(id) => {
                                    prop2id.insert("P496".to_string(), id.to_string());
                                }
                                None => {}
                            }
                        }
                        other => println!("Unknown author source: {}:{}", &other, &id),
                    },
                    _ => {}
                }
            }
            ret.push(GenericAuthorInfo {
                name: self.get_author_name_string(&author),
                prop2id: prop2id,
                wikidata_item: None,
                list_number: Some(list_num.to_string()),
                alternative_names: vec![],
            });
            list_num = list_num + 1;
        }

        ret
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
    fn publication_id_from_pubmed(&mut self, publication_id: &String) -> Option<String> {
    fn publication_ids_from_doi(&mut self, doi: &String) -> Vec<String> {
    fn add_identifiers_from_cached_publication(
    fn name(&self) -> &str {
    fn author_cache(&self) -> &HashMap<String, String> {
    fn author_cache_mut(&mut self) -> &mut HashMap<String, String> {
    fn publication_property(&self) -> Option<String> {
    fn get_work_titles(&self, publication_id: &String) -> Vec<LocaleString> {
    fn get_work_issn(&self, publication_id: &String) -> Option<String> {
    fn get_identifier_list(
    fn update_statements_for_publication_id(&self, publication_id: &String, item: &mut Entity) {
    fn do_cache_work(&mut self, publication_id: &String) -> Option<String> {
    fn get_author_list(&mut self, publication_id: &String) -> Vec<GenericAuthorInfo> {
    */
}
