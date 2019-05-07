//extern crate chrono;
extern crate config;
extern crate mediawiki;
extern crate serde_json;

//use crate::AuthorItemInfo;
//use chrono::prelude::*;
//use wikibase::*;
use crate::*;
use pubmed::*;
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct Pubmed2Wikidata {
    author_cache: HashMap<String, String>,
    work_cache: HashMap<String, PubmedArticle>,
    client: Client,
}

impl Pubmed2Wikidata {
    pub fn new() -> Self {
        Pubmed2Wikidata {
            author_cache: HashMap::new(),
            work_cache: HashMap::new(),
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
        Some(ret)
    }
}

impl ScientificPublicationAdapter for Pubmed2Wikidata {
    fn name(&self) -> &str {
        "Pubmed2Wikidata"
    }

    fn author_property(&self) -> Option<String> {
        return Some("P496".to_string());
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
        // Check PubMed ID
        match self.get_external_identifier_from_item(item, &self.publication_property().unwrap()) {
            Some(publication_id) => {
                let pub_id_u64 = publication_id.parse::<u64>().unwrap();
                let work = self.client.article(pub_id_u64).unwrap();
                self.work_cache.insert(publication_id.clone(), work);
                return Some(publication_id);
            }
            None => {}
        };
        // TODO other ID types than DOI?
        let doi = match self.get_external_identifier_from_item(item, "P356") {
            Some(s) => s,
            None => return None,
        };
        let query = "".to_string() + &doi + "";
        let work_ids = match self.client.article_ids_from_query(&query, 10) {
            Ok(work_ids) => work_ids,
            _ => return None, // No such work
        };
        if work_ids.len() != 1 {
            return None;
        }
        let publication_id = work_ids[0];
        let work = self.client.article(publication_id).unwrap();

        self.work_cache.insert(publication_id.to_string(), work);
        Some(publication_id.to_string())
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

    fn get_author_list(&self, publication_id: &String) -> Vec<GenericAuthorInfo> {
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
                            prop2id.insert("P496".to_string(), id.to_string());
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
            });
            list_num = list_num + 1;
        }

        ret
    }

    fn get_identifier_list(&self, publication_id: &String) -> Vec<GenericWorkIdentifier> {
        let mut ret: Vec<GenericWorkIdentifier> = vec![];

        let work = match self.get_cached_publication_from_id(publication_id) {
            Some(w) => w,
            None => return ret,
        };

        ret.push(GenericWorkIdentifier {
            catalog_type: GenericWorkType::Property(self.publication_property().unwrap()),
            catalog_id: publication_id.clone(),
        });

        let medline_citation = match &work.medline_citation {
            Some(x) => x,
            None => return ret,
        };
        let article = match &medline_citation.article {
            Some(x) => x,
            None => return ret,
        };

        for elid in &article.e_location_ids {
            if !elid.valid {
                continue;
            }
            match (&elid.e_id_type, &elid.id) {
                (Some(id_type), Some(id)) => match id_type.as_str() {
                    "doi" => ret.push(GenericWorkIdentifier {
                        catalog_type: GenericWorkType::Property("P356".to_string()),
                        catalog_id: id.clone(),
                    }),
                    _ => {}
                },
                _ => continue,
            }
        }

        ret
    }

    fn update_statements_for_publication_id(&self, publication_id: &String, _item: &mut Entity) {
        let _work = match self.get_cached_publication_from_id(publication_id) {
            Some(w) => w,
            None => return,
        };
    }
    /*
        fn author2item(
            &mut self,
            author_name: &String,
            mw_api: &mut mediawiki::api::Api,
            publication_id: Option<&String>,
            item: Option<&mut Entity>,
        ) -> AuthorItemInfo {
    */
}
