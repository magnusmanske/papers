extern crate config;
extern crate mediawiki;
extern crate papers;
#[macro_use]
extern crate lazy_static;
extern crate regex;
//#[macro_use]
extern crate serde_json;

use config::{Config, File};
use crossref::Crossref;
//use mediawiki::entity_diff;
use regex::Regex;
use std::collections::HashMap;
use wikibase::*;

/*
struct AuthorRepresentation {
    name: String,
    alt_names: Vec<String>,
    extnernal_id: String,
    Property: String,
}
*/

pub trait ScientificPublicationAdapter {
    fn author_property(&self) -> String;
    fn author_cache(&self) -> &HashMap<String, String>;
    fn author_cache_mut(&mut self) -> &mut HashMap<String, String>;
    fn publication_id_from_item(&mut self, item: &Entity) -> Option<String>;
    fn update_statements_for_publication_id(&self, publication_id: &String, item: &mut Entity);

    // Pre-filled methods

    fn set_author_cache_entry(&mut self, catalog_author_id: &String, q: &String) {
        self.author_cache_mut()
            .insert(catalog_author_id.to_string(), q.to_string());
    }

    fn get_author_item_from_cache(&self, catalog_author_id: &String) -> Option<&String> {
        self.author_cache().get(catalog_author_id)
    }

    fn author_cache_is_empty(&self) -> bool {
        self.author_cache().is_empty()
    }

    fn get_author_item_id(
        &mut self,
        catalog_author_id: &String,
        mw_api: &mediawiki::api::Api,
    ) -> Option<String> {
        let author_property = self.author_property();
        // Load all semanticscholar authors from Wikidata, if not done so already
        if self.author_cache_is_empty() {
            let res = mw_api
                .sparql_query(&("SELECT ?q ?id { ?q wdt:".to_owned() + &author_property + " ?id }"))
                .unwrap();
            //println!("{}", ::serde_json::to_string_pretty(&res).unwrap());

            for b in res["results"]["bindings"].as_array().unwrap() {
                match (b["q"]["value"].as_str(), b["id"]["value"].as_str()) {
                    (Some(entity_url), Some(id)) => {
                        let q = mw_api.extract_entity_from_uri(entity_url).unwrap();
                        self.set_author_cache_entry(&id.to_string(), &q);
                    }
                    _ => {}
                }
            }
        }

        // Now check cache
        match self.get_author_item_from_cache(catalog_author_id) {
            Some(q) => return Some(q.to_string()),
            _ => {}
        }

        // Paranoia check via Wikidata search
        let query: String =
            "haswbstatement:".to_owned() + &author_property + &"=".to_owned() + &catalog_author_id;
        let params: HashMap<_, _> = vec![
            ("action", "query"),
            ("list", "search"),
            ("srnamespace", "0"),
            ("srsearch", &query.as_str()),
        ]
        .into_iter()
        .collect();
        let res = mw_api.get_query_api_json(&params).unwrap();
        match res["query"]["search"].as_array() {
            Some(items) => {
                if items.len() > 0 {
                    let author_q = items[0]["title"].as_str()?;
                    self.set_author_cache_entry(&query, &author_q.to_string());
                    return Some(author_q.to_string());
                }
            }
            None => {}
        }

        None
    }
}

struct Semanticscholar2Wikidata {
    author_cache: HashMap<String, String>,
    work_cache: HashMap<String, papers::semanticscholar::Work>,
    client: papers::semanticscholar::Client,
}

impl Semanticscholar2Wikidata {
    pub fn new() -> Self {
        Semanticscholar2Wikidata {
            author_cache: HashMap::new(),
            work_cache: HashMap::new(),
            client: papers::semanticscholar::Client::new(),
        }
    }

    pub fn get_publication_from_id(
        &self,
        publication_id: &String,
    ) -> Option<&papers::semanticscholar::Work> {
        self.work_cache.get(publication_id)
    }

    fn _create_author_item(
        &mut self,
        ss_author: &papers::semanticscholar::Author,
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
                            Value::StringValue(s) => doi = Some(s.to_string()),
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

struct WikidataPapers {
    adapters: Vec<Box<ScientificPublicationAdapter>>,
}

impl WikidataPapers {
    pub fn new() -> WikidataPapers {
        WikidataPapers { adapters: vec![] }
    }

    pub fn adapters_mut(&mut self) -> &mut Vec<Box<ScientificPublicationAdapter>> {
        &mut self.adapters
    }

    pub fn get_wikidata_item_for_doi(
        &self,
        mw_api: &mediawiki::api::Api,
        doi: &String,
    ) -> Option<String> {
        let sparql = format!(
            "SELECT DISTINCT ?q {{ VALUES ?doi {{ '{}' '{}' '{}' }} . ?q wdt:P356 ?doi }}",
            doi,
            doi.to_uppercase(),
            doi.to_lowercase()
        ); // DOIs in Wikidata can be any upper/lowercase :-(
        let res = match mw_api.sparql_query(&sparql) {
            Ok(res) => res,
            _ => return None,
        };
        let qs = mw_api.entities_from_sparql_result(&res, "q");

        match qs.len() {
            0 => None,
            1 => Some(qs[0].clone()),
            _ => {
                println!(
                    "Multiple Wikidata items for DOI '{}' : {}",
                    &doi,
                    qs.join(", ")
                );
                None
            }
        }
    }

    fn _author_names_match(&self, name1: &str, name2: &str) -> bool {
        lazy_static! {
            static ref RE1: Regex = Regex::new(r"\b(\w{3,})\b").unwrap();
        }
        if RE1.is_match(name1) && RE1.is_match(name2) {
            let mut parts1: Vec<String> = vec![];
            for cap in RE1.captures_iter(name1) {
                parts1.push(cap[1].to_string());
            }
            parts1.sort();
            let mut parts2: Vec<String> = vec![];
            for cap in RE1.captures_iter(name2) {
                parts2.push(cap[1].to_string());
            }
            parts2.sort();
            return parts1 == parts2;
        }
        false
    }

    fn _try_wikidata_edit(
        &self,
        mw_api: &mut mediawiki::api::Api,
        item: &mut wikibase::Entity,
        params: &HashMap<&str, &str>,
        num_tries_left: i64,
    ) -> Result<(), Box<::std::error::Error>> {
        let res = mw_api.post_query_api_json(&params).unwrap();

        match res["success"].as_i64() {
            Some(num) => {
                if num == 1 {
                    // Success, now use updated item JSON
                    match &res["entity"] {
                        serde_json::Value::Null => {}
                        entity_json => {
                            //entity_json => entities.set_entity_from_json(&entity_json).unwrap(),
                            let x = from_json::entity_from_json(entity_json).unwrap();
                            *item = x;
                            return Ok(());
                        }
                    };
                }
            }
            None => {}
        }

        if num_tries_left > 0 {
            // TODO sleep 5 sec
            self._try_wikidata_edit(mw_api, item, params, num_tries_left - 1)
        } else {
            Err(From::from(format!(
                "Failed to edit with params '{:?}', result:{:?}",
                &params, &res
            )))
        }
    }

    pub fn update_item_from_adapters(&mut self, mut item: &mut Entity) {
        for adapter in &mut self.adapters {
            let publication_id = match adapter.publication_id_from_item(item) {
                Some(id) => id,
                _ => continue,
            };
            println!(
                "Found publication ID '{}' for item {}",
                &publication_id,
                item.id()
            );
            adapter.update_statements_for_publication_id(&publication_id, &mut item);
        }
    }

    pub fn update_authors_from_adapters(&mut self, item: &mut Entity) {
        // SS authors (P50) match

        // SS authors (P2093) match
        for claim in item.claims_with_property("P2093") {
            if claim.claim_type() != "statement" || claim.main_snak().datatype() != "string" {
                continue;
            }
            let datavalue = match claim.main_snak().data_value() {
                Some(dv) => dv,
                None => continue,
            };
            let _author_name = match datavalue.value() {
                Value::StringValue(s) => s,
                _ => continue,
            };
            /*
                        let mut ss_candidates: Vec<usize> = vec![];
                        for num in 0..ss_work.authors.len() {
                            let ss_author = &ss_work.authors[num];
                            if None == ss_author.author_id {
                                continue;
                            }
                            let ss_author_name = match &ss_author.name {
                                Some(s) => s,
                                _ => continue,
                            };
                            if self.author_names_match(&author_name, &ss_author_name) {
                                ss_candidates.push(num);
                            }
                        }
                        if ss_candidates.len() != 1 {
                            continue;
                        }
                        let ss_author = &ss_work.authors[ss_candidates[0]];
                        let author_q =
                            self.get_or_create_semanticscholar_author_item_id(&ss_author, &author_name, mw_api);
                        match author_q {
                            Some(q) => println!("Found author: https://www.wikidata.org/wiki/{}", &q),
                            None => println!("Found no author '{:?}'", &ss_author),
                        }
            */
        }
    }

    pub fn update_dois(&mut self, mw_api: &mut mediawiki::api::Api, dois: &Vec<&str>) {
        let mut entities = mediawiki::entity_container::EntityContainer::new();

        for doi in dois {
            let mut item;
            let q;
            match self.get_wikidata_item_for_doi(&mw_api, &doi.to_string()) {
                Some(i) => {
                    q = i;
                    if entities.load_entities(&mw_api, &vec![q.clone()]).is_err() {
                        continue;
                    }

                    let item_opt = entities.get_entity(q.clone());
                    item = match item_opt {
                        Some(the_item) => the_item.clone(),
                        None => continue,
                    };
                }
                None => {
                    // TODO create blank item
                    continue;
                }
            };
            self.update_item_from_adapters(&mut item);
            self.update_authors_from_adapters(&mut item);
        }
    }

    pub fn _test_crossref() {
        let client = Crossref::builder().build().unwrap();
        let work = client.work("10.1037/0003-066X.59.1.29").unwrap();
        dbg!(work);
    }
}

fn main() {
    let mut mw_api = mediawiki::api::Api::new("https://www.wikidata.org/w/api.php").unwrap();
    if true {
        let mut settings = Config::default();
        // File::with_name(..) is shorthand for File::from(Path::new(..))
        settings.merge(File::with_name("test.ini")).unwrap();
        let lgname = settings.get_str("user.user").unwrap();
        let lgpass = settings.get_str("user.pass").unwrap();
        mw_api.login(lgname, lgpass).unwrap();
    }

    let mut wdp = WikidataPapers::new();
    wdp.adapters_mut()
        .push(Box::new(Semanticscholar2Wikidata::new()));
    wdp.update_dois(&mut mw_api, &vec!["10.1038/nrn3241"]);
}
