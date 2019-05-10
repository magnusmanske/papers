extern crate config;
extern crate mediawiki;
extern crate regex;
extern crate serde_json;
extern crate wikibase;

use crate::*;
use std::collections::HashMap;
use std::collections::HashSet;
//use crate::AuthorItemInfo;
//use multimap::MultiMap;
//use regex::Regex;
//use wikibase::entity_diff::*;
//use wikibase::*;

pub struct WikidataPapersCache {
    issn2q: HashMap<String, String>,
    is_initialized: bool,
    mw_api: Option<mediawiki::api::Api>,
}

impl WikidataPapersCache {
    pub fn new() -> Self {
        Self {
            issn2q: HashMap::new(),
            is_initialized: false,
            mw_api: None,
        }
    }
    pub fn issn2q(&mut self, issn: &String) -> Option<String> {
        match self.issn2q.get(issn) {
            Some(q) => {
                if q.is_empty() {
                    None
                } else {
                    Some(q.to_string())
                }
            }
            None => match self.search_issn2q(issn) {
                Some(q) => {
                    self.issn2q.insert(issn.to_string(), q.clone());
                    Some(q)
                }
                None => None,
            },
        }
    }

    fn search_issn2q(&mut self, issn: &String) -> Option<String> {
        match self.search_wikibase(&("haswbstatement:P236=".to_string() + issn)) {
            Ok(items) => match items.len() {
                1 => Some(items[0].to_string()),
                _ => None,
            },
            Err(e) => {
                println!("ERROR:{}", e);
                None
            }
        }
    }

    pub fn search_wikibase(&self, query: &String) -> Result<Vec<String>, String> {
        let mw_api = match &self.mw_api {
            Some(x) => x,
            None => return Err("no mw_api set in WikidataPapersCache".to_string()),
        };
        let params: HashMap<_, _> = vec![
            ("action", "query"),
            ("list", "search"),
            ("srnamespace", "0"),
            ("srsearch", &query.as_str()),
        ]
        .into_iter()
        .map(|(x, y)| (x.to_string(), y.to_string()))
        .collect();
        let res = mw_api.get_query_api_json(&params).unwrap();
        match res["query"]["search"].as_array() {
            Some(items) => Ok(items
                .iter()
                .map(|item| item["title"].as_str().unwrap().to_string())
                .collect()),
            None => Ok(vec![]),
        }
    }

    pub fn init(&mut self, mw_api: &mediawiki::api::Api) {
        if self.is_initialized {
            return;
        }

        self.mw_api = Some(mw_api.clone());

        // DEACTIVATE FOR TESTING
        if false {
            self.init_issn_cache(&mw_api);
        }

        self.is_initialized = true;
    }

    /// Loads all ISSNs from Wikidata via SPARQL
    fn init_issn_cache(&mut self, mw_api: &mediawiki::api::Api) {
        match mw_api.sparql_query("SELECT ?q ?issn { ?q wdt:P236 ?issn }") {
            Ok(sparql_result) => {
                for b in sparql_result["results"]["bindings"].as_array().unwrap() {
                    match b["q"]["value"].as_str() {
                        Some(entity_url) => {
                            let q = mw_api.extract_entity_from_uri(entity_url).unwrap();
                            match b["issn"]["value"].as_str() {
                                Some(issn) => {
                                    if self.issn2q.contains_key(issn) {
                                        self.issn2q.insert(issn.to_string(), "".to_string());
                                    } else {
                                        self.issn2q.insert(issn.to_string(), q);
                                    }
                                }
                                None => {}
                            }
                        }
                        None => {}
                    }
                }
            }
            _ => {}
        }
        //println!("ISSN cache size: {}", self.issn2q.len());
    }
}

pub struct WikidataPapers {
    adapters: Vec<Box<ScientificPublicationAdapter>>,
    //caches: WikidataPapersCache,
    //id_cache: HashMap<String, String>,
}

impl WikidataPapers {
    pub fn new() -> WikidataPapers {
        WikidataPapers {
            adapters: vec![],
            //caches: WikidataPapersCache::new(),
            //id_cache: HashMap::new(),
        }
    }

    pub fn adapters_mut(&mut self) -> &mut Vec<Box<ScientificPublicationAdapter>> {
        &mut self.adapters
    }

    pub fn add_adapter(&mut self, adapter_box: Box<ScientificPublicationAdapter>) {
        self.adapters.push(adapter_box);
    }
    /*
        pub fn get_wikidata_items_for_doi(
            &mut self,
            mw_api: &mediawiki::api::Api,
            doi: &String,
        ) -> Vec<String> {
            self.get_wikidata_items_for_property(mw_api, doi, "P356")
        }

        pub fn get_wikidata_items_for_pmid(
            &mut self,
            mw_api: &mediawiki::api::Api,
            pmid: &String,
        ) -> Vec<String> {
            self.get_wikidata_items_for_property(mw_api, pmid, "P698")
        }

        fn fix_paper_id_prefix(&self, s: &String) -> String {
            // Automatically remove leading PMC, if any
            lazy_static! {
                static ref RE1: Regex = Regex::new(r"^[Pp][Mm][Cc]").unwrap();
            }
            RE1.replace(s, "").to_string()
        }

        pub fn get_wikidata_items_for_pmcid(
            &mut self,
            mw_api: &mediawiki::api::Api,
            pmcid: &String,
        ) -> Vec<String> {
            let pmcid = self.fix_paper_id_prefix(&pmcid);
            self.get_wikidata_items_for_property(mw_api, &pmcid, "P932")
        }

        fn generate_id_property_cache_key(&self, id: &String, property: &str) -> String {
            property.to_string() + ":" + &id.trim().to_lowercase()
        }

        fn get_wikidata_items_for_property(
            &mut self,
            mw_api: &mediawiki::api::Api,
            id: &String,
            property: &str,
        ) -> Vec<String> {
            let cache_key = self.generate_id_property_cache_key(id, property);
            match self.id_cache.get(&cache_key) {
                Some(qs) => return qs.split('|').map(|s| s.to_string()).collect(),
                None => {}
            }

            // TODO use search instead, where applicable (DOIs don't seem to work with haswbstatement)
            let sparql = format!(
                "SELECT DISTINCT ?q {{ VALUES ?id {{ '{}' '{}' '{}' }} . ?q wdt:{} ?id }}",
                id,
                id.to_uppercase(),
                id.to_lowercase(),
                &property
            ); // DOIs in Wikidata can be any upper/lowercase :-(
            let res = match mw_api.sparql_query(&sparql) {
                Ok(res) => res,
                _ => return vec![],
            };
            let ret: Vec<String> = mw_api.entities_from_sparql_result(&res, "q");
            self.id_cache.insert(cache_key, ret.join("|"));
            ret
        }

        /*
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
        */
        pub fn update_item_from_adapters(
            &mut self,
            mut item: &mut Entity,
            adapter2work_id: &mut HashMap<usize, String>,
        ) {
            for adapter_id in 0..self.adapters.len() {
                let publication_id = match self.adapters[adapter_id].publication_id_from_item(item) {
                    Some(id) => id,
                    _ => continue,
                };

                let adapter = &self.adapters[adapter_id];
                adapter2work_id.insert(adapter_id, publication_id.clone());
                println!("Applying adapter {}", adapter.name());

                adapter.update_statements_for_publication_id_default(
                    &publication_id,
                    &mut item,
                    &mut self.caches,
                );
                adapter.update_statements_for_publication_id(&publication_id, &mut item);

                let authors = adapter.get_author_list(&publication_id);
                if !authors.is_empty() {
                    println!("!!Authors: {:?}", &authors);
                }
                // TODO use authors

                let publication_ids = adapter.get_identifier_list(&publication_id);
                if !publication_ids.is_empty() {
                    println!("!!PubIDs: {:?}", &publication_ids);
                }
                // TODO add to item, re-run all adapters

                //println!("{}", serde_json::to_string_pretty(&new_item).unwrap());
            }
        }

        pub fn update_authors_from_adapters(
            &mut self,
            item: &mut Entity,
            adapter2work_id: &HashMap<usize, String>,
            mw_api: &mut mediawiki::api::Api,
        ) {
            let mut entities = entity_container::EntityContainer::new();
            let mut claims = item.claims().to_owned();

            // SS authors (P50) match
            let mut p50_authors: HashSet<String> = HashSet::new();
            for claim_num in 0..claims.len() {
                let claim = &claims[claim_num];
                if claim.claim_type() != "statement"
                    || *claim.main_snak().datatype() != wikibase::SnakDataType::WikibaseItem
                    || claim.main_snak().property() != "P50"
                {
                    continue;
                }
                let datavalue = match claim.main_snak().data_value() {
                    Some(dv) => dv,
                    None => continue,
                };
                match datavalue.value() {
                    Value::Entity(entity) => {
                        let q = entity.id();
                        p50_authors.insert(q.into());
                    }
                    _ => continue,
                }
            }

            // SS authors (P2093) match
            let mut claims_to_replace = vec![];
            for claim_num in 0..claims.len() {
                let claim = &claims[claim_num];
                if claim.claim_type() != "statement"
                    || *claim.main_snak().datatype() != wikibase::SnakDataType::String
                    || claim.main_snak().property() != "P2093"
                {
                    continue;
                }
                let datavalue = match claim.main_snak().data_value() {
                    Some(dv) => dv,
                    None => continue,
                };
                let author_name = match datavalue.value() {
                    Value::StringValue(s) => s,
                    _ => continue,
                };
                let mut author_q: Option<String> = None;
                for adapter_num in 0..self.adapters.len() {
                    match self.adapters[adapter_num].author2item(
                        &author_name,
                        mw_api,
                        adapter2work_id.get(&adapter_num),
                        None,
                    ) {
                        AuthorItemInfo::WikidataItem(q) => {
                            println!(
                                "{}: {} => {}",
                                self.adapters[adapter_num].name(),
                                &author_name,
                                &q
                            );
                            author_q = Some(q);
                            break;
                        }
                        _ => continue,
                    }
                }

                let mut author_item: Entity;
                let original_item: Entity;
                match author_q {
                    Some(q) => {
                        if p50_authors.contains(&q) {
                            // Paranoia
                            continue;
                        }
                        if entities.load_entities(&mw_api, &vec![q.clone()]).is_err() {
                            continue;
                        }
                        author_item = match entities.get_entity(q.clone()) {
                            Some(the_item) => the_item.clone(),
                            None => continue,
                        };
                        original_item = author_item.clone();
                    }
                    None => {
                        original_item = Entity::new_empty_item();
                        author_item = Entity::new_empty_item();
                    }
                };

                let mut adapter_new_author: HashMap<usize, String> = HashMap::new();
                for adapter_num in 0..self.adapters.len() {
                    let res = self.adapters[adapter_num].author2item(
                        &author_name,
                        mw_api,
                        adapter2work_id.get(&adapter_num),
                        Some(&mut author_item),
                    );
                    match res {
                        AuthorItemInfo::CatalogId(author_id) => {
                            adapter_new_author.insert(adapter_num, author_id)
                        }
                        _ => continue,
                    };
                }

                let mut diff_params = EntityDiffParams::none();
                diff_params.labels.add = EntityDiffParamState::All;
                diff_params.aliases.add = EntityDiffParamState::All;
                diff_params.descriptions.add = EntityDiffParamState::All;
                diff_params.claims.add = EntityDiffParamState::All;

                let diff = EntityDiff::new(&original_item, &author_item, &diff_params);
                if diff.is_empty() {
                    if author_item.id().is_empty() {
                        // Diff is empty, no ID => don't bother
                        continue;
                    }
                    println!(
                        "No change for author '{}' https://www.wikidata.org/wiki/{}",
                        &author_name,
                        author_item.id()
                    );
                    claims_to_replace.push((claim_num, author_item.id().to_string()));
                    continue;
                }

                //println!("{:?}", &diff_params);
                let new_json = diff.apply_diff(mw_api, &diff).unwrap(); // target?
                println!("{}", ::serde_json::to_string_pretty(&new_json).unwrap());
                let entity_id = EntityDiff::get_entity_id(&new_json).unwrap();
                println!("https://www.wikidata.org/wiki/{}", &entity_id);

                // Update author caches
                for adapter_num in 0..self.adapters.len() {
                    match adapter_new_author.get(&adapter_num) {
                        Some(author_id) => {
                            self.adapters[adapter_num].set_author_cache_entry(&author_id, &entity_id);
                        }
                        None => continue,
                    }
                }

                claims_to_replace.push((claim_num, entity_id.to_string()));
            }

            self.replace_author_string_with_author_items(&mut claims_to_replace, &mut claims);
            item.set_claims(claims);
        }

        pub fn replace_author_string_with_author_items(
            &mut self,
            claims_to_replace: &mut std::vec::Vec<(usize, std::string::String)>,
            claims: &mut Vec<Statement>,
        ) {
            // Replace P2093 claims with P50
            if claims_to_replace.is_empty() {
                // Nothing to do
                return;
            }
            while !claims_to_replace.is_empty() {
                let (claim_num, q) = claims_to_replace.pop().unwrap();
                if q.is_empty() {
                    continue; // Paranoia
                }
                let claim = claims[claim_num].to_owned();
                let datavalue = match claim.main_snak().data_value() {
                    Some(dv) => dv,
                    None => continue,
                };
                let author_name = match datavalue.value() {
                    Value::StringValue(s) => s,
                    _ => continue,
                };
                let mut qualifiers = claim.qualifiers().to_owned();
                let references = claim.references().to_owned();

                // Add original name as qualifier
                // or rather P1810?
                qualifiers.push(Snak::new_string("P1932", author_name));

                let new_claim =
                    Statement::new_normal(Snak::new_item("P50", &q), qualifiers, references);

                // Add new claim
                claims.push(new_claim);

                // Remove string claim
                claims.remove(claim_num);
            }
        }

        fn create_blank_item_for_publication_from_doi(&self, doi: &String) -> Entity {
            let mut item = Entity::new_empty_item();
            item.add_claim(Statement::new_normal(
                Snak::new_external_id("P356", doi),
                vec![],
                vec![],
            ));

            item
        }

    fn create_blank_item_for_publication_from_ids(
        &self,
        ids: &MultiMap<&str, &str>,
    ) -> wikibase::Entity {
        let mut item = Entity::new_empty_item();
        PUBLICATION_KEYS2PROPERTY
            .iter()
            .for_each(|(key, prop)| match ids.get_vec(key) {
                Some(v) => {
                    v.iter().for_each(|id| {
                        let id = self.fix_paper_id_prefix(&id.to_string());
                        item.add_claim(Statement::new_normal(
                            Snak::new_external_id(prop.to_string(), id),
                            vec![],
                            vec![],
                        ));
                    });
                }
                None => {}
            });
        item
    }

    pub fn get_entities_for_paper_ids(
        &mut self,
        mw_api: &mut mediawiki::api::Api,
        ids: &MultiMap<&str, &str>,
    ) -> Vec<String> {
        let mut qs = HashSet::new();
        match ids.get_vec("PMID") {
            Some(v) => {
                for pmid in v {
                    self.get_wikidata_items_for_pmid(&mw_api, &pmid.to_string())
                        .iter()
                        .for_each(|q| {
                            qs.insert(q.clone());
                        });
                }
            }
            None => {}
        }
        match ids.get_vec("PMCID") {
            Some(v) => {
                for pmcid in v {
                    self.get_wikidata_items_for_pmcid(&mw_api, &pmcid.to_string())
                        .iter()
                        .for_each(|q| {
                            qs.insert(q.clone());
                        });
                }
            }
            None => {}
        }
        match ids.get_vec("DOI") {
            Some(v) => {
                for doi in v {
                    self.get_wikidata_items_for_doi(&mw_api, &doi.to_string())
                        .iter()
                        .for_each(|q| {
                            qs.insert(q.clone());
                        });
                }
            }
            None => {}
        }

        let ret: Vec<String> = qs.iter().map(|s| s.to_string()).collect();
        ret
    }

    // ID keys need to be uppercase (e.g. "PMID","DOI")
    pub fn update_from_paper_ids(
        &mut self,
        mw_api: &mut mediawiki::api::Api,
        ids: &MultiMap<&str, &str>,
    ) {
        let qs = self.get_entities_for_paper_ids(mw_api, ids);

        let mut entities = wikibase::entity_container::EntityContainer::new();
        let mut item: wikibase::Entity;
        let original_item;

        match qs.len() {
            0 => {
                original_item = Entity::new_empty_item();
                item = self.create_blank_item_for_publication_from_ids(&ids);
            }
            1 => {
                let q = qs.iter().last().unwrap();
                if entities.load_entities(&mw_api, &vec![q.clone()]).is_err() {
                    return;
                }

                match entities.get_entity(q.clone()) {
                    Some(the_item) => {
                        item = the_item.clone();
                    }
                    None => return,
                };
                original_item = item.clone();
            }
            n => {
                println!("{} items for IDs {:?}", &n, &ids);
                return;
            }
        };
        let mut adapter2work_id = HashMap::new();
        //self.update_item_from_adapters(&mut item, &mut adapter2work_id);
        //self.update_authors_from_adapters(&mut item, &adapter2work_id, mw_api);
        //self.apply_changes(mw_api, &item, &original_item);
    }

    fn apply_changes(
        &mut self,
        mw_api: &mut mediawiki::api::Api,
        item: &wikibase::Entity,
        original_item: &wikibase::Entity,
    ) {
        let mut diff_params = EntityDiffParams::none();
        diff_params.labels.add = EntityDiffParamState::All;
        diff_params.aliases.add = EntityDiffParamState::All;
        diff_params.descriptions.add = EntityDiffParamState::All;
        let mut claims_add: Vec<String> = PUBLICATION_KEYS2PROPERTY
            .iter()
            .map(|s| s.1.to_string())
            .collect();
        let mut claims_remove = vec![];
        for adapter in &self.adapters {
            match adapter.publication_property() {
                Some(p) => claims_add.push(p),
                None => {}
            }
        }
        claims_add.push("P50".to_string());
        claims_remove.push("P2093".to_string());

        diff_params.claims.add = EntityDiffParamState::Some(claims_add);
        diff_params.claims.remove = EntityDiffParamState::Some(claims_remove);

        let diff = EntityDiff::new(&original_item, &item, &diff_params);
        if diff.is_empty() {
            println!("No change");
            return;
        }
        println!("{}", diff.to_string_pretty().unwrap());
        if false {
            let new_json = diff.apply_diff(mw_api, &diff).unwrap();
            //println!("{}", ::serde_json::to_string_pretty(&new_json).unwrap());
            let entity_id = EntityDiff::get_entity_id(&new_json).unwrap();
            println!("https://www.wikidata.org/wiki/{}", &entity_id);
        }
    }

        pub fn update_dois(&mut self, mw_api: &mut mediawiki::api::Api, dois: &Vec<&str>) {
            self.caches.init(mw_api);
            let mut entities = wikibase::entity_container::EntityContainer::new();

            for doi in dois {
                let mut item;
                let original_item;
                let qs = self.get_wikidata_items_for_doi(&mw_api, &doi.to_string());

                match qs.len() {
                    0 => {
                        original_item = Entity::new_empty_item();
                        item = self.create_blank_item_for_publication_from_doi(&doi.to_string());
                    }
                    1 => {
                        let q = &qs[0];
                        if entities.load_entities(&mw_api, &vec![q.clone()]).is_err() {
                            continue;
                        }

                        item = match entities.get_entity(q.clone()) {
                            Some(the_item) => the_item.clone(),
                            None => continue,
                        };
                        original_item = item.clone();
                    }
                    n => {
                        println!("{} items for DOI {}", &n, &doi);
                        continue;
                    }
                };
                let mut adapter2work_id = HashMap::new();
                self.update_item_from_adapters(&mut item, &mut adapter2work_id);
                self.update_authors_from_adapters(&mut item, &adapter2work_id, mw_api);
                self.apply_changes(mw_api, &item, &original_item);
            }
        }
    */

    // ID keys need to be uppercase (e.g. "PMID","DOI")
    pub fn update_from_paper_ids(
        &mut self,
        original_ids: &Vec<GenericWorkIdentifier>,
    ) -> Vec<GenericWorkIdentifier> {
        let mut ids: HashSet<GenericWorkIdentifier> = HashSet::new();
        for id in original_ids {
            ids.insert(id.to_owned());
        }
        loop {
            let last_id_size = ids.len();
            for adapter_id in 0..self.adapters.len() {
                let adapter = &mut self.adapters[adapter_id];
                let vids: Vec<GenericWorkIdentifier> = ids.iter().map(|x| x.to_owned()).collect();
                //println!("Adapter {}", adapter.name());
                adapter.get_identifier_list(&vids).iter().for_each(|id| {
                    ids.insert(id.clone());
                });
            }
            if last_id_size == ids.len() {
                break;
            }
        }
        ids.iter().map(|x| x.to_owned()).collect()
    }

    pub fn get_items_for_ids(
        &self,
        mw_api: &mediawiki::api::Api,
        ids: &Vec<GenericWorkIdentifier>,
    ) -> Vec<String> {
        let mut parts: Vec<String> = vec![];
        for id in ids {
            match &id.work_type {
                GenericWorkType::Property(prop) => {
                    parts.push(format!("?q wdt:{} '{}'", &prop, &id.id));
                    if prop == PROP_DOI {
                        parts.push(format!("?q wdt:{} '{}'", &prop, &id.id.to_lowercase()));
                        parts.push(format!("?q wdt:{} '{}'", &prop, &id.id.to_uppercase()));
                    }
                }
                GenericWorkType::Item => {}
            }
        }
        if parts.is_empty() {
            return vec![];
        }
        parts.sort();
        parts.dedup();
        let sparql = format!("SELECT DISTINCT ?q {{ {{ {} }} }}", parts.join("} UNION {"));
        println!("SPARQL: {}", &sparql);
        match mw_api.sparql_query(&sparql) {
            Ok(result) => mw_api.entities_from_sparql_result(&result, "q"),
            _ => vec![],
        }
    }
}
