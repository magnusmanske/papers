extern crate lazy_static;

use crate::generic_author_info::GenericAuthorInfo;
use crate::scientific_publication_adapter::ScientificPublicationAdapter;
use crate::*;
//use mediawiki::api::Api;
//use pubmed::*;
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct PMC2Wikidata {
    author_cache: HashMap<String, String>,
    //work_cache: HashMap<String, PubmedArticle>,
    //query_cache: HashMap<String, Vec<u64>>,
    //client: Client,
}

impl PMC2Wikidata {
    pub fn new() -> Self {
        Self {
            author_cache: HashMap::new(),
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
        None //Some("P698".to_string()) //??
    }

    fn get_work_titles(&self, _publication_id: &String) -> Vec<LocaleString> {
        vec![]
        /*
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
        */
    }

    fn get_work_issn(&self, _publication_id: &String) -> Option<String> {
        None
        /*
        let work = self
            .get_cached_publication_from_id(publication_id)?
            .to_owned();
        let medline_citation = work.medline_citation?.to_owned();
        let article = medline_citation.article?.to_owned();
        let journal = article.journal?.to_owned();
        let issn = journal.issn?.to_owned();
        Some(issn)
        */
    }

    fn get_identifier_list(
        &mut self,
        _ids: &Vec<GenericWorkIdentifier>,
    ) -> Vec<GenericWorkIdentifier> {
        vec![]
        /*
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
        */
    }

    fn update_statements_for_publication_id(&self, _publication_id: &String, _item: &mut Entity) {
        /*
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
        */
    }

    fn do_cache_work(&mut self, _publication_id: &String) -> Option<String> {
        None
        /*
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
        */
    }

    fn get_author_list(&mut self, _publication_id: &String) -> Vec<GenericAuthorInfo> {
        vec![]
        /*
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
        */
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
