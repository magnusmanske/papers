extern crate config;
extern crate mediawiki;
extern crate papers;
#[macro_use]
extern crate lazy_static;
extern crate regex;
#[macro_use]
extern crate serde_json;

use config::{Config, File};
use crossref::Crossref;
use mediawiki::entity_diff;
use regex::Regex;
use std::collections::HashMap;
use wikibase::*;

struct WikidataPapers {
    semaniticscholars_author_cache: HashMap<String, String>,
}

impl WikidataPapers {
    pub fn new() -> WikidataPapers {
        WikidataPapers {
            semaniticscholars_author_cache: HashMap::new(),
        }
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

    fn author_names_match(&self, name1: &str, name2: &str) -> bool {
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

    fn get_semanticscholar_author_item_id(
        &mut self,
        ss_author: &papers::semanticscholar::Author,
        mw_api: &mediawiki::api::Api,
    ) -> Option<String> {
        let ss_author_id = ss_author.author_id.clone()?;

        // Load semanticscholars from Wikidata, if not done so already
        if self.semaniticscholars_author_cache.is_empty() {
            let res = mw_api
                .sparql_query("SELECT ?q ?id { ?q wdt:P4012 ?id }")
                .unwrap();
            //println!("{}", ::serde_json::to_string_pretty(&res).unwrap());

            for b in res["results"]["bindings"].as_array().unwrap() {
                match (b["q"]["value"].as_str(), b["id"]["value"].as_str()) {
                    (Some(entity_url), Some(id)) => {
                        let q = mw_api.extract_entity_from_uri(entity_url).unwrap();
                        self.semaniticscholars_author_cache
                            .insert(id.to_string(), q);
                    }
                    _ => {}
                }
            }
        }

        // Check cache
        if self
            .semaniticscholars_author_cache
            .contains_key(&ss_author_id)
        {
            return Some(self.semaniticscholars_author_cache[&ss_author_id].to_string());
        }

        // TODO Paranoia check via search (eg haswbstatement:P4012=059932554) but doesn't work on Wikidata right now
        None
    }

    fn get_or_create_semanticscholar_author_item_id(
        &mut self,
        ss_author: &papers::semanticscholar::Author,
        author_name: &str,
        mw_api: &mut mediawiki::api::Api,
    ) -> Option<String> {
        match self.get_semanticscholar_author_item_id(ss_author, mw_api) {
            Some(author_q) => Some(author_q),
            None => self.create_semanticscholar_author_item_id(ss_author, author_name, mw_api),
        }
    }

    fn create_semanticscholar_author_item_id(
        &mut self,
        ss_author: &papers::semanticscholar::Author,
        author_name: &str,
        mw_api: &mut mediawiki::api::Api,
    ) -> Option<String> {
        let ss_author_name = ss_author.name.clone()?;
        let ss_author_id = ss_author.author_id.clone()?;

        // Create new author item
        let mut new_item = Entity::new_empty();
        new_item.set_label(LocaleString::new("en", &ss_author_name));
        if ss_author_name != *author_name {
            new_item.add_alias(LocaleString::new("en", &author_name));
        }

        new_item.add_claim(Statement::new(
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
        new_item.add_claim(Statement::new(
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
        self.semaniticscholars_author_cache
            .insert(ss_author_id, entity_id.clone());
        println!("=> {}", &entity_id);

        Some(entity_id)
    }

    fn try_wikidata_edit(
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
            self.try_wikidata_edit(mw_api, item, params, num_tries_left - 1)
        } else {
            Err(From::from(format!(
                "Failed to edit with params '{:?}', result:{:?}",
                &params, &res
            )))
        }
    }

    fn check_semanticscholar(
        &mut self,
        ss_client: &papers::semanticscholar::Client,
        item: &mut wikibase::Entity,
        doi: &str,
        mw_api: &mut mediawiki::api::Api,
        q: String,
    ) {
        let ss_work = match ss_client.work(doi) {
            Ok(work) => work,
            _ => return,
        };

        let ss_paper_id = match &ss_work.paper_id {
            Some(paper_id) => paper_id,
            None => return, // No ID
        };

        // SS paper ID
        if !item.has_claims_with_property("P4011") {
            let json = json!({"claims":[{"mainsnak":{"snaktype":"value","property":"P4011","datavalue":{"value":ss_paper_id,"type":"string"}},"type":"statement","rank":"normal"}]});
            let json = json.to_string();
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
        }

        // TODO check no(P50) and no(P2093)

        // SS authors (P50) match

        // SS authors (P2093) match
        for claim in item.claims_with_property("P2093") {
            if claim.claim_type() != "statement" {
                continue;
            }
            let main_snak = claim.main_snak();
            if main_snak.datatype() != "string" {
                continue;
            }
            let datavalue = match main_snak.data_value() {
                Some(dv) => dv,
                None => continue,
            };
            let author_name = match datavalue.value() {
                Value::StringValue(s) => s,
                _ => continue,
            };

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
        }
    }

    pub fn update_dois(&mut self, mw_api: &mut mediawiki::api::Api, dois: &Vec<&str>) {
        let ss_client = papers::semanticscholar::Client::new();
        let mut entities = mediawiki::entity_container::EntityContainer::new();

        for doi in dois {
            let q = match self.get_wikidata_item_for_doi(&mw_api, &doi.to_string()) {
                Some(i) => i,
                None => continue,
            };
            if entities.load_entities(&mw_api, &vec![q.clone()]).is_err() {
                continue;
            }

            let item_opt = entities.get_entity(q.clone());
            let mut item = match item_opt {
                Some(the_item) => the_item.clone(),
                None => continue,
            };
            self.check_semanticscholar(&ss_client, &mut item, doi, mw_api, q);
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
    wdp.update_dois(&mut mw_api, &vec!["10.1038/nrn3241"]);
}
