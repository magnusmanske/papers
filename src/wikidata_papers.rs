extern crate config;
extern crate lazy_static;
extern crate mediawiki;
extern crate regex;
extern crate serde_json;
extern crate wikibase;

use crate::generic_author_info::GenericAuthorInfo;
use crate::scientific_publication_adapter::ScientificPublicationAdapter;
use crate::*;
use std::collections::HashMap;
use std::collections::HashSet;

pub struct EditResult {
    pub q: String,
    pub edited: bool,
}

pub struct WikidataPapersCache {
    issn2q: HashMap<String, String>,
    is_initialized: bool,
    mw_api: Option<mediawiki::api::Api>,
}

impl WikidataInteraction for WikidataPapersCache {}

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
        let mw_api = match &self.mw_api {
            Some(x) => x,
            None => panic!("no mw_api set in WikidataPapersCache".to_string()),
        };
        match self.search_wikibase(&("haswbstatement:P236=".to_string() + issn), mw_api) {
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

impl WikidataInteraction for WikidataPapers {}

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

    fn create_author_statements(&mut self, authors: &Vec<GenericAuthorInfo>, item: &mut Entity) {
        for author in authors {
            author.create_author_statement_in_paper_item(item);
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
        // TODO check for duplicate P50/P2093
        if !item.has_claims_with_property("P50") && !item.has_claims_with_property("P2093") {
            self.create_author_statements(authors, item);
        } else {
            self.update_author_statements(authors, item);
        }
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
        println!("MERGING AUTHOR: {:?} AND {:?}", &authors, &authors2);
        for author in authors2.iter() {
            let mut best_candidate: usize = 0;
            let mut best_points: u16 = 0;
            for candidate_id in 0..authors.len() {
                let points = author.compare(&authors[candidate_id]);
                if points > best_points {
                    best_points = points;
                    best_candidate = candidate_id;
                }
            }
            if best_points == 0 {
                // No match found, add the author
                authors.push(author.clone());
            } else {
                authors[best_candidate].merge_from(&author);
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
            .map(|author| author.get_or_create_author_item(mw_api))
            .collect();

        self.update_author_items(&authors, mw_api);

        self.create_or_update_author_statements(&mut item, &authors);
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
            author.update_author_item(&entities, mw_api);
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
    ) -> Option<EditResult> {
        self.caches.init(&mw_api);
        let items = self.get_items_for_ids(&mw_api, &ids);
        self.create_or_update_item_from_items(mw_api, ids, &items)
    }

    pub fn create_or_update_item_from_q(
        &mut self,
        mw_api: &mut mediawiki::api::Api,
        q: &String,
    ) -> Option<EditResult> {
        self.caches.init(&mw_api);
        let items = vec![q.to_owned()];
        self.create_or_update_item_from_items(mw_api, &vec![], &items)
    }

    fn create_or_update_item_from_items(
        &mut self,
        mw_api: &mut mediawiki::api::Api,
        ids: &Vec<GenericWorkIdentifier>,
        items: &Vec<String>,
    ) -> Option<EditResult> {
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
                id => {
                    return Some(EditResult {
                        q: id.to_string(),
                        edited: false,
                    })
                }
            }
        }
        let new_json = diff.apply_diff(mw_api, &diff).unwrap();
        let q = EntityDiff::get_entity_id(&new_json)?;
        Some(EditResult {
            q: q.to_string(),
            edited: true,
        })
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
