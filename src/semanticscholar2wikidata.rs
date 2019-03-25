extern crate config;
extern crate mediawiki;
extern crate serde_json;

use crate::semanticscholar::*;
use crate::ScientificPublicationAdapter;
use std::collections::HashMap;
use wikibase::*;

pub struct Semanticscholar2Wikidata {
    author_cache: HashMap<String, String>,
    work_cache: HashMap<String, crate::semanticscholar::Work>,
    client: crate::semanticscholar::Client,
}

impl Semanticscholar2Wikidata {
    pub fn new() -> Self {
        Semanticscholar2Wikidata {
            author_cache: HashMap::new(),
            work_cache: HashMap::new(),
            client: crate::semanticscholar::Client::new(),
        }
    }

    pub fn get_publication_from_id(
        &self,
        publication_id: &String,
    ) -> Option<&crate::semanticscholar::Work> {
        self.work_cache.get(publication_id)
    }

    fn update_author_item(&mut self, author: &Author, author_name: &String, item: &mut Entity) {
        let semanticscholar_author_name: String = author.name.clone().unwrap();
        let author_id = author.author_id.clone().unwrap();
        item.set_label(LocaleString::new("en", &semanticscholar_author_name));
        if semanticscholar_author_name != *author_name {
            item.add_alias(LocaleString::new("en", &author_name));
        }

        item.add_claim(Statement::new(
            "statement",
            StatementRank::Normal,
            Snak::new(
                "wikibase-item",
                "P31",
                SnakType::Value,
                Some(DataValue::new(
                    DataValueType::EntityId,
                    Value::Entity(EntityValue::new(EntityType::Item, "Q5")),
                )),
            ),
            vec![],
            vec![],
        ));
        item.add_claim(Statement::new(
            "statement",
            StatementRank::Normal,
            Snak::new(
                "external-id",
                &self.author_property().unwrap(),
                SnakType::Value,
                Some(DataValue::new(
                    DataValueType::StringType,
                    Value::StringValue(author_id.clone()),
                )),
            ),
            vec![],
            vec![],
        ));
    }
}

impl ScientificPublicationAdapter for Semanticscholar2Wikidata {
    fn author_property(&self) -> Option<String> {
        return Some("P4012".to_string());
    }
    fn publication_property(&self) -> Option<String> {
        return Some("P4011".to_string());
    }

    fn topic_property(&self) -> Option<String> {
        return Some("P6611".to_string());
    }

    fn author_cache(&self) -> &HashMap<String, String> {
        &self.author_cache
    }

    fn author_cache_mut(&mut self) -> &mut HashMap<String, String> {
        &mut self.author_cache
    }

    fn publication_id_from_item(&mut self, item: &Entity) -> Option<String> {
        // TODO other ID types than DOI?
        let mut doi: Option<String> = None;
        for claim in item.claims() {
            if claim.main_snak().property() == "P356"
                && claim.main_snak().snak_type().to_owned() == SnakType::Value
            {
                match claim.main_snak().data_value() {
                    Some(dv) => {
                        let value = dv.value().clone();
                        match &value {
                            Value::StringValue(s) => doi = Some(s.to_string().to_lowercase()),
                            _ => continue,
                        }
                    }
                    None => continue,
                }
                break;
            }
        }
        let doi = match doi {
            Some(s) => s,
            None => return None,
        };
        let work = match self.client.work(&doi) {
            Ok(w) => w,
            _ => return None, // No such work
        };

        let publication_id = match &work.paper_id {
            Some(paper_id) => paper_id.to_string(),
            None => return None, // No ID
        };

        self.work_cache.insert(publication_id.clone(), work);
        Some(publication_id)
    }

    fn update_statements_for_publication_id(&self, publication_id: &String, item: &mut Entity) {
        let _work = match self.get_publication_from_id(publication_id) {
            Some(w) => w,
            None => return,
        };

        // SS paper ID
        if !item.has_claims_with_property(self.publication_property().unwrap()) {
            item.add_claim(Statement::new(
                "statement",
                StatementRank::Normal,
                Snak::new(
                    "external-id",
                    &self.publication_property().unwrap(),
                    SnakType::Value,
                    Some(DataValue::new(
                        DataValueType::StringType,
                        Value::StringValue(publication_id.clone()),
                    )),
                ),
                vec![],
                vec![],
            ));
        }
    }

    fn author2item(
        &mut self,
        author_name: &String,
        mw_api: &mut mediawiki::api::Api,
        publication_id: Option<&String>,
        item: Option<&mut Entity>,
    ) -> Option<String> {
        // RETURNS WIKIDATA ITEM ID, CATALOG AUHTOR ID, OR None, DEPENDING ON CONTEXT
        let work: Work;
        match publication_id {
            Some(id) => {
                let publication_id_option = self.get_publication_from_id(id).to_owned();
                work = match publication_id_option {
                    Some(w) => w.clone(),
                    None => return None,
                };
            }
            None => return None,
        }

        let mut candidates: Vec<usize> = vec![];
        for num in 0..work.authors.len() {
            let author = &work.authors[num];
            if None == author.author_id {
                continue;
            }
            let current_author_name = match &author.name {
                Some(s) => s,
                _ => continue,
            };
            if self.author_names_match(&author_name, &current_author_name) {
                candidates.push(num);
            }
        }
        if candidates.len() != 1 {
            return None;
        }
        let author = &work.authors[candidates[0]];
        let author_id = author.author_id.clone().unwrap();
        match item {
            None => self.get_author_item_id(&author_id, mw_api), // RETURNS ITEM ID
            Some(item) => {
                self.update_author_item(&author, author_name, item);
                Some(author_id) // RETURNS AUTHOR ID
            }
        }
    }
}
