extern crate crossref;
extern crate reqwest;
#[macro_use]
extern crate lazy_static;
#[macro_use]
extern crate serde_json;

use crate::wikidata_papers::WikidataPapersCache;
use regex::Regex;
use std::collections::HashMap;
use wikibase::entity_diff::*;
use wikibase::*;

pub const PROP_PMID: &str = "P698";
pub const PROP_PMCID: &str = "P932";
pub const PROP_DOI: &str = "P356";
pub const PROP_ARXIV: &str = "P818";

#[derive(Debug, Clone, PartialEq)]
pub struct GenericAuthorInfo {
    pub name: Option<String>,
    pub prop2id: HashMap<String, String>,
    pub wikidata_item: Option<String>,
    pub list_number: Option<String>,
    pub alternative_names: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum GenericWorkType {
    Property(String),
    Item,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct GenericWorkIdentifier {
    pub work_type: GenericWorkType,
    pub id: String,
}

impl GenericWorkIdentifier {
    pub fn new_prop(prop: &str, id: &str) -> Self {
        return GenericWorkIdentifier {
            work_type: GenericWorkType::Property(prop.to_string()),
            id: id.to_string(),
        };
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum AuthorItemInfo {
    WikidataItem(String),
    CatalogId(String),
    None,
}

pub trait ScientificPublicationAdapter {
    // You will need to implement these yourself

    /// Returns the name of the resource; internal/debugging use only
    fn name(&self) -> &str;

    /// Returns a cache object reference for the author_id => wikidata_item mapping; this is handled automatically
    fn author_cache(&self) -> &HashMap<String, String>;

    /// Returns a mutable cache object reference for the author_id => wikidata_item mapping; this is handled automatically
    fn author_cache_mut(&mut self) -> &mut HashMap<String, String>;

    /// Tries to determine the publication ID of the resource, from a Wikidata item
    fn publication_id_from_item(&mut self, item: &Entity) -> Option<String> {
        match self.publication_property() {
            Some(self_prop) => match self.get_external_identifier_from_item(item, &self_prop) {
                Some(publication_id) => self.do_cache_work(&publication_id),
                None => None,
            },
            None => None,
        }
    }

    /// Adds/updates "special" statements of an item from the resource, given the publication ID.
    /// Many common statements, title, aliases etc are automatically handeled via `update_statements_for_publication_id_default`
    fn update_statements_for_publication_id(&self, publication_id: &String, item: &mut Entity);

    // You should implement these yourself, where applicable

    /// Returns a list of the authors, if available, with list number, name, catalog-specific author ID, and WIkidata ID, as available
    fn get_author_list(&mut self, _publication_id: &String) -> Vec<GenericAuthorInfo> {
        vec![]
    }

    /// Returns a list of IDs for that paper (PMID, DOI etc.)
    fn get_identifier_list(
        &mut self,
        _ids: &Vec<GenericWorkIdentifier>,
    ) -> Vec<GenericWorkIdentifier> {
        vec![]
    }

    /// Returns the property for an author ID of the resource as a `String`, e.g. P4012 for Semantic Scholar
    fn author_property(&self) -> Option<String> {
        None
    }

    /// Returns the property for a publication ID of the resource as a `String`, e.g. P4011 for Semantic Scholar
    fn publication_property(&self) -> Option<String> {
        None
    }

    /// Returns the property for a topic ID of the resource as a `String`, e.g. P6611 for Semantic Scholar
    fn topic_property(&self) -> Option<String> {
        None
    }

    // For a publication ID, return the ISSN as a `String`, if known
    fn get_work_issn(&self, _publication_id: &String) -> Option<String> {
        None
    }

    // For a publication ID, return all known titles as a `Vec<LocaleString>`, main title first (per language)
    fn get_work_titles(&self, _publication_id: &String) -> Vec<LocaleString> {
        vec![]
    }

    // Pre-filled methods; no need to implement them unless there is a need

    fn create_item(&self, item: &Entity, mw_api: &mut mediawiki::api::Api) -> Option<String> {
        let params = EntityDiffParams::all();
        let diff = EntityDiff::new(&Entity::new_empty_item(), item, &params);
        if diff.is_empty() {
            return None;
        }
        let new_json = diff.apply_diff(mw_api, &diff).unwrap();
        EntityDiff::get_entity_id(&new_json)
    }

    fn create_or_update_author_statements(
        &mut self,
        publication_id: &String,
        item: &mut Entity,
        mw_api: &mut mediawiki::api::Api,
    ) {
        if !item.has_claims_with_property("P50") && !item.has_claims_with_property("P2093") {
            self.create_author_statements(publication_id, item, mw_api);
        } else {
            self.update_author_statements(publication_id, item);
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
        match &author.name {
            Some(name) => item.set_label(LocaleString::new("en", name)),
            None => {}
        }
        for n in &author.alternative_names {
            item.add_alias(LocaleString::new("en", n));
        }

        // Human
        item.add_claim(Statement::new_normal(
            Snak::new_item("P31", "Q5"),
            vec![],
            self.reference(),
        ));

        // Researcher
        item.add_claim(Statement::new_normal(
            Snak::new_item("P106", "Q1650915"),
            vec![],
            self.reference(),
        ));

        // External IDs
        for (prop, id) in &ret.prop2id {
            let statement = Statement::new_normal(
                Snak::new_external_id(prop.to_string(), id.to_string()),
                vec![],
                self.reference(),
            );
            item.add_claim(statement);
        }

        // Create new item and use its ID
        ret.wikidata_item = self.create_item(&item, mw_api);
        ret
    }

    fn create_author_statements(
        &mut self,
        publication_id: &String,
        item: &mut Entity,
        mw_api: &mut mediawiki::api::Api,
    ) {
        let authors = self.get_author_list(publication_id);
        let authors: Vec<GenericAuthorInfo> = authors
            .iter()
            .map(|author| self.get_or_create_author_item(author, mw_api))
            .collect();
        for author in &authors {
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
                    Statement::new_normal(Snak::new_item("P50", &q), qualifiers, self.reference())
                }
                None => Statement::new_normal(
                    Snak::new_string("P2093", &name),
                    qualifiers,
                    self.reference(),
                ),
            };
            item.add_claim(statement);
        }
    }

    fn update_author_statements(&self, _publication_id: &String, _item: &mut Entity) {}

    fn do_cache_work(&mut self, _publication_id: &String) -> Option<String> {
        None
    }

    fn reference(&self) -> Vec<Reference> {
        // TODO
        vec![]
    }

    fn sanitize_author_name(&self, author_name: &String) -> String {
        author_name
            .replace("†", "")
            .replace("‡", "")
            .trim()
            .to_string()
    }

    fn update_statements_for_publication_id_default(
        &self,
        publication_id: &String,
        item: &mut Entity,
        caches: &mut WikidataPapersCache,
    ) {
        self.update_work_item_with_title(publication_id, item);
        self.update_work_item_with_property(publication_id, item);
        self.update_work_item_with_journal(publication_id, item, caches);
    }

    fn titles_are_equal(&self, t1: &String, t2: &String) -> bool {
        // Maybe it's easy...
        if t1 == t2 {
            return true;
        }
        // Not so easy then...
        let t1 = t1
            .clone()
            .to_lowercase()
            .trim_end_matches('.')
            .to_string()
            .trim()
            .to_string();
        let t2 = t2
            .clone()
            .to_lowercase()
            .trim_end_matches('.')
            .to_string()
            .trim()
            .to_string();
        return t1 == t2;
    }

    fn update_work_item_with_title(&self, publication_id: &String, item: &mut Entity) {
        let titles = self.get_work_titles(publication_id);
        if titles.len() == 0 {
            return;
        }

        // Re-org
        let mut by_lang: HashMap<String, Vec<String>> = HashMap::new();
        titles.iter().for_each(|t| {
            let lv = by_lang.entry(t.language().to_string()).or_insert(vec![]);
            lv.push(t.value().to_string())
        });

        for (language, titles) in by_lang.iter() {
            let mut titles = titles.clone();
            // Add title
            match item.label_in_locale(&language) {
                Some(t) => {
                    titles.retain(|x| !self.titles_are_equal(&x.to_string(), &t.to_string()))
                } // Title exists, remove from title list
                None => item.set_label(LocaleString::new("en", &titles.swap_remove(0))), // No title, add and remove from title list
            }
            let main_title = item.label_in_locale("en").unwrap_or("").to_string();

            // Add other potential titles as aliases
            titles
                .iter()
                .filter(|t| !dbg!(self.titles_are_equal(t, &main_title)))
                .for_each(|t| {
                    item.add_alias(LocaleString::new(language.to_string(), t.to_string()))
                });

            // Add P1476 (title)
            if !item.has_claims_with_property("P1476") {
                match item.label_in_locale(&language) {
                    Some(title) => item.add_claim(Statement::new_normal(
                        Snak::new_monolingual_text("P1476", &language, title),
                        vec![],
                        self.reference(),
                    )),
                    None => {}
                }
            }
        }
    }

    fn update_work_item_with_journal(
        &self,
        publication_id: &String,
        item: &mut Entity,
        caches: &mut wikidata_papers::WikidataPapersCache,
    ) {
        if item.has_claims_with_property("P1433") {
            return;
        }
        match self.get_work_issn(publication_id) {
            Some(issn) => match caches.issn2q(&issn) {
                Some(q) => item.add_claim(Statement::new_normal(
                    Snak::new_string("P1433", &q),
                    vec![],
                    self.reference(),
                )),
                None => {}
            },
            _ => {}
        }
    }

    fn update_work_item_with_property(&self, publication_id: &String, item: &mut Entity) {
        match self.publication_property() {
            Some(prop) => {
                if !item.has_claims_with_property(prop) {
                    item.add_claim(Statement::new_normal(
                        Snak::new_external_id(
                            self.publication_property().unwrap(),
                            publication_id.to_string(),
                        ),
                        vec![],
                        self.reference(),
                    ));
                }
            }
            _ => {}
        }
    }

    fn get_wb_time_from_partial(
        &self,
        property: String,
        year: u32,
        month: Option<u8>,
        day: Option<u8>,
    ) -> Statement {
        let mut precision: u64 = 9; // Year; default
        let mut time = "+".to_string();
        time += &year.to_string();
        match month {
            Some(x) => {
                time += &format!("-{:02}", x);
                precision = 10
            }
            None => time += "-01",
        };
        match day {
            Some(x) => {
                time += &format!("-{:02}", x);
                precision = 11
            }
            None => time += "-01",
        };
        time += "T00:00:00Z";
        Statement::new_normal(
            Snak::new_time(property, time, precision),
            vec![],
            self.reference(),
        )
    }

    fn get_external_identifier_from_item(&self, item: &Entity, property: &str) -> Option<String> {
        for claim in item.claims() {
            if claim.main_snak().property() == property
                && claim.main_snak().snak_type().to_owned() == SnakType::Value
            {
                match claim.main_snak().data_value() {
                    Some(dv) => {
                        let value = dv.value().clone();
                        match &value {
                            Value::StringValue(s) => return Some(s.to_string()),
                            _ => continue,
                        }
                    }
                    None => continue,
                }
            }
        }
        None
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

    /// Compares long (3+ characters) name parts and returns true if identical
    fn author_names_match(&self, name1: &str, name2: &str) -> bool {
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
            return !parts1.is_empty() && parts1 == parts2;
        }
        false
    }

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

    fn author2item(
        &mut self,
        _author_name: &String,
        _mw_api: &mut mediawiki::api::Api,
        _publication_id: Option<&String>,
        _item: Option<&mut Entity>,
    ) -> AuthorItemInfo {
        AuthorItemInfo::None
    }

    fn update_author_item(
        &mut self,
        source_author_name: &String,
        author_id: &String,
        author_name: &String,
        item: &mut Entity,
    ) {
        item.set_label(LocaleString::new("en", &source_author_name));
        if source_author_name != author_name {
            item.add_alias(LocaleString::new("en", &author_name));
        }

        if !item.has_claims_with_property("P31") {
            item.add_claim(Statement::new_normal(
                Snak::new_item("P31", "Q5"),
                vec![],
                self.reference(),
            ));
        }
        match self.author_property() {
            Some(prop) => {
                if !item.has_claims_with_property("P31") {
                    item.add_claim(Statement::new_normal(
                        Snak::new_external_id(prop, author_id.to_string()),
                        vec![],
                        self.reference(),
                    ));
                }
            }
            None => {}
        }
    }

    /*
    fn get_author_item_id(
        &mut self,
        catalog_author_id: &String,
        mw_api: &mediawiki::api::Api,
    ) -> Option<String> {
        let author_property = match self.author_property() {
            Some(p) => p,
            None => return None,
        };
        // Load all authors from Wikidata, if not done so already
        if self.author_cache_is_empty() {
            let res = mw_api
                .sparql_query(&("SELECT ?q ?id { ?q wdt:".to_owned() + &author_property + " ?id }"))
                .unwrap();

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
        .map(|(x, y)| (x.to_string(), y.to_string()))
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
    */
}

pub mod crossref2wikidata;
pub mod orcid2wikidata;
pub mod pubmed2wikidata;
pub mod semanticscholar2wikidata;
pub mod wikidata_papers;
