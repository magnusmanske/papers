use crate::wikidata_interaction::WikidataInteraction;
use crate::wikidata_string_cache::WikidataStringCache;
use crate::*;
use anyhow::{anyhow, Result};
use deunicode::deunicode;
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
        let dv = statement.main_snak().data_value().as_ref()?;
        match statement.property() {
            "P2093" => match dv.value() {
                Value::StringValue(name) => ret.name = Some(name.to_string()),
                _ => return None,
            },
            "P50" => match dv.value() {
                Value::Entity(entity) => ret.wikidata_item = Some(entity.id().to_string()),
                _ => return None,
            },
            _ => return None,
        }

        for snak in statement.qualifiers() {
            if let Some(dv) = snak.data_value().as_ref() {
                if let Value::StringValue(s) = dv.value() {
                    match snak.property() {
                        "P1545" => ret.list_number = Some(s.to_string()),
                        "P1932" => ret.name = Some(s.to_string()),
                        _ => {}
                    }
                }
            }
        }

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

    /// Returns true if there's a meaningful partial match with any author in the list.
    /// Used to detect ambiguous matches that didn't meet the threshold for a definitive match.
    /// Requires more than just a list number coincidence (score > SCORE_LIST_NUMBER).
    pub fn has_partial_match(&self, authors: &[GenericAuthorInfo]) -> bool {
        authors.iter().any(|a| self.compare(a) > SCORE_LIST_NUMBER)
    }

    /// Deduplicates a list of authors by merging entries that match.
    /// Earlier entries (from higher-priority sources) take precedence.
    pub fn deduplicate(authors: &mut Vec<GenericAuthorInfo>) {
        let mut i = authors.len();
        while i > 1 {
            i -= 1;
            let author_i = authors[i].clone();
            if let Some((j, _score)) = author_i.find_best_match(&authors[..i]) {
                if authors[j].merge_from(&author_i).is_ok() {
                    authors.remove(i);
                }
            }
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

    /// Scores how human-readable a name is. Higher is better.
    /// Prefers names with more fully spelled-out words (not initials).
    fn name_readability_score(name: &str) -> usize {
        name.split_whitespace()
            .filter(|w| {
                let trimmed = w.trim_matches('.');
                trimmed.len() >= 2
            })
            .count()
    }

    /// From all available names, pick the most human-readable one as label,
    /// and return the rest as aliases.
    fn pick_best_label(&self) -> (Option<String>, Vec<String>) {
        let mut all_names: Vec<String> = Vec::new();
        if let Some(name) = &self.name {
            if !name.is_empty() {
                all_names.push(name.clone());
            }
        }
        for n in &self.alternative_names {
            if !n.is_empty() && !all_names.contains(n) {
                all_names.push(n.clone());
            }
        }
        if all_names.is_empty() {
            return (None, vec![]);
        }
        // Pick the name with the highest readability score; break ties by length
        let best_idx = all_names
            .iter()
            .enumerate()
            .max_by_key(|(_, n)| (Self::name_readability_score(n), n.len()))
            .map(|(i, _)| i)
            .unwrap();
        let label = all_names.remove(best_idx);
        (Some(label), all_names)
    }

    pub fn amend_author_item(&self, item: &mut Entity) {
        let (best_label, aliases) = self.pick_best_label();

        // Set the best name as label, unless already set (then try alias)
        if let Some(name) = &best_label {
            match item.label_in_locale("en") {
                Some(s) => {
                    if s != name {
                        item.add_alias(LocaleString::new("en", name));
                    }
                }
                None => item.set_label(LocaleString::new("en", name)),
            }
        }

        item.descriptions_mut()
            .push(LocaleString::new("en", "researcher"));

        // Remaining names as aliases
        for n in &aliases {
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

    pub fn merge_from(&mut self, author2: &GenericAuthorInfo) -> Result<()> {
        if self.name.is_none() {
            self.name = author2.name.clone();
        } else if let Some(name) = &author2.name {
            self.alternative_names.push(name.to_owned()); // Sort/dedup at the end
        }
        if self.wikidata_item.is_none() {
            self.wikidata_item = author2.wikidata_item.clone();
        } else if author2.wikidata_item.is_none() {
        } else if self.wikidata_item != author2.wikidata_item {
            return Err(anyhow!(
                "GenericAuthorInfo::merge_from: Different items {:?} and {:?}, skipping",
                self.wikidata_item,
                author2.wikidata_item
            ));
        }
        if self.list_number.is_none() {
            self.list_number = author2.list_number.clone();
        } else if author2.list_number.is_some() && self.list_number != author2.list_number {
            // Keep existing list number from higher-priority source
            eprintln!(
                "GenericAuthorInfo::merge_from: Different list numbers {:?} and {:?} for {:?}, keeping {:?}",
                self.list_number, author2.list_number, self.name, self.list_number
            );
        }
        for (k, v) in &author2.prop2id {
            match self.prop2id.get(k) {
                Some(x) => {
                    if x != v {
                        // Keep existing value from higher-priority source
                        eprintln!(
                            "GenericAuthorInfo::merge_from: Different property {} values {} and {} for {:?}, keeping {}",
                            k, x, v, self.name, x
                        );
                    }
                }
                None => {
                    self.prop2id.insert(k.to_string(), v.to_string());
                }
            }
        }
        self.alternative_names
            .extend(author2.alternative_names.iter().cloned());
        self.alternative_names.sort();
        self.alternative_names.dedup();
        Ok(())
    }

    fn asciify_string(&self, s: &str) -> String {
        deunicode(s).to_lowercase()
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

    /// Collects all long (3+ char) words from a pre-processed name string, sorted.
    fn sorted_name_parts(re: &Regex, s: &str) -> Vec<String> {
        let mut parts: Vec<String> = re.captures_iter(s).map(|c| c[1].to_string()).collect();
        parts.sort();
        parts
    }

    /// Compares long (3+ characters) name parts, with initials compatibility check.
    /// Returns 0 if the non-matching parts have conflicting initials
    /// (e.g. "Bruce Allen" vs "G. Allen" — shared surname but B ≠ G).
    fn author_names_match(&self, name1: &str, name2: &str) -> u16 {
        lazy_static! {
            static ref RE_LONG: Regex = Regex::new(r"\b(\w{3,})\b")
                .expect("GenericAuthorInfo::author_names_match: could not compile RE_LONG");
            static ref RE_ALL: Regex = Regex::new(r"\b(\w+)\b")
                .expect("GenericAuthorInfo::author_names_match: could not compile RE_ALL");
        }
        let name1_mod = self.asciify_string(name1).replace('.', " ");
        let name2_mod = self.asciify_string(name2).replace('.', " ");
        if !RE_LONG.is_match(&name1_mod) || !RE_LONG.is_match(&name2_mod) {
            return 0;
        }
        let parts1 = Self::sorted_name_parts(&RE_LONG, &name1_mod);
        let parts2 = Self::sorted_name_parts(&RE_LONG, &name2_mod);
        let matching_count = parts1.iter().filter(|part| parts2.contains(*part)).count() as u16;
        if matching_count == 0 {
            return 0;
        }

        // Check for conflicting first-name initials.
        // After removing matched long words, if both names still have remaining parts,
        // their initials must overlap. Otherwise the names are for different people
        // (e.g. "Bruce Allen" vs "G. Allen" — shared surname but conflicting first initials).
        let all1: Vec<String> = RE_ALL
            .captures_iter(&name1_mod)
            .map(|c| c[1].to_string())
            .collect();
        let all2: Vec<String> = RE_ALL
            .captures_iter(&name2_mod)
            .map(|c| c[1].to_string())
            .collect();
        let matched_words: Vec<String> = parts1
            .iter()
            .filter(|p| parts2.contains(p))
            .cloned()
            .collect();
        let remaining1 = Self::remove_matched_words(&all1, &matched_words);
        let remaining2 = Self::remove_matched_words(&all2, &matched_words);
        if !remaining1.is_empty() && !remaining2.is_empty() {
            let initials1 = Self::extract_initials_from_parts(&remaining1);
            let initials2 = Self::extract_initials_from_parts(&remaining2);
            if !initials1.iter().any(|c| initials2.contains(c)) {
                return 0; // Conflicting initials
            }
        }

        matching_count
    }

    /// Removes one occurrence of each matched word from the parts list.
    fn remove_matched_words(all_parts: &[String], matched: &[String]) -> Vec<String> {
        let mut remaining = all_parts.to_vec();
        for m in matched {
            if let Some(pos) = remaining.iter().position(|p| p == m) {
                remaining.remove(pos);
            }
        }
        remaining
    }

    /// Extracts initials from name parts.
    /// Short parts (< 3 chars) contribute each character as a potential initial.
    /// Long parts (>= 3 chars) contribute only their first character.
    fn extract_initials_from_parts(parts: &[String]) -> Vec<char> {
        let mut initials = Vec::new();
        for part in parts {
            if part.len() < 3 {
                initials.extend(part.chars());
            } else if let Some(c) = part.chars().next() {
                initials.push(c);
            }
        }
        initials
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
            // Small bonus for exact name match (after accent normalization) to break ties
            if self.asciify_string(n1) == self.asciify_string(n2) {
                ret += 1;
            }
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
        let mut diff = EntityDiff::new(&original_item, &item, &params);
        diff.set_edit_summary(Some("(automated edit by SourceMD)".to_string()));
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

    // --- merge_from tests ---

    #[test]
    fn merge_from_basic_name_and_props() {
        let mut ga1 = GenericAuthorInfo::new();
        ga1.name = Some("John Smith".to_string());
        ga1.list_number = Some("1".to_string());

        let mut ga2 = GenericAuthorInfo::new();
        ga2.name = Some("J Smith".to_string());
        ga2.list_number = Some("1".to_string());
        ga2.prop2id
            .insert("P496".to_string(), "0000-1234-5678-9012".to_string());

        assert!(ga1.merge_from(&ga2).is_ok());
        assert_eq!(ga1.name, Some("John Smith".to_string())); // primary name kept
        assert!(ga1.alternative_names.contains(&"J Smith".to_string())); // secondary added as alias
        assert_eq!(
            ga1.prop2id.get("P496"),
            Some(&"0000-1234-5678-9012".to_string())
        ); // prop absorbed
        assert_eq!(ga1.list_number, Some("1".to_string()));
    }

    #[test]
    fn merge_from_conflicting_list_numbers_succeeds() {
        // Previously this would return Err; now it should succeed and keep existing number.
        let mut ga1 = GenericAuthorInfo::new();
        ga1.name = Some("John Smith".to_string());
        ga1.list_number = Some("1".to_string());

        let mut ga2 = GenericAuthorInfo::new();
        ga2.name = Some("John Smith".to_string());
        ga2.list_number = Some("3".to_string()); // Different position in another source

        assert!(ga1.merge_from(&ga2).is_ok());
        assert_eq!(ga1.list_number, Some("1".to_string())); // Existing kept
    }

    #[test]
    fn merge_from_conflicting_prop2id_succeeds_keeps_existing() {
        // Previously this would return Err; now it should succeed and keep existing value.
        let mut ga1 = GenericAuthorInfo::new();
        ga1.prop2id
            .insert("P496".to_string(), "0000-1111-2222-3333".to_string());

        let mut ga2 = GenericAuthorInfo::new();
        ga2.prop2id
            .insert("P496".to_string(), "9999-8888-7777-6666".to_string()); // Conflict
        ga2.prop2id
            .insert("P1053".to_string(), "A-1234-5678".to_string()); // New, no conflict

        assert!(ga1.merge_from(&ga2).is_ok());
        // Conflicting property: keep existing (higher-priority source)
        assert_eq!(
            ga1.prop2id.get("P496"),
            Some(&"0000-1111-2222-3333".to_string())
        );
        // Non-conflicting property: absorbed
        assert_eq!(ga1.prop2id.get("P1053"), Some(&"A-1234-5678".to_string()));
    }

    #[test]
    fn merge_from_different_wikidata_items_still_fails() {
        // Different Q-items means different people; this should remain an Err.
        let mut ga1 = GenericAuthorInfo::new();
        ga1.wikidata_item = Some("Q1".to_string());

        let mut ga2 = GenericAuthorInfo::new();
        ga2.wikidata_item = Some("Q2".to_string());

        assert!(ga1.merge_from(&ga2).is_err());
        assert_eq!(ga1.wikidata_item, Some("Q1".to_string())); // Unchanged after failure
    }

    #[test]
    fn merge_from_adopts_wikidata_item_if_missing() {
        let mut ga1 = GenericAuthorInfo::new();
        ga1.name = Some("John Smith".to_string());
        // ga1 has no wikidata_item

        let mut ga2 = GenericAuthorInfo::new();
        ga2.wikidata_item = Some("Q42".to_string());
        ga2.prop2id
            .insert("P496".to_string(), "0000-0001-2345-6789".to_string());

        assert!(ga1.merge_from(&ga2).is_ok());
        assert_eq!(ga1.wikidata_item, Some("Q42".to_string())); // Adopted from ga2
        assert_eq!(
            ga1.prop2id.get("P496"),
            Some(&"0000-0001-2345-6789".to_string())
        );
    }

    #[test]
    fn merge_from_adopts_list_number_if_missing() {
        let mut ga1 = GenericAuthorInfo::new();
        ga1.name = Some("Jane Doe".to_string());
        // No list_number (e.g., from ORCID)

        let mut ga2 = GenericAuthorInfo::new();
        ga2.name = Some("Jane Doe".to_string());
        ga2.list_number = Some("2".to_string());

        assert!(ga1.merge_from(&ga2).is_ok());
        assert_eq!(ga1.list_number, Some("2".to_string())); // Adopted
    }

    // --- compare exact name bonus tests ---

    #[test]
    fn compare_exact_name_scores_higher_than_partial() {
        // "John Smith" exactly matches "John Smith" → gets +1 bonus
        let mut ga_ref = GenericAuthorInfo::new();
        ga_ref.name = Some("John Smith".to_string());

        let mut ga_exact = GenericAuthorInfo::new();
        ga_exact.name = Some("John Smith".to_string());

        let mut ga_partial = GenericAuthorInfo::new();
        ga_partial.name = Some("J Smith".to_string()); // only "smith" matches, no exact bonus

        let score_exact = ga_ref.compare(&ga_exact);
        let score_partial = ga_ref.compare(&ga_partial);
        assert!(
            score_exact > score_partial,
            "Exact name match must outscore partial"
        );
        // 101 (2 words × 50 + 1 bonus) vs 50 (1 word match, no bonus)
        assert_eq!(score_exact, SCORE_NAME_MATCH * 2 + 1);
        assert_eq!(score_partial, SCORE_NAME_MATCH);
    }

    #[test]
    fn compare_exact_name_bonus_isolated() {
        // Names under 3 chars don't trigger word-matching but do trigger the exact bonus,
        // cleanly isolating the +1 effect.
        let mut ga_ref = GenericAuthorInfo::new();
        ga_ref.name = Some("Li".to_string()); // 2-char word: filtered by \w{3,} regex

        let mut ga_same = GenericAuthorInfo::new();
        ga_same.name = Some("Li".to_string());

        let mut ga_different = GenericAuthorInfo::new();
        ga_different.name = Some("Lo".to_string());

        assert_eq!(
            ga_ref.compare(&ga_same),
            1,
            "Only the +1 exact bonus contributes"
        );
        assert_eq!(ga_ref.compare(&ga_different), 0, "No match, no bonus");
    }

    #[test]
    fn compare_exact_name_bonus_applies_after_asciification() {
        // Accent-normalized names should get the exact name bonus
        let mut ga1 = GenericAuthorInfo::new();
        ga1.name = Some("Müller Hans".to_string());

        let mut ga_accent = GenericAuthorInfo::new();
        ga_accent.name = Some("Müller Hans".to_string()); // Identical

        let mut ga_ascii = GenericAuthorInfo::new();
        ga_ascii.name = Some("Muller Hans".to_string()); // ASCII equivalent

        // Both should get the exact match bonus (ü → u after asciification)
        assert_eq!(ga1.compare(&ga_accent), ga1.compare(&ga_ascii));
    }

    // --- find_best_match with exact name tiebreaker ---

    #[test]
    fn find_best_match_exact_name_breaks_surname_tie() {
        // Without list numbers (e.g., ORCID data), "Li Wang" and "Min Wang" both score
        // 50 via the shared surname "Wang". The exact name bonus should break the tie
        // and correctly identify "Li Wang" as the matching entry.
        let mut ga_new = GenericAuthorInfo::new();
        ga_new.name = Some("Li Wang".to_string());

        let ga_li = {
            let mut g = GenericAuthorInfo::new();
            g.name = Some("Li Wang".to_string());
            g
        };
        let ga_min = {
            let mut g = GenericAuthorInfo::new();
            g.name = Some("Min Wang".to_string());
            g
        };

        let result = ga_new.find_best_match(&[ga_li, ga_min]);
        // Should match ga_li (index 0) with score 51 (50 + 1 bonus)
        assert_eq!(result, Some((0, SCORE_NAME_MATCH + 1)));
    }

    #[test]
    fn find_best_match_still_returns_none_for_ambiguous_different_names() {
        // "Min Wang" vs ["Li Wang", "Min Wang"]: the exact name bonus distinguishes them.
        // "min" (3 chars) + "wang" (4 chars) → 2-word match = 100, + 1 bonus = 101 for "Min Wang".
        // "wang" only (1-word match, no bonus) = 50 for "Li Wang".
        let mut ga_new = GenericAuthorInfo::new();
        ga_new.name = Some("Min Wang".to_string());

        let ga_li = {
            let mut g = GenericAuthorInfo::new();
            g.name = Some("Li Wang".to_string());
            g
        };
        let ga_min = {
            let mut g = GenericAuthorInfo::new();
            g.name = Some("Min Wang".to_string());
            g
        };

        let result = ga_new.find_best_match(&[ga_li, ga_min]);
        // "min" is 3 chars so it counts; 2 word matches + bonus = 101
        assert_eq!(result, Some((1, SCORE_NAME_MATCH * 2 + 1)));
    }

    // --- has_partial_match tests ---

    #[test]
    fn has_partial_match_empty_list() {
        let ga = GenericAuthorInfo::new();
        assert!(!ga.has_partial_match(&[]));
    }

    #[test]
    fn has_partial_match_list_number_only_is_not_partial() {
        // Score of exactly SCORE_LIST_NUMBER (5) should NOT count as a partial match.
        // The check is strictly >, so 5 > 5 is false.
        let mut ga = GenericAuthorInfo::new();
        ga.list_number = Some("1".to_string());

        let mut other = GenericAuthorInfo::new();
        other.list_number = Some("1".to_string()); // Same position, no name overlap

        assert!(!ga.has_partial_match(&[other]));
    }

    #[test]
    fn has_partial_match_true_for_name_overlap() {
        // A shared surname (score 50) is a meaningful partial match.
        let mut ga = GenericAuthorInfo::new();
        ga.name = Some("J Smith".to_string());

        let mut other = GenericAuthorInfo::new();
        other.name = Some("John Smith".to_string());

        assert!(ga.has_partial_match(&[other]));
    }

    #[test]
    fn has_partial_match_true_for_prop_match() {
        // A matching external ID (score 90) is a strong partial match.
        let mut ga = GenericAuthorInfo::new();
        ga.prop2id
            .insert("P496".to_string(), "0000-0001-2345-6789".to_string());

        let mut other = GenericAuthorInfo::new();
        other
            .prop2id
            .insert("P496".to_string(), "0000-0001-2345-6789".to_string());

        assert!(ga.has_partial_match(&[other]));
    }

    #[test]
    fn has_partial_match_false_for_completely_different_authors() {
        let mut ga = GenericAuthorInfo::new();
        ga.name = Some("Li Wang".to_string());
        ga.list_number = Some("3".to_string());

        let other = GenericAuthorInfo::new_from_name_num("John Smith", 1);

        assert!(!ga.has_partial_match(&[other]));
    }

    #[test]
    fn has_partial_match_checks_all_candidates() {
        // Should return true even if only the second candidate is a partial match.
        let mut ga = GenericAuthorInfo::new();
        ga.name = Some("Jane Doe".to_string());

        let unrelated = GenericAuthorInfo::new_from_name_num("Bob Jones", 1);
        let mut matching = GenericAuthorInfo::new();
        matching.name = Some("J Doe".to_string()); // shares "doe"

        assert!(!ga.has_partial_match(std::slice::from_ref(&unrelated)));
        assert!(ga.has_partial_match(&[unrelated, matching]));
    }

    // --- deduplicate tests ---

    #[test]
    fn deduplicate_empty_list() {
        let mut authors: Vec<GenericAuthorInfo> = vec![];
        GenericAuthorInfo::deduplicate(&mut authors);
        assert!(authors.is_empty());
    }

    #[test]
    fn deduplicate_single_entry_unchanged() {
        let mut authors = vec![GenericAuthorInfo::new_from_name_num("John Smith", 1)];
        GenericAuthorInfo::deduplicate(&mut authors);
        assert_eq!(authors.len(), 1);
        assert_eq!(authors[0].name, Some("John Smith".to_string()));
    }

    #[test]
    fn deduplicate_distinct_authors_unchanged() {
        let mut authors = vec![
            GenericAuthorInfo::new_from_name_num("John Smith", 1),
            GenericAuthorInfo::new_from_name_num("Jane Doe", 2),
            GenericAuthorInfo::new_from_name_num("Bob Jones", 3),
        ];
        GenericAuthorInfo::deduplicate(&mut authors);
        assert_eq!(authors.len(), 3);
    }

    #[test]
    fn deduplicate_removes_exact_duplicate() {
        let mut authors = vec![
            GenericAuthorInfo::new_from_name_num("John Smith", 1),
            GenericAuthorInfo::new_from_name_num("John Smith", 1), // Exact duplicate
        ];
        GenericAuthorInfo::deduplicate(&mut authors);
        assert_eq!(authors.len(), 1);
        assert_eq!(authors[0].name, Some("John Smith".to_string()));
    }

    #[test]
    fn deduplicate_abbreviated_name_same_list_number() {
        // "John Smith"(1) and "J Smith"(1): score = 50 + 30 = 80 → merged.
        // Earlier entry's name is kept; later becomes an alternative.
        let mut authors = vec![
            GenericAuthorInfo::new_from_name_num("John Smith", 1),
            GenericAuthorInfo::new_from_name_num("J Smith", 1),
        ];
        GenericAuthorInfo::deduplicate(&mut authors);
        assert_eq!(authors.len(), 1);
        assert_eq!(authors[0].name, Some("John Smith".to_string()));
        assert!(authors[0]
            .alternative_names
            .contains(&"J Smith".to_string()));
    }

    #[test]
    fn deduplicate_absorbs_external_ids_from_duplicate() {
        // The later entry (lower priority) has an ORCID; after dedup it should be on the first.
        let first = GenericAuthorInfo::new_from_name_num("John Smith", 1);
        let mut second = GenericAuthorInfo::new_from_name_num("John Smith", 1);
        second
            .prop2id
            .insert("P496".to_string(), "0000-0001-2345-6789".to_string());

        let mut authors = vec![first, second];
        GenericAuthorInfo::deduplicate(&mut authors);
        assert_eq!(authors.len(), 1);
        assert_eq!(
            authors[0].prop2id.get("P496"),
            Some(&"0000-0001-2345-6789".to_string())
        );
    }

    #[test]
    fn deduplicate_preserves_first_entry_data() {
        // First entry's data must win; second entry's conflicting data is discarded.
        let mut first = GenericAuthorInfo::new_from_name_num("John Smith", 1);
        first
            .prop2id
            .insert("P496".to_string(), "FIRST-ORCID".to_string());

        let mut second = GenericAuthorInfo::new_from_name_num("John Smith", 1);
        second
            .prop2id
            .insert("P496".to_string(), "SECOND-ORCID".to_string()); // Conflict: first wins

        let mut authors = vec![first, second];
        GenericAuthorInfo::deduplicate(&mut authors);
        assert_eq!(authors.len(), 1);
        assert_eq!(
            authors[0].prop2id.get("P496"),
            Some(&"FIRST-ORCID".to_string())
        );
    }

    #[test]
    fn deduplicate_tolerates_different_list_numbers_when_names_match_strongly() {
        // Two authors with the same full name but different list numbers (from sources that
        // disagree on ordering) should be merged; the first entry's list number is kept.
        let first = GenericAuthorInfo::new_from_name_num("Heinrich Manske", 1);
        let mut second = GenericAuthorInfo::new_from_name_num("Heinrich Manske", 3); // shifted position
        second
            .prop2id
            .insert("P496".to_string(), "0000-0001-2345-6789".to_string());

        let mut authors = vec![first.clone(), second];
        GenericAuthorInfo::deduplicate(&mut authors);
        assert_eq!(authors.len(), 1);
        assert_eq!(authors[0].list_number, first.list_number); // First's list number kept
        assert!(authors[0].prop2id.contains_key("P496")); // ORCID absorbed
    }

    // --- initials conflict detection tests ---

    #[test]
    fn author_names_match_rejects_conflicting_first_initials() {
        let ga = GenericAuthorInfo::new();
        // Cases from the false-positive Wikidata edit:
        assert_eq!(ga.author_names_match("Bruce Allen", "G. Allen"), 0);
        assert_eq!(
            ga.author_names_match("Jonathan Anderson", "S. B. Anderson"),
            0
        );
        assert_eq!(ga.author_names_match("Carl Blair", "D. G. Blair"), 0);
        assert_eq!(ga.author_names_match("R Gustafson", "E. K. Gustafson"), 0);
        assert_eq!(ga.author_names_match("Andrew M. Hopkins", "P. Hopkins"), 0);
        assert_eq!(ga.author_names_match("Bryn Jones", "D. I. Jones"), 0);
    }

    #[test]
    fn author_names_match_allows_compatible_initials() {
        let ga = GenericAuthorInfo::new();
        // Initial matches first letter of full name
        assert_eq!(ga.author_names_match("John Smith", "J. Smith"), 1);
        assert_eq!(ga.author_names_match("J Smith", "John Smith"), 1);
        // Multiple initials, one matches
        assert_eq!(ga.author_names_match("John Smith", "J. A. Smith"), 1);
    }

    #[test]
    fn author_names_match_no_conflict_when_one_side_fully_matched() {
        let ga = GenericAuthorInfo::new();
        // "Heinrich Manske" vs "manske heinrich" — all long words match, no remaining parts
        assert_eq!(
            ga.author_names_match("Heinrich Manske", "manske heinrich"),
            2
        );
    }

    #[test]
    fn conflicting_initials_prevents_false_match_with_list_number() {
        // "Bruce Allen"(5) vs "G. Allen"(5): should NOT match even with same list number.
        let mut ga1 = GenericAuthorInfo::new();
        ga1.name = Some("Bruce Allen".to_string());
        ga1.list_number = Some("5".to_string());

        let mut ga2 = GenericAuthorInfo::new();
        ga2.name = Some("G. Allen".to_string());
        ga2.list_number = Some("5".to_string());

        let score = ga1.compare(&ga2);
        assert!(
            score < SCORE_MATCH_MIN,
            "Conflicting initials must not produce a match (score {} >= {})",
            score,
            SCORE_MATCH_MIN
        );
    }

    /*
    TODO:
    fn new_from_statement
    fn get_or_create_author_item(
    fn update_author_item(
    */
}
