extern crate crossref;
extern crate lazy_static;
extern crate reqwest;
extern crate serde_json;

use crate::*;
use regex::Regex;
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq)]
pub struct GenericAuthorInfo {
    pub name: Option<String>,
    pub prop2id: HashMap<String, String>,
    pub wikidata_item: Option<String>,
    pub list_number: Option<String>,
    pub alternative_names: Vec<String>,
}

impl WikidataInteraction for GenericAuthorInfo {}

impl GenericAuthorInfo {
    pub fn create_author_statement_in_paper_item(&self, item: &mut Entity) {
        let name = match &self.name {
            Some(s) => s.to_string(),
            None => "".to_string(),
        };
        let mut qualifiers: Vec<Snak> = vec![];
        match &self.list_number {
            Some(num) => {
                qualifiers.push(Snak::new_string("P1545", &num));
            }
            None => {}
        }
        let statement = match &self.wikidata_item {
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

    pub fn amend_author_item(&self, item: &mut Entity) {
        // Set label, unless already set (then try alias)
        match &self.name {
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
        for n in &self.alternative_names {
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
        for (prop, id) in &self.prop2id {
            let existing = item.values_for_property(prop.to_string());
            let to_check = Value::StringValue(id.to_string());
            if existing.contains(&to_check) {
                continue;
            }
            /*
            println!(
                "Adding author statement {}:'{}' to {}",
                &prop,
                &id,
                item.id()
            );
            */
            let statement = Statement::new_normal(
                Snak::new_external_id(prop.to_string(), id.to_string()),
                vec![],
                vec![],
            );
            item.add_claim(statement);
        }
    }

    pub fn get_or_create_author_item(&self, mw_api: &mut mediawiki::api::Api) -> GenericAuthorInfo {
        let mut ret = self.clone();
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
        ret.amend_author_item(&mut item);

        // Create new item and use its ID
        ret.wikidata_item = self.create_item(&item, mw_api);
        ret
    }

    pub fn merge_from(&mut self, author2: &GenericAuthorInfo) {
        if self.name.is_none() {
            self.name = author2.name.clone();
        }
        if self.wikidata_item.is_none() {
            self.wikidata_item = author2.wikidata_item.clone();
        }
        if self.list_number.is_none() {
            self.list_number = author2.list_number.clone();
        }
        for (k, v) in &author2.prop2id {
            self.prop2id.insert(k.to_string(), v.to_string());
        }
        for name in &author2.alternative_names {
            self.alternative_names.push(name.to_string());
        }
        self.alternative_names.sort();
        self.alternative_names.dedup();
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

    pub fn compare(&self, author2: &GenericAuthorInfo) -> u16 {
        match (&self.wikidata_item, &author2.wikidata_item) {
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

        for (k, v) in &self.prop2id {
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
        match (&self.name, &author2.name) {
            (Some(n1), Some(n2)) => {
                ret += 50 * self.author_names_match(&n1.as_str(), &n2.as_str());
            }
            _ => {}
        }

        // List number
        match (&self.list_number, &author2.list_number) {
            (Some(n1), Some(n2)) => {
                if n1 == n2 {
                    ret += 30;
                }
            }
            _ => {}
        }

        ret
    }

    pub fn update_author_item(
        &self,
        entities: &entity_container::EntityContainer,
        mw_api: &mut mediawiki::api::Api,
    ) {
        let q = match &self.wikidata_item {
            Some(q) => q.to_string(),
            None => return,
        };
        let original_item = match entities.get_entity(q) {
            Some(i) => i.clone(),
            None => return,
        };
        let mut item = original_item.clone();
        self.amend_author_item(&mut item);

        let mut params = EntityDiffParams::none();
        params.labels.add = EntityDiffParamState::All;
        params.aliases.add = EntityDiffParamState::All;
        params.claims.add = EntityDiffParamState::All;
        let diff = EntityDiff::new(&original_item, &item, &params);
        if diff.is_empty() {
            return;
        }
        //println!("{}", diff.actions());
        let _new_json = diff.apply_diff(mw_api, &diff).unwrap();
        //EntityDiff::get_entity_id(&new_json);
        // TODO
    }
}
