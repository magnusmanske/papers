extern crate crossref;
extern crate lazy_static;
extern crate reqwest;
extern crate serde_json;

use crate::wikidata_interaction::WikidataInteraction;
use crate::wikidata_string_cache::WikidataStringCache;
use crate::*;
use regex::Regex;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use wikibase::mediawiki::api::Api;

const SCORE_LIST_NUMBER: u16 = 5;
const SCORE_LIST_NUMBER_AND_NAME: u16 = 30;
const SCORE_NAME_MATCH: u16 = 50;
const SCORE_PROP_MATCH: u16 = 90;
const SCORE_ITEM_MATCH: u16 = 100;
const SCORE_MATCH_MIN: u16 = 51;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct GenericAuthorInfo {
    pub name: Option<String>,
    pub prop2id: HashMap<String, String>,
    pub wikidata_item: Option<String>,
    pub list_number: Option<String>,
    pub alternative_names: Vec<String>,
}

impl WikidataInteraction for GenericAuthorInfo {}

impl GenericAuthorInfo {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn new_from_name_num(name: &str, num: usize) -> Self {
        Self {
            name: Some(name.to_string()),
            prop2id: HashMap::new(),
            wikidata_item: None,
            list_number: Some(num.to_string()),
            alternative_names: vec![],
        }
    }

    pub fn new_from_statement(statement: &Statement) -> Option<Self> {
        let mut ret = Self::new();
        if statement.property() == "P2093" {
            match statement.main_snak().data_value() {
                Some(dv) => match dv.value() {
                    Value::StringValue(name) => {
                        ret.name = Some(name.to_string());
                    }
                    _ => return None,
                },
                _ => return None,
            }
        } else if statement.property() == "P50" {
            match statement.main_snak().data_value() {
                Some(dv) => match dv.value() {
                    Value::Entity(entity) => {
                        ret.wikidata_item = Some(entity.id().to_string());
                    }
                    _ => return None,
                },
                _ => return None,
            }
        } else {
            return None;
        }

        statement
            .qualifiers()
            .iter()
            .for_each(|snak| match snak.property() {
                // List number
                "P1545" => match snak.data_value() {
                    Some(dv) => {
                        if let Value::StringValue(s) = dv.value() {
                            ret.list_number = Some(s.to_string())
                        }
                    }
                    None => {}
                },
                // Named as
                "P1932" => match snak.data_value() {
                    Some(dv) => {
                        if let Value::StringValue(s) = dv.value() {
                            ret.name = Some(s.to_string())
                        }
                    }
                    None => {}
                },
                _ => {}
            });

        Some(ret)
    }

    pub fn find_best_match(&self, authors: &[GenericAuthorInfo]) -> Option<(usize, u16)> {
        let mut best_candidate: usize = 0;
        let mut best_points: u16 = 0;
        let mut multiple_best: bool = false;
        for (candidate_id, author) in authors.iter().enumerate() {
            let points = self.compare(author);
            if points > best_points {
                best_points = points;
                best_candidate = candidate_id;
                multiple_best = false;
            } else if points == best_points && points > 0 {
                multiple_best = true;
            }
        }
        if multiple_best {
            return None;
        }
        if best_points < SCORE_MATCH_MIN {
            return None;
        }
        match best_points {
            0 => None,
            points => Some((best_candidate, points)),
        }
    }

    pub fn generate_author_statement(&self) -> Option<Statement> {
        let name = match &self.name {
            Some(s) => s.to_string(),
            None => "".to_string(),
        };
        let mut qualifiers: Vec<Snak> = vec![];
        if let Some(num) = &self.list_number {
            qualifiers.push(Snak::new_string("P1545", num));
        }
        let statement = match &self.wikidata_item {
            Some(q) => {
                if !name.is_empty() {
                    qualifiers.push(Snak::new_string("P1932", &name));
                }
                Statement::new_normal(Snak::new_item("P50", q), qualifiers, vec![])
            }
            None => {
                if name.is_empty() && qualifiers.is_empty() {
                    return None; // No addition
                }
                Statement::new_normal(Snak::new_string("P2093", &name), qualifiers, vec![])
            }
        };
        Some(statement)
    }

    pub fn create_author_statement_in_paper_item(&self, item: &mut Entity) {
        if let Some(statement) = self.generate_author_statement() {
            item.add_claim(statement);
        }
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

        item.descriptions_mut()
            .push(LocaleString::new("en", "researcher"));

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
            let existing = item.values_for_property(prop);
            let to_check = Value::StringValue(id.to_string());
            if existing.contains(&to_check) {
                continue;
            }
            let statement = Statement::new_normal(
                Snak::new_external_id(prop.to_string(), id.to_string()),
                vec![],
                vec![],
            );
            item.add_claim(statement);
        }
    }

    pub async fn get_or_create_author_item(
        &self,
        mw_api: Arc<RwLock<Api>>,
        cache: Arc<WikidataStringCache>,
        allow_no_external_ids: bool,
    ) -> GenericAuthorInfo {
        let mut ret = self.clone();
        // Already has item?
        if ret.wikidata_item.is_some() {
            return ret;
        }
        // No external IDs
        if !allow_no_external_ids && ret.prop2id.is_empty() {
            return ret;
        }

        // Use search
        for (prop, id) in &ret.prop2id {
            if let Some(q) = cache.get(prop, id).await {
                ret.wikidata_item = Some(q);
                return ret;
            }
        }

        // Labels/aliases
        let mut item = Entity::new_empty_item();
        ret.amend_author_item(&mut item);

        // Create new item and use its ID
        ret.wikidata_item = self.create_item(&item, mw_api).await;

        // Update external IDs cache
        for (prop, id) in &ret.prop2id {
            cache.set(prop, id, ret.wikidata_item.clone()).await;
        }
        ret
    }

    pub fn merge_from(&mut self, author2: &GenericAuthorInfo) -> Result<(), String> {
        if self.name.is_none() {
            self.name = author2.name.clone();
        } else if let Some(name) = &author2.name {
            self.alternative_names.push(name.to_owned()); // Sort/dedup at the end
        }
        if self.wikidata_item.is_none() {
            self.wikidata_item = author2.wikidata_item.clone();
        } else if author2.wikidata_item.is_none() {
        } else if self.wikidata_item != author2.wikidata_item {
            return Err(format!(
                "GenericAuthorInfo::merge_from: Different items {:?} and {:?}, skipping",
                self.wikidata_item, author2.wikidata_item
            ));
        }
        if self.list_number.is_none() {
            self.list_number = author2.list_number.clone();
        } else if author2.list_number.is_none() {
        } else if self.list_number != author2.list_number {
            return Err(format!(
                "GenericAuthorInfo::merge_from: Different list numbers {:?} and {:?}, skipping",
                self.list_number, author2.list_number
            ));
        }
        for (k, v) in &author2.prop2id {
            match self.prop2id.get(k) {
                Some(x) => {
                    if x != v {
                        return Err(format!("GenericAuthorInfo::merge_from: Different property {} values {} and {}, skipping",k,x,v));
                    }
                }
                None => {
                    self.prop2id.insert(k.to_string(), v.to_string());
                }
            }
        }
        for name in &author2.alternative_names {
            self.alternative_names.push(name.to_string());
        }
        self.alternative_names.sort();
        self.alternative_names.dedup();
        Ok(())
    }

    fn asciify_string(&self, s: &str) -> String {
        // As long as some sources insist on using ASCII only for names :-(
        s.to_lowercase()
            .replace('ä', "a")
            .replace('ö', "o")
            .replace('ü', "u")
            .replace(['á', 'à', 'â'], "a")
            .replace(['é', 'è'], "e")
            .replace('ñ', "n")
            .replace('ï', "i")
            .replace('ç', "c")
            .replace('ß', "ss")
    }

    /// Simplifies a name by removing short words
    pub fn simplify_name(s: &str) -> String {
        lazy_static! {
            static ref RE1: Regex = Regex::new(r"\b(\w{3,})\b")
                .expect("GenericAuthorInfo::simplify_name: could not compile RE1");
        }
        let mut ret = "".to_string();
        let name_mod = s.replace('.', " ");
        for cap in RE1.captures_iter(&name_mod) {
            ret.push_str(&cap[1]);
            ret.push(' ');
        }
        ret.trim().to_string()
    }

    /// Compares long (3+ characters) name parts
    fn author_names_match(&self, name1: &str, name2: &str) -> u16 {
        let mut ret = 0;
        lazy_static! {
            static ref RE1: Regex = Regex::new(r"\b(\w{3,})\b")
                .expect("GenericAuthorInfo::author_names_match: could not compile RE1");
        }
        let name1_mod = self.asciify_string(name1).replace('.', " ");
        let name2_mod = self.asciify_string(name2).replace('.', " ");
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
        if let (Some(q1), Some(q2)) = (&self.wikidata_item, &author2.wikidata_item) {
            if q1 == q2 {
                return SCORE_ITEM_MATCH; // This is it
            } else {
                return 0; // Different items
            }
        }

        let mut ret = 0;

        for (k, v) in &self.prop2id {
            if let Some(v2) = author2.prop2id.get(k) {
                if v == v2 {
                    ret += SCORE_PROP_MATCH;
                }
            }
        }

        // Name match
        if let (Some(n1), Some(n2)) = (&self.name, &author2.name) {
            ret += SCORE_NAME_MATCH * self.author_names_match(n1.as_str(), n2.as_str());
        }

        // List number
        if let (Some(n1), Some(n2)) = (&self.list_number, &author2.list_number) {
            if n1 == n2 {
                // Same list number
                // TODO: Check if this is a good idea
                let l1 = self.get_longest_name_part();
                let l2 = author2.get_longest_name_part();
                if l1.is_some() && l2.is_some() && l1 == l2 {
                    // Same longest name part
                    ret += SCORE_LIST_NUMBER_AND_NAME;
                } else {
                    ret += SCORE_LIST_NUMBER;
                }
            }
        }

        ret
    }

    fn get_longest_name_part(&self) -> Option<String> {
        let name = self.name.as_ref()?;
        let mut ret = "".to_string();
        let parts = name.split([' ', '.'].as_ref());
        parts.for_each(|part| {
            if part.len() > ret.len() {
                ret = part.to_string();
            }
        });
        if ret.len() < 3 {
            return None;
        }
        Some(ret)
    }

    pub async fn update_author_item(
        &self,
        entities: &mut wikibase::entity_container::EntityContainer,
        mw_api: Arc<RwLock<Api>>,
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

        let mut mw_api = mw_api.write().await;
        entities.apply_diff(&mut mw_api, &diff).await;

        // TODO what?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    //use wikibase::mediawiki::api::Api;

    #[test]
    fn asciify_string() {
        let ga = GenericAuthorInfo::new();
        assert_eq!(ga.asciify_string("äöüáàâéèñïçß"), "aouaaaeenicss");
    }

    #[test]
    fn author_names_match() {
        let ga = GenericAuthorInfo::new();
        assert_eq!(ga.author_names_match("Manske M", "Manske HM"), 1);
        assert_eq!(ga.author_names_match("Manske M", "HM Manske"), 1);
        assert_eq!(
            ga.author_names_match("Heinrich M Manske", "manske heinrich"),
            2
        );
        assert_eq!(
            ga.author_names_match("Notmyname M Manske", "Heinrich M Manske"),
            1
        );
    }

    #[test]
    fn compare_by_item() {
        let mut ga1 = GenericAuthorInfo::new();
        let mut ga2 = GenericAuthorInfo::new();
        assert_eq!(ga1.compare(&ga2), 0);
        ga1.wikidata_item = Some("Q1".to_string());
        assert_eq!(ga1.compare(&ga2), 0);
        ga2.wikidata_item = Some("Q1".to_string());
        assert_eq!(ga1.compare(&ga2), SCORE_ITEM_MATCH);
        ga1.wikidata_item = Some("Q2".to_string());
        assert_eq!(ga1.compare(&ga2), 0);
    }

    #[test]
    fn compare_by_props() {
        let mut ga1 = GenericAuthorInfo::new();
        let mut ga2 = GenericAuthorInfo::new();
        assert_eq!(ga1.compare(&ga2), 0);
        ga1.prop2id.insert("foo".to_string(), "bar".to_string());
        assert_eq!(ga1.compare(&ga2), 0);
        ga2.prop2id.insert("foo".to_string(), "bar".to_string());
        assert_eq!(ga1.compare(&ga2), SCORE_PROP_MATCH);
        ga1.prop2id.insert("foo2".to_string(), "bar2".to_string());
        ga2.prop2id.insert("foo2".to_string(), "bar2".to_string());
        assert_eq!(ga1.compare(&ga2), SCORE_PROP_MATCH * 2);
    }

    #[test]
    fn compare_by_name() {
        let mut ga1 = GenericAuthorInfo::new();
        let mut ga2 = GenericAuthorInfo::new();
        assert_eq!(ga1.compare(&ga2), 0);
        ga1.name = Some("Heinrich M Manske".to_string());
        assert_eq!(ga1.compare(&ga2), 0);
        ga2.name = Some("Manske Heinrich M".to_string());
        assert_eq!(ga1.compare(&ga2), SCORE_NAME_MATCH * 2);
        ga1.name = Some("Manske HM".to_string());
        assert_eq!(ga1.compare(&ga2), SCORE_NAME_MATCH);
    }

    #[test]
    fn compare_by_list_number() {
        let mut ga1 = GenericAuthorInfo::new();
        let mut ga2 = GenericAuthorInfo::new();
        assert_eq!(ga1.compare(&ga2), 0);
        ga1.list_number = Some("123".to_string());
        assert_eq!(ga1.compare(&ga2), 0);
        ga2.list_number = Some("123".to_string());
        assert_eq!(ga1.compare(&ga2), SCORE_LIST_NUMBER);
        ga1.name = Some("Foobar Baz".to_string());
        ga2.name = Some("Foobar B.".to_string());
        assert_eq!(
            ga1.compare(&ga2),
            SCORE_LIST_NUMBER_AND_NAME + SCORE_NAME_MATCH
        );
        ga1.name = None;
        ga2.name = None;
        ga1.list_number = Some("456".to_string());
        assert_eq!(ga1.compare(&ga2), 0);
    }

    #[test]
    fn create_author_statement_in_paper_item() {
        let mut item = Entity::new_empty_item();
        let ga = GenericAuthorInfo::new();
        ga.create_author_statement_in_paper_item(&mut item);
        assert!(item.claims().is_empty());

        let mut item = Entity::new_empty_item();
        let mut ga = GenericAuthorInfo::new();
        ga.name = Some("Magnus Manske".to_string());
        ga.create_author_statement_in_paper_item(&mut item);
        assert_eq!(item.claims().len(), 1);
        assert_eq!(item.claims()[0].main_snak().property(), "P2093");
        assert!(item.claims()[0].qualifiers().is_empty());

        ga.list_number = Some("123".to_string());
        let mut item = Entity::new_empty_item();
        ga.create_author_statement_in_paper_item(&mut item);
        assert_eq!(item.claims().len(), 1);
        assert_eq!(item.claims()[0].qualifiers().len(), 1);

        ga.wikidata_item = Some("Q12345".to_string());
        let mut item = Entity::new_empty_item();
        ga.create_author_statement_in_paper_item(&mut item);
        assert_eq!(item.claims().len(), 1);
        assert_eq!(item.claims()[0].main_snak().property(), "P50");
        let qualifiers = item.claims()[0].qualifiers();
        assert_eq!(qualifiers[0], Snak::new_string("P1545", "123"));
        assert_eq!(qualifiers[1], Snak::new_string("P1932", "Magnus Manske"));
    }

    #[test]
    fn amend_author_item() {
        let mut ga = GenericAuthorInfo::new();
        ga.name = Some("Magnus Manske".to_string());
        ga.alternative_names.push("HM Manske".to_string());
        ga.prop2id
            .insert("P496".to_string(), "1234-5678-1234-5678".to_string());
        let mut item = Entity::new_empty_item();
        ga.amend_author_item(&mut item);
        assert_eq!(item.label_in_locale("en"), Some("Magnus Manske"));
        assert_eq!(*item.aliases(), vec![LocaleString::new("en", "HM Manske")]);
        assert_eq!(*item.claims()[0].main_snak(), Snak::new_item("P31", "Q5"));
        assert_eq!(
            *item.claims()[1].main_snak(),
            Snak::new_item("P106", "Q1650915")
        );
        assert_eq!(
            *item.claims()[2].main_snak(),
            Snak::new_external_id("P496", "1234-5678-1234-5678")
        );
    }

    #[test]
    fn find_best_match() {
        let mut ga_main = GenericAuthorInfo::new();
        ga_main.name = Some("Magnus Manske".to_string());

        let mut vector: Vec<GenericAuthorInfo> = vec![];
        let mut ga = GenericAuthorInfo::new();
        ga.name = Some("Manske M".to_string());
        vector.push(ga);
        let mut ga = GenericAuthorInfo::new();
        ga.name = Some("Bar Fo".to_string());
        vector.push(ga);

        assert_eq!(ga_main.find_best_match(&vector), None);

        // Again

        let mut ga_main = GenericAuthorInfo::new();
        ga_main.name = Some("Magnus Manske".to_string());
        ga_main.list_number = Some(123.to_string());

        let mut vector: Vec<GenericAuthorInfo> = vec![];
        let mut ga = GenericAuthorInfo::new();
        ga.name = Some("Manske M".to_string());
        ga.list_number = Some(123.to_string());
        vector.push(ga);
        let mut ga = GenericAuthorInfo::new();
        ga.name = Some("Bar Fo".to_string());
        ga.list_number = Some(456.to_string());
        vector.push(ga);

        assert_eq!(
            ga_main.find_best_match(&vector),
            Some((0, SCORE_NAME_MATCH + SCORE_LIST_NUMBER))
        );

        // Again

        let mut ga_main = GenericAuthorInfo::new();
        ga_main.name = Some("Magnus Manske".to_string());
        ga_main.list_number = Some(123.to_string());

        let mut vector: Vec<GenericAuthorInfo> = vec![];
        let mut ga = GenericAuthorInfo::new();
        ga.name = Some("Manske M".to_string());
        ga.list_number = Some(456.to_string());
        vector.push(ga);
        let mut ga = GenericAuthorInfo::new();
        ga.name = Some("Bar Fo".to_string());
        ga.list_number = Some(123.to_string());
        vector.push(ga);

        assert_eq!(ga_main.find_best_match(&vector), None);
    }

    /*
    TODO:
    fn new_from_statement
    fn get_or_create_author_item(
    fn merge_from(&mut self, author2: &GenericAuthorInfo)
    fn update_author_item(
    */
}
