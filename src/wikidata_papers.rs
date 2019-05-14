extern crate config;
extern crate lazy_static;
extern crate mediawiki;
extern crate regex;
extern crate serde_json;
extern crate wikibase;

use crate::*;
use regex::Regex;
use std::collections::HashMap;
use std::collections::HashSet;
//use wikibase::*;
//use crate::AuthorItemInfo;
//use multimap::MultiMap;
//use wikibase::entity_diff::*;

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
    caches: WikidataPapersCache,
    //id_cache: HashMap<String, String>,
}

impl WikidataPapers {
    pub fn new() -> WikidataPapers {
        WikidataPapers {
            adapters: vec![],
            caches: WikidataPapersCache::new(),
            //id_cache: HashMap::new(),
        }
    }

    pub fn adapters_mut(&mut self) -> &mut Vec<Box<ScientificPublicationAdapter>> {
        &mut self.adapters
    }

    pub fn add_adapter(&mut self, adapter_box: Box<ScientificPublicationAdapter>) {
        self.adapters.push(adapter_box);
    }

    fn create_item(&self, item: &Entity, mw_api: &mut mediawiki::api::Api) -> Option<String> {
        let params = EntityDiffParams::all();
        let diff = EntityDiff::new(&Entity::new_empty_item(), item, &params);
        if diff.is_empty() {
            return None;
        }
        let new_json = diff.apply_diff(mw_api, &diff).unwrap();
        EntityDiff::get_entity_id(&new_json)
    }

    fn create_author_statements(&mut self, authors: &Vec<GenericAuthorInfo>, item: &mut Entity) {
        for author in authors {
            let name = match &author.name {
                Some(s) => s.to_string(),
                None => "".to_string(),
            };
            let mut qualifiers: Vec<Snak> = vec![];
            match &author.list_number {
                Some(num) => {
                    qualifiers.push(Snak::new_string("P1545", &num));
                }
                None => {}
            }
            let statement = match &author.wikidata_item {
                Some(q) => {
                    if !name.is_empty() {
                        qualifiers.push(Snak::new_string("P1932", &name));
                    }
                    Statement::new_normal(Snak::new_item("P50", &q), qualifiers, vec![])
                }
                None => Statement::new_normal(Snak::new_string("P2093", &name), qualifiers, vec![]),
            };
            item.add_claim(statement);
        }
    }

    fn update_author_statements(&self, _authors: &Vec<GenericAuthorInfo>, _item: &mut Entity) {
        // TODO
    }

    fn create_or_update_author_statements(
        &mut self,
        item: &mut Entity,
        authors: &Vec<GenericAuthorInfo>,
    ) {
        if !item.has_claims_with_property("P50") && !item.has_claims_with_property("P2093") {
            self.create_author_statements(authors, item);
        } else {
            self.update_author_statements(authors, item);
        }
    }

    fn search_external_id(
        &self,
        property: &str,
        id: &str,
        mw_api: &mediawiki::api::Api,
    ) -> Vec<String> {
        let query: String = "haswbstatement:".to_owned() + &property + &"=".to_owned() + &id;
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
        let mut ret: Vec<String> = vec![];
        match res["query"]["search"].as_array() {
            Some(items) => {
                for item in items {
                    let q = item["title"].as_str().unwrap();
                    ret.push(q.to_string());
                }
            }
            None => {}
        }
        ret
    }

    pub fn amend_author_item(&self, item: &mut Entity, author: &GenericAuthorInfo) {
        // Set label, unless already set (then try alias)
        match &author.name {
            Some(name) => {
                if !name.is_empty() {
                    match item.label_in_locale("en") {
                        Some(s) => {
                            if s != name {
                                item.add_alias(LocaleString::new("en", name));
                            }
                        }
                        None => item.set_label(LocaleString::new("en", name)),
                    }
                }
            }
            None => {}
        }

        // Alternative names
        for n in &author.alternative_names {
            if !n.is_empty() {
                match item.label_in_locale("en") {
                    Some(s) => {
                        if s != n {
                            item.add_alias(LocaleString::new("en", n));
                        }
                    }
                    None => {
                        item.add_alias(LocaleString::new("en", n));
                    }
                }
            }
        }

        // Human
        if !item.has_target_entity("P31", "Q5") {
            item.add_claim(Statement::new_normal(
                Snak::new_item("P31", "Q5"),
                vec![],
                vec![],
            ));
        }

        // Researcher
        if !item.has_claims_with_property("P106") {
            item.add_claim(Statement::new_normal(
                Snak::new_item("P106", "Q1650915"),
                vec![],
                vec![],
            ));
        }

        // External IDs
        for (prop, id) in &author.prop2id {
            let existing = item.values_for_property(prop.to_string());
            let to_check = Value::StringValue(id.to_string());
            if existing.contains(&to_check) {
                continue;
            }
            println!(
                "Adding author statement {}:'{}' to {}",
                &prop,
                &id,
                item.id()
            );
            let statement = Statement::new_normal(
                Snak::new_external_id(prop.to_string(), id.to_string()),
                vec![],
                vec![],
            );
            item.add_claim(statement);
        }
    }

    fn get_or_create_author_item(
        &self,
        author: &GenericAuthorInfo,
        mw_api: &mut mediawiki::api::Api,
    ) -> GenericAuthorInfo {
        let mut ret = author.clone();
        // Already has item?
        if ret.wikidata_item.is_some() {
            return ret;
        }
        // No external IDs
        if ret.prop2id.is_empty() {
            return ret;
        }

        // Use search
        for (prop, id) in &ret.prop2id {
            let items = self.search_external_id(prop, id, mw_api);
            if !items.is_empty() {
                ret.wikidata_item = Some(items[0].clone());
                return ret;
            }
        }

        // Labels/aliases
        let mut item = Entity::new_empty_item();
        self.amend_author_item(&mut item, &ret);

        // Create new item and use its ID
        ret.wikidata_item = self.create_item(&item, mw_api);
        ret
    }

    fn asciify_string(&self, s: &str) -> String {
        // As long as some sources insist on using ASCII only for names :-(
        s.to_lowercase()
            .replace('ä', "a")
            .replace('ö', "o")
            .replace('ü', "u")
            .replace('á', "a")
            .replace('à', "a")
            .replace('â', "a")
            .replace('é', "e")
            .replace('è', "e")
            .replace('ñ', "n")
            .replace('ï', "i")
            .replace('ç', "c")
            .replace('ß', "ss")
    }

    /// Compares long (3+ characters) name parts
    fn author_names_match(&self, name1: &str, name2: &str) -> u16 {
        let mut ret = 0;
        lazy_static! {
            static ref RE1: Regex = Regex::new(r"\b(\w{3,})\b").unwrap();
        }
        let name1_mod = self.asciify_string(name1);
        let name2_mod = self.asciify_string(name2);
        if RE1.is_match(&name1_mod) && RE1.is_match(&name2_mod) {
            let mut parts1: Vec<String> = vec![];
            for cap in RE1.captures_iter(&name1_mod) {
                parts1.push(cap[1].to_string());
            }
            parts1.sort();
            let mut parts2: Vec<String> = vec![];
            for cap in RE1.captures_iter(&name2_mod) {
                parts2.push(cap[1].to_string());
            }
            parts2.sort();
            parts1.iter().for_each(|part| {
                if parts2.contains(part) {
                    ret += 1;
                }
            });
        }
        ret
    }

    fn compare_authors(&self, author1: &GenericAuthorInfo, author2: &GenericAuthorInfo) -> u16 {
        match (&author1.wikidata_item, &author2.wikidata_item) {
            (Some(q1), Some(q2)) => {
                if q1 == q2 {
                    return 100; // This is it
                } else {
                    return 0; // Different items
                }
            }
            _ => {}
        }

        let mut ret = 0;

        for (k, v) in &author1.prop2id {
            match author2.prop2id.get(k) {
                Some(v2) => {
                    if v == v2 {
                        ret += 90;
                    }
                }
                None => {}
            }
        }

        // Name match
        match (&author1.name, &author2.name) {
            (Some(n1), Some(n2)) => {
                ret += 50 * self.author_names_match(&n1.as_str(), &n2.as_str());
            }
            _ => {}
        }

        // List number
        match (&author1.list_number, &author2.list_number) {
            (Some(n1), Some(n2)) => {
                if n1 == n2 {
                    ret += 30;
                }
            }
            _ => {}
        }

        ret
    }

    fn merge_author(&self, author1: &mut GenericAuthorInfo, author2: &GenericAuthorInfo) {
        if author1.name.is_none() {
            author1.name = author2.name.clone();
        }
        if author1.wikidata_item.is_none() {
            author1.wikidata_item = author2.wikidata_item.clone();
        }
        if author1.list_number.is_none() {
            author1.list_number = author2.list_number.clone();
        }
        for (k, v) in &author2.prop2id {
            author1.prop2id.insert(k.to_string(), v.to_string());
        }
        for name in &author2.alternative_names {
            author1.alternative_names.push(name.to_string());
        }
        author1.alternative_names.sort();
        author1.alternative_names.dedup();
    }

    fn merge_authors(
        &self,
        authors: &mut Vec<GenericAuthorInfo>,
        authors2: &Vec<GenericAuthorInfo>,
    ) {
        if authors.is_empty() {
            authors2
                .iter()
                .for_each(|author| authors.push(author.clone()));
            return;
        }
        for author in authors2.iter() {
            let mut best_candidate: usize = 0;
            let mut best_points: u16 = 0;
            for candidate_id in 0..authors.len() {
                let points = self.compare_authors(&author, &authors[candidate_id]);
                if points > best_points {
                    best_points = points;
                    best_candidate = candidate_id;
                }
            }
            if best_points == 0 {
                // No match found, add the author
                authors.push(author.clone());
            } else {
                self.merge_author(&mut authors[best_candidate], &author);
            }
        }
    }

    pub fn update_item_from_adapters(
        &mut self,
        mut item: &mut Entity,
        adapter2work_id: &mut HashMap<usize, String>,
        mw_api: &mut mediawiki::api::Api,
    ) {
        let mut authors: Vec<GenericAuthorInfo> = vec![];
        for adapter_id in 0..self.adapters.len() {
            let publication_id = match self.adapters[adapter_id].publication_id_from_item(item) {
                Some(id) => id,
                _ => continue,
            };

            let adapter = &mut self.adapters[adapter_id];
            adapter2work_id.insert(adapter_id, publication_id.clone());
            //println!("Applying adapter {}", adapter.name());

            adapter.update_statements_for_publication_id_default(
                &publication_id,
                &mut item,
                &mut self.caches,
            );
            adapter.update_statements_for_publication_id(&publication_id, &mut item);

            // Authors
            let authors2 = adapter.get_author_list(&publication_id);
            self.merge_authors(&mut authors, &authors2);
        }

        let authors: Vec<GenericAuthorInfo> = authors
            .iter()
            .map(|author| self.get_or_create_author_item(author, mw_api))
            .collect();

        self.update_author_items(&authors, mw_api);

        self.create_or_update_author_statements(&mut item, &authors);
        //dbg!(&item);
    }

    fn update_author_items(
        &self,
        authors: &Vec<GenericAuthorInfo>,
        mw_api: &mut mediawiki::api::Api,
    ) {
        let mut qs: Vec<String> = vec![];
        for author in authors {
            let q = match &author.wikidata_item {
                Some(q) => q,
                None => continue,
            };
            qs.push(q.to_string());
        }
        if qs.is_empty() {
            return;
        }

        let mut entities = entity_container::EntityContainer::new();
        match entities.load_entities(mw_api, &qs) {
            Ok(_) => {}
            _ => return,
        }

        for author in authors {
            let q = match &author.wikidata_item {
                Some(q) => q.to_string(),
                None => continue,
            };
            let original_item = match entities.get_entity(q) {
                Some(i) => i.clone(),
                None => continue,
            };
            let mut item = original_item.clone();
            self.amend_author_item(&mut item, author);

            let mut params = EntityDiffParams::none();
            params.labels.add = EntityDiffParamState::All;
            params.aliases.add = EntityDiffParamState::All;
            params.claims.add = EntityDiffParamState::All;
            let diff = EntityDiff::new(&original_item, &item, &params);
            if diff.is_empty() {
                continue;
            }
            //println!("{}", diff.actions());
            let _new_json = diff.apply_diff(mw_api, &diff).unwrap();
            //EntityDiff::get_entity_id(&new_json);
        }
    }

    fn update_item_with_ids(&self, item: &mut wikibase::Entity, ids: &Vec<GenericWorkIdentifier>) {
        for id in ids {
            let prop = match &id.work_type {
                GenericWorkType::Property(prop) => prop.to_owned(),
                _ => continue,
            };
            if item.has_claims_with_property(prop.clone()) {
                // TODO use claims_with_property to check the values
                continue;
            }
            item.add_claim(Statement::new_normal(
                Snak::new_external_id(prop.clone(), id.id.clone()),
                vec![],
                vec![],
            ));
        }
    }

    pub fn create_or_update_item_from_ids(
        &mut self,
        mw_api: &mut mediawiki::api::Api,
        ids: &Vec<GenericWorkIdentifier>,
    ) -> Option<String> {
        self.caches.init(&mw_api);
        let items = self.get_items_for_ids(&mw_api, &ids);
        let mut entities = entity_container::EntityContainer::new();
        let mut item: wikibase::Entity;
        let original_item: wikibase::Entity;
        match items.get(0) {
            Some(q) => {
                item = entities.load_entity(&mw_api, q.clone()).unwrap().to_owned();
                original_item = item.clone();
            }
            None => {
                original_item = Entity::new_empty_item();
                item = Entity::new_empty_item();
                item.add_claim(Statement::new_normal(
                    Snak::new_item("P31", "Q591041"),
                    vec![],
                    vec![],
                ));
            }
        }

        self.update_item_with_ids(&mut item, &ids);

        let mut adapter2work_id = HashMap::new();
        self.update_item_from_adapters(&mut item, &mut adapter2work_id, mw_api);

        let mut params = EntityDiffParams::none();
        params.labels.add = EntityDiffParamState::All;
        params.aliases.add = EntityDiffParamState::All;
        params.claims.add = EntityDiffParamState::All;
        let diff = EntityDiff::new(&original_item, &item, &params);
        if diff.is_empty() {
            match original_item.id() {
                "" => return None,
                id => return Some(id.to_string()),
            }
        }
        let new_json = diff.apply_diff(mw_api, &diff).unwrap();
        EntityDiff::get_entity_id(&new_json)
    }

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
                GenericWorkType::Item => {
                    parts.push(format!("VALUES ?q {{ wd:{} }}", &id.id));
                }
            }
        }
        if parts.is_empty() {
            return vec![];
        }
        parts.sort();
        parts.dedup();
        let sparql = format!("SELECT DISTINCT ?q {{ {{ {} }} }}", parts.join("} UNION {"));
        match mw_api.sparql_query(&sparql) {
            Ok(result) => mw_api.entities_from_sparql_result(&result, "q"),
            _ => vec![],
        }
    }
}
