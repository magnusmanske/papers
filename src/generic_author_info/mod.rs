use std::{collections::HashMap, sync::Arc};

use anyhow::{anyhow, Result};
use deunicode::deunicode;
use regex::Regex;
use tokio::sync::RwLock;
use wikibase::mediawiki::api::Api;

use crate::{
    wikidata_interaction::WikidataInteraction, wikidata_string_cache::WikidataStringCache, *,
};

const SCORE_LIST_NUMBER: u16 = 5;
const SCORE_LIST_NUMBER_AND_NAME: u16 = 30;
const SCORE_NAME_MATCH: u16 = 50;
const SCORE_PROP_MATCH: u16 = 90;
const SCORE_ITEM_MATCH: u16 = 100;
const SCORE_MATCH_MIN: u16 = 51;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct GenericAuthorInfo {
    name: Option<String>,
    prop2id: HashMap<String, String>,
    wikidata_item: Option<String>,
    list_number: Option<String>,
    alternative_names: Vec<String>,
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
                        _ => {},
                    }
                }
            }
        }

        Some(ret)
    }

    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    pub fn set_name(&mut self, name: Option<String>) {
        self.name = name;
    }

    pub fn wikidata_item(&self) -> Option<&str> {
        self.wikidata_item.as_deref()
    }

    pub fn set_wikidata_item(&mut self, q: Option<String>) {
        self.wikidata_item = q;
    }

    pub fn list_number(&self) -> Option<&str> {
        self.list_number.as_deref()
    }

    pub fn set_list_number(&mut self, n: Option<String>) {
        self.list_number = n;
    }

    pub fn prop2id(&self) -> &HashMap<String, String> {
        &self.prop2id
    }

    pub fn prop2id_mut(&mut self) -> &mut HashMap<String, String> {
        &mut self.prop2id
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

    /// Returns true if there's a meaningful partial match with any author in
    /// the list. Used to detect ambiguous matches that didn't meet the
    /// threshold for a definitive match. Requires more than just a list
    /// number coincidence (score > SCORE_LIST_NUMBER).
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
            },
            None => {
                if name.is_empty() && qualifiers.is_empty() {
                    return None; // No addition
                }
                Statement::new_normal(Snak::new_string("P2093", &name), qualifiers, vec![])
            },
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
    /// Picks the best label for this author. Considers `self.name` plus
    /// each entry in `alternative_names`, ranks them by name-readability
    /// score (tie-break: longest), and returns the winner. Returns
    /// `None` only when there are no usable names.
    fn pick_best_label(&self) -> Option<String> {
        let mut all_names: Vec<&str> = Vec::new();
        if let Some(name) = &self.name {
            if !name.is_empty() {
                all_names.push(name);
            }
        }
        for n in &self.alternative_names {
            if !n.is_empty() && !all_names.contains(&n.as_str()) {
                all_names.push(n);
            }
        }
        all_names
            .into_iter()
            .max_by_key(|n| (Self::name_readability_score(n), n.len()))
            .map(|s| s.to_string())
    }

    pub fn amend_author_item(&self, item: &mut Entity) {
        // Set the best name as label if the item doesn't already have
        // one. Previously this also pushed alternative_names as Wikidata
        // aliases, gated by an `add_aliases()` const fn that always
        // returned `false` ("Seems to go wrong more than it goes right").
        // The dead alias-push branches were removed as P3 polish; the
        // `alternative_names` field is still populated by `merge_from`
        // because tests observe it, but it no longer flows into the item.
        if let Some(name) = self.pick_best_label() {
            if item.label_in_locale("en").is_none() {
                item.set_label(LocaleString::new("en", &name));
            }
        }

        item.descriptions_mut().push(LocaleString::new("en", "researcher"));

        // Human
        if !item.has_target_entity("P31", "Q5") {
            item.add_claim(Statement::new_normal(Snak::new_item("P31", "Q5"), vec![], vec![]));
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

        // Create new item and use its ID. On API error we degrade to
        // `None` so the outer best-effort flow can still proceed; the
        // caller's "no item ID" branch will pick it up.
        ret.wikidata_item = match self.create_item(&item, mw_api).await {
            Ok(opt) => opt,
            Err(e) => {
                tracing::warn!(error = %e, "create_item failed for author");
                None
            },
        };

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
            self.alternative_names.push(name.to_owned()); // Sort/dedup at the
                                                          // end
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
            tracing::warn!(
                kept = ?self.list_number,
                discarded = ?author2.list_number,
                name = ?self.name,
                "merge_from: conflicting list numbers, keeping higher-priority value",
            );
        }
        for (k, v) in &author2.prop2id {
            match self.prop2id.get(k) {
                Some(x) => {
                    if x != v {
                        // Keep existing value from higher-priority source
                        tracing::warn!(
                            property = %k,
                            kept = %x,
                            discarded = %v,
                            name = ?self.name,
                            "merge_from: conflicting property values, keeping higher-priority value",
                        );
                    }
                },
                None => {
                    self.prop2id.insert(k.to_string(), v.to_string());
                },
            }
        }
        self.alternative_names.extend(author2.alternative_names.iter().cloned());
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

    /// Collects all long (3+ char) words from a pre-processed name string,
    /// sorted.
    fn sorted_name_parts(re: &Regex, s: &str) -> Vec<String> {
        let mut parts: Vec<String> = re.captures_iter(s).map(|c| c[1].to_string()).collect();
        parts.sort();
        parts
    }

    /// Returns true if two pre-processed (asciified, dots replaced) name
    /// strings have conflicting initials after removing shared long words.
    /// E.g., "ck clarke" vs "jenny clarke" → true (c,k vs j conflict)
    /// "j smith" vs "john smith" → false (j matches j)
    /// "heinrich manske" vs "manske heinrich" → false (no remaining parts)
    fn names_have_conflicting_initials(name1_mod: &str, name2_mod: &str) -> bool {
        lazy_static! {
            static ref RE_LONG_C: Regex = Regex::new(r"\b(\w{3,})\b")
                .expect("names_have_conflicting_initials: could not compile RE_LONG_C");
            static ref RE_ALL_C: Regex = Regex::new(r"\b(\w+)\b")
                .expect("names_have_conflicting_initials: could not compile RE_ALL_C");
        }
        let all1: Vec<String> =
            RE_ALL_C.captures_iter(name1_mod).map(|c| c[1].to_string()).collect();
        let all2: Vec<String> =
            RE_ALL_C.captures_iter(name2_mod).map(|c| c[1].to_string()).collect();
        let parts1 = Self::sorted_name_parts(&RE_LONG_C, name1_mod);
        let parts2 = Self::sorted_name_parts(&RE_LONG_C, name2_mod);
        let matched_words: Vec<String> =
            parts1.iter().filter(|p| parts2.contains(p)).cloned().collect();
        let remaining1 = Self::remove_matched_words(&all1, &matched_words);
        let remaining2 = Self::remove_matched_words(&all2, &matched_words);
        if !remaining1.is_empty() && !remaining2.is_empty() {
            let initials1 = Self::extract_initials_from_parts(&remaining1);
            let initials2 = Self::extract_initials_from_parts(&remaining2);
            return !initials1.iter().any(|c| initials2.contains(c));
        }
        false
    }

    /// Compares long (3+ characters) name parts, with initials compatibility
    /// check. Returns 0 if the non-matching parts have conflicting initials
    /// (e.g. "Bruce Allen" vs "G. Allen" — shared surname but B ≠ G).
    fn author_names_match(&self, name1: &str, name2: &str) -> u16 {
        lazy_static! {
            static ref RE_LONG: Regex = Regex::new(r"\b(\w{3,})\b")
                .expect("GenericAuthorInfo::author_names_match: could not compile RE_LONG");
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

        if Self::names_have_conflicting_initials(&name1_mod, &name2_mod) {
            return 0;
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
    /// Short parts (< 3 chars) contribute each character as a potential
    /// initial. Long parts (>= 3 chars) contribute only their first
    /// character.
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

    /// Author-similarity score. Composed of four independent slices:
    ///
    /// 1. **Item match (early return)**: if both authors carry a
    ///    `wikidata_item`, equality decides everything (no other signals
    ///    can override it).
    /// 2. **Shared external-id properties** (DOI, ORCID, etc.).
    /// 3. **Name match** via [`Self::author_names_match`], plus a 1-point
    ///    tie-breaker for exact match after accent normalisation.
    /// 4. **List-number match**, optionally bumped to a higher score
    ///    when the names also share their longest token AND don't have
    ///    conflicting initials.
    pub fn compare(&self, author2: &GenericAuthorInfo) -> u16 {
        if let Some(score) = self.score_item_match(author2) {
            return score;
        }
        self.score_props(author2) + self.score_name(author2) + self.score_list_number(author2)
    }

    /// Resolves the wikidata_item half of [`Self::compare`]: returns
    /// `Some(SCORE_ITEM_MATCH)` on equal Q-ids, `Some(0)` on different
    /// Q-ids (both authors have one but they disagree → forced
    /// not-a-match), and `None` when item identity is unknown for one
    /// or both sides (defer to the other signals).
    fn score_item_match(&self, other: &Self) -> Option<u16> {
        match (&self.wikidata_item, &other.wikidata_item) {
            (Some(q1), Some(q2)) if q1 == q2 => Some(SCORE_ITEM_MATCH),
            (Some(_), Some(_)) => Some(0),
            _ => None,
        }
    }

    /// One `SCORE_PROP_MATCH` per external-id property that both
    /// authors carry with the same value.
    fn score_props(&self, other: &Self) -> u16 {
        self.prop2id
            .iter()
            .filter(|(k, v)| other.prop2id.get(*k) == Some(*v))
            .map(|_| SCORE_PROP_MATCH)
            .sum()
    }

    /// `SCORE_NAME_MATCH × author_names_match(…)`, plus a +1 tie-breaker
    /// when the two names are character-equal after accent stripping.
    fn score_name(&self, other: &Self) -> u16 {
        let (Some(n1), Some(n2)) = (&self.name, &other.name) else {
            return 0;
        };
        let mut score = SCORE_NAME_MATCH * self.author_names_match(n1, n2);
        if self.asciify_string(n1) == self.asciify_string(n2) {
            score += 1;
        }
        score
    }

    /// `SCORE_LIST_NUMBER` for matching list numbers, bumped to
    /// `SCORE_LIST_NUMBER_AND_NAME` when the two names *also* share
    /// their longest token and don't have conflicting initials.
    fn score_list_number(&self, other: &Self) -> u16 {
        let (Some(n1), Some(n2)) = (&self.list_number, &other.list_number) else {
            return 0;
        };
        if n1 != n2 {
            return 0;
        }
        if !self.shares_longest_name_part(other) {
            return SCORE_LIST_NUMBER;
        }
        // Same longest token: bump unless the initials disagree.
        if self.has_conflicting_initials_with(other) {
            SCORE_LIST_NUMBER
        } else {
            SCORE_LIST_NUMBER_AND_NAME
        }
    }

    /// True if both authors have a `get_longest_name_part` (i.e. a
    /// ≥3-char token) and the two are equal.
    fn shares_longest_name_part(&self, other: &Self) -> bool {
        matches!(
            (self.get_longest_name_part(), other.get_longest_name_part()),
            (Some(l1), Some(l2)) if l1 == l2
        )
    }

    /// True when *both* authors have a name AND those names'
    /// (asciified, dot-stripped) initials disagree. False on any
    /// missing-name input, so callers can use it as "definitely
    /// conflicting" without needing to handle Option themselves.
    fn has_conflicting_initials_with(&self, other: &Self) -> bool {
        let (Some(n1), Some(n2)) = (&self.name, &other.name) else {
            return false;
        };
        let n1_mod = self.asciify_string(n1).replace('.', " ");
        let n2_mod = self.asciify_string(n2).replace('.', " ");
        Self::names_have_conflicting_initials(&n1_mod, &n2_mod)
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
mod tests;
