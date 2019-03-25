extern crate config;
extern crate mediawiki;
extern crate serde_json;

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

    fn _create_author_item(
        &mut self,
        ss_author: &crate::semanticscholar::Author,
        author_name: &str,
    ) -> Option<Entity> {
        let ss_author_name = ss_author.name.clone()?;
        let ss_author_id = ss_author.author_id.clone()?;

        // Create new author item
        let mut item = Entity::new_empty();
        item.set_label(LocaleString::new("en", &ss_author_name));
        if ss_author_name != *author_name {
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
                "string",
                "P4012",
                SnakType::Value,
                Some(DataValue::new(
                    DataValueType::StringType,
                    Value::StringValue(ss_author_id.clone()),
                )),
            ),
            vec![],
            vec![],
        ));
        Some(item)
        /*
                let empty = Entity::new_empty();
                let diff_params = entity_diff::EntityDiffParams::all();
                let diff = entity_diff::EntityDiff::new(&empty, &new_item, &diff_params);
                println!("{:?}\n", &new_item);
                println!("{}\n", diff.as_str().unwrap());

                // Apply diff
                let new_json = entity_diff::EntityDiff::apply_diff(
                    mw_api,
                    &diff,
                    entity_diff::EditTarget::New("item".to_string()),
                )
                .unwrap();
                let entity_id = entity_diff::EntityDiff::get_entity_id(&new_json).unwrap();
                //self.semaniticscholars_author_cache.insert(ss_author_id, entity_id.clone());
                //println!("=> {}", &entity_id);

                Some(entity_id)
        */
    }
}

impl ScientificPublicationAdapter for Semanticscholar2Wikidata {
    fn author_property(&self) -> String {
        return "P4012".to_string();
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
            _ => return,
        };

        // SS paper ID
        if !item.has_claims_with_property("P4011") {
            item.add_claim(Statement::new(
                "statement",
                StatementRank::Normal,
                Snak::new(
                    "string",
                    "P4011",
                    SnakType::Value,
                    Some(DataValue::new(
                        DataValueType::StringType,
                        Value::StringValue(publication_id.clone()),
                    )),
                ),
                vec![],
                vec![],
            ));

            //            let json = json!({"claims":[{"mainsnak":{"snaktype":"value","property":"P4011","datavalue":{"value":ss_paper_id,"type":"string"}},"type":"statement","rank":"normal"}]});
            //            let json = json.to_string();
            /*
                        let token = mw_api.get_edit_token().unwrap();
                        let params: HashMap<_, _> = vec![
                            ("action", "wbeditentity"),
                            ("id", &q),
                            ("data", &json),
                            ("token", &token),
                        ]
                        .into_iter()
                        .collect();
                        dbg!(&params);
                        self.try_wikidata_edit(mw_api, item, &params, 3).unwrap();
            */
        }
    }
}
