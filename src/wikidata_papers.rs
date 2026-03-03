use crate::generic_author_info::GenericAuthorInfo;
use crate::scientific_publication_adapter::ScientificPublicationAdapter;
use crate::wikidata_string_cache::WikidataStringCache;
use crate::*;
use rayon::prelude::*;
use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::RwLock;
use wikibase::mediawiki::api::Api;

use self::identifiers::GenericWorkIdentifier;
use self::identifiers::GenericWorkType;
use self::wikidata_interaction::WikidataInteraction;

pub type Spas = Box<dyn ScientificPublicationAdapter + Sync>;

lazy_static! {
    static ref SNAK_REMOVE_STATEMENT: Snak = Snak::new_no_value("P2093", SnakDataType::String);
}

pub struct EditResult {
    pub q: String,
    pub edited: bool,
}

pub struct WikidataPapers {
    adapters: Vec<Spas>,
    cache: Arc<WikidataStringCache>,
    edit_summary: Option<String>,
    pub testing: bool,
    entities: entity_container::EntityContainer,
}

impl WikidataInteraction for WikidataPapers {}

impl WikidataPapers {
    pub fn new(cache: Arc<WikidataStringCache>) -> WikidataPapers {
        let mut entities = entity_container::EntityContainer::new();
        entities.allow_special_entity_data(false);
        WikidataPapers {
            adapters: vec![],
            cache,
            edit_summary: None,
            testing: false,
            entities,
        }
    }

    pub fn adapters_mut(&mut self) -> &mut Vec<Spas> {
        &mut self.adapters
    }

    pub fn add_adapter(&mut self, adapter_box: Spas) {
        self.adapters.push(adapter_box);
    }

    pub fn edit_summary(&self) -> &Option<String> {
        &self.edit_summary
    }

    pub fn set_edit_summary(&mut self, edit_summary: Option<String>) {
        self.edit_summary = edit_summary;
    }

    fn create_author_statements(&mut self, authors: &Vec<GenericAuthorInfo>, item: &mut Entity) {
        let mut seen_q_items: HashSet<String> = HashSet::new();
        let mut seen_names: HashSet<String> = HashSet::new();
        for author in authors {
            // Skip duplicate wikidata items (prevents duplicate P50 for same person)
            if let Some(q) = &author.wikidata_item {
                if !seen_q_items.insert(q.clone()) {
                    continue;
                }
            }
            // Skip duplicate author name strings for P2093
            if author.wikidata_item.is_none() {
                if let Some(name) = &author.name {
                    let key = GenericAuthorInfo::simplify_name(&name.to_lowercase());
                    if !key.is_empty() && !seen_names.insert(key) {
                        continue;
                    }
                }
            }
            author.create_author_statement_in_paper_item(item);
        }
    }

    pub fn update_author_name_statement(
        &self,
        asn: &str,
        new_author: &GenericAuthorInfo,
        item: &mut Entity,
    ) {
        let author_q = match new_author.wikidata_item.as_ref() {
            Some(q) => q,
            None => return,
        };
        if Self::get_p50s_from_item(item).contains(&format!("Q{}", author_q)) {
            return; // Had that author already
        }
        item.claims_mut()
            .par_iter_mut()
            .filter(|statement| statement.property() == "P2093")
            .filter_map(|statement| {
                let author = GenericAuthorInfo::new_from_statement(statement)?;
                Some((author, statement))
            })
            .filter(|(author, _statement)| author.name == Some(asn.to_string()))
            .for_each(|(_author, p2093_statement)| {
                let p50_statement = match &new_author.generate_author_statement() {
                    Some(p50_statement) => p50_statement.to_owned(),
                    None => return,
                };
                Self::update_p2093_to_p50_statement(&p50_statement, p2093_statement);
            });
        Self::remove_statements_with_no_value(item);
    }

    fn update_author_statements(&self, authors: &[GenericAuthorInfo], item: &mut Entity) {
        let p50 = Self::get_p50s_from_item(item);
        let mut used_candidates: HashSet<usize> = HashSet::new();

        for statement in item.claims_mut().iter_mut() {
            if statement.property() != "P2093" {
                continue;
            }
            let author = match GenericAuthorInfo::new_from_statement(statement) {
                Some(a) => a,
                None => continue,
            };
            let (candidate, _points) = match author.find_best_match(authors) {
                Some(m) => m,
                None => continue,
            };
            let q = match &authors[candidate].wikidata_item {
                Some(q) => q,
                None => continue,
            };
            if p50.contains(q) || used_candidates.contains(&candidate) {
                // Author already has P50 or candidate already used; remove redundant P2093
                Self::remove_p2093_statement(statement);
            } else if let Some(p50_statement) = &authors[candidate].generate_author_statement() {
                Self::update_p2093_to_p50_statement(p50_statement, statement);
                used_candidates.insert(candidate);
            }
        }

        Self::remove_statements_with_no_value(item);
    }

    pub fn create_or_update_author_statements(
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

    #[allow(clippy::ptr_arg)]
    fn merge_authors(
        &self,
        authors: &mut Vec<GenericAuthorInfo>,
        authors2: &Vec<GenericAuthorInfo>,
    ) {
        // Shortcut
        if authors.is_empty() {
            *authors = authors2.clone();
            return;
        }

        for author in authors2.iter() {
            match author.find_best_match(authors) {
                Some((candidate, _points)) => match authors[candidate].merge_from(author) {
                    Ok(_) => {}
                    Err(e) => eprintln!("{:?}: {}", &author, e),
                },
                None => {
                    // Only add if there's truly no overlap with any existing author.
                    // If there's a partial match (ambiguous), skip to avoid duplicates.
                    if !author.has_partial_match(authors) {
                        authors.push(author.clone());
                    }
                }
            }
        }
    }

    pub async fn update_item_from_adapters(
        &mut self,
        item: &mut Entity,
        adapter2work_id: &mut HashMap<usize, String>,
        mw_api: Arc<RwLock<Api>>,
    ) {
        let mut authors: Vec<GenericAuthorInfo> = vec![];
        for adapter_id in 0..self.adapters.len() {
            let publication_id = match self.adapters[adapter_id]
                .publication_id_from_item(item)
                .await
            {
                Some(id) => id,
                _ => continue,
            };

            let adapter = &mut self.adapters[adapter_id];
            adapter2work_id.insert(adapter_id, publication_id.clone());
            adapter
                .update_statements_for_publication_id_default(
                    &publication_id,
                    item,
                    // self.cache.clone(),
                )
                .await;
            adapter
                .update_statements_for_publication_id(&publication_id, item)
                .await;

            // Authors
            let authors2 = adapter.get_author_list(&publication_id);
            self.merge_authors(&mut authors, &authors2);
        }

        // Set P31 (instance of) based on work type from adapters, if not already set.
        // Adapters like Crossref can determine the correct type (book, article, etc.)
        if !item.has_claims_with_property("P31") {
            let work_type_q = adapter2work_id
                .iter()
                .find_map(|(adapter_id, pub_id)| self.adapters[*adapter_id].get_work_type(pub_id))
                .unwrap_or_else(|| "Q13442814".to_string()); // default: scientific article
            item.add_claim(Statement::new_normal(
                Snak::new_item("P31", &work_type_q),
                vec![],
                vec![],
            ));
        }

        // Final deduplication pass after all sources have been merged
        GenericAuthorInfo::deduplicate(&mut authors);

        let mut new_authors: Vec<GenericAuthorInfo> = vec![];
        for author in authors {
            let r = author
                .get_or_create_author_item(mw_api.clone(), self.cache.clone(), false)
                .await;
            new_authors.push(r);
        }
        self.update_author_items(&new_authors, mw_api.clone()).await;
        self.create_or_update_author_statements(item, &new_authors);
    }

    pub async fn update_author_items(
        &mut self,
        authors: &Vec<GenericAuthorInfo>,
        mw_api: Arc<RwLock<Api>>,
    ) {
        let qs: Vec<String> = authors
            .iter()
            .filter_map(|a| a.wikidata_item.clone())
            .collect();
        if qs.is_empty() {
            return;
        }

        let api = mw_api.read().await;
        if self.entities.load_entities(&api, &qs).await.is_err() {
            return;
        }
        drop(api);

        for author in authors {
            author
                .update_author_item(&mut self.entities, mw_api.clone())
                .await;
        }
    }

    fn update_item_with_ids(&self, item: &mut wikibase::Entity, ids: &Vec<GenericWorkIdentifier>) {
        for id in ids {
            let prop = match id.work_type() {
                GenericWorkType::Property(prop) => prop.to_owned(),
                _ => continue,
            };
            if item.has_claims_with_property(prop.as_str()) {
                // TODO use claims_with_property to check the individual values
                continue;
            }
            let id2statement = self
                .adapters
                .iter()
                .filter(|adapter| adapter.publication_property().is_some())
                .filter(|adapter| Some(prop.to_owned()) == adapter.publication_property())
                .filter_map(|adapter| adapter.publication_id_for_statement(id.id()))
                .next();
            if let Some(id) = id2statement {
                item.add_claim(Statement::new_normal(
                    Snak::new_external_id(prop.as_str(), &id),
                    vec![],
                    vec![],
                ))
            }
        }
    }

    pub async fn create_or_update_item_from_ids(
        &mut self,
        mw_api: Arc<RwLock<Api>>,
        ids: &Vec<GenericWorkIdentifier>,
    ) -> Option<EditResult> {
        if ids.is_empty() {
            return None;
        }
        let items = match self.testing {
            true => vec![],
            false => self.get_items_for_ids(ids).await,
        };
        self.create_or_update_item_from_items(mw_api, ids, &items)
            .await
    }

    pub async fn create_or_update_item_from_q(
        &mut self,
        mw_api: Arc<RwLock<Api>>,
        q: &str,
    ) -> Option<EditResult> {
        let items = vec![q.to_owned()];
        self.create_or_update_item_from_items(mw_api, &vec![], &items)
            .await
    }

    fn new_publication_item(&self) -> Entity {
        Entity::new_empty_item()
    }

    async fn create_or_update_item_from_items(
        &mut self,
        mw_api: Arc<RwLock<Api>>,
        ids: &Vec<GenericWorkIdentifier>,
        items: &[String],
    ) -> Option<EditResult> {
        let mut item: wikibase::Entity;
        let mut original_item = Entity::new_empty_item();
        match items.first() {
            Some(q) => {
                let api = mw_api.read().await;
                item = self
                    .entities
                    .load_entity(&api, q.clone())
                    .await
                    .ok()?
                    .to_owned();
                drop(api);
                original_item = item.clone();
            }
            None => item = self.new_publication_item(),
        }

        self.update_item_with_ids(&mut item, ids);

        let mut adapter2work_id = HashMap::new();
        self.update_item_from_adapters(&mut item, &mut adapter2work_id, mw_api.clone())
            .await;

        // Paranoia
        if item.claims().len() < 4 {
            println!("Skipping {:?}", &ids);
            return None;
        }

        self.apply_diff_for_item(original_item, item, mw_api).await
    }

    pub async fn apply_diff_for_item(
        &mut self,
        original_item: Entity,
        item: Entity,
        mw_api: Arc<RwLock<Api>>,
    ) -> Option<EditResult> {
        let mut params = EntityDiffParams::none();
        params.labels.add = EntityDiffParamState::All;
        params.aliases.add = EntityDiffParamState::All;
        params.claims.add = EntityDiffParamState::All;
        params.claims.remove = EntityDiffParamState::some(&vec!["P2093"]);
        params.references.list = vec![(
            EntityDiffParamState::All,
            EntityDiffParamState::except(&vec!["P813"]),
        )];
        let mut diff = EntityDiff::new(&original_item, &item, &params);
        diff.set_edit_summary(self.edit_summary.to_owned());

        if diff.is_empty() {
            return match original_item.id().as_str() {
                "" => None,
                id => Some(EditResult {
                    q: id.to_string(),
                    edited: false,
                }),
            };
        }

        if self.testing {
            println!("{}", diff.to_string_pretty().unwrap());
            None
        } else {
            let mut api = mw_api.write().await;
            let new_json = diff.apply_diff(&mut api, &diff).await.ok()?;
            let q = EntityDiff::get_entity_id(&new_json)?;
            Some(EditResult {
                q: q.to_string(),
                edited: true,
            })
        }
    }

    pub async fn update_from_paper_ids(
        &mut self,
        original_ids: &[GenericWorkIdentifier],
    ) -> Vec<GenericWorkIdentifier> {
        let mut ids: HashSet<GenericWorkIdentifier> = HashSet::new();
        original_ids
            .iter()
            .filter(|id| id.is_legit())
            .for_each(|id| {
                ids.insert(id.to_owned());
            });
        loop {
            let last_id_size = ids.len();
            for adapter_id in 0..self.adapters.len() {
                let adapter = &mut self.adapters[adapter_id];
                let vids: Vec<GenericWorkIdentifier> = ids.par_iter().cloned().collect();
                //println!("Adapter {}", adapter.name());
                adapter
                    .get_identifier_list(&vids)
                    .await
                    .iter()
                    .filter(|id| id.is_legit())
                    .for_each(|id| {
                        ids.insert(id.clone());
                    });
            }
            if last_id_size == ids.len() {
                break;
            }
        }
        ids.par_iter().filter(|id| id.is_legit()).cloned().collect()
    }

    pub async fn get_items_for_ids(&self, ids: &Vec<GenericWorkIdentifier>) -> Vec<String> {
        let mut items: Vec<String> = vec![];
        for id in ids {
            let r = match id.work_type() {
                GenericWorkType::Property(prop) => self.cache.get(prop.as_str(), id.id()).await,
                GenericWorkType::Item => Some(id.id().to_owned()),
            };
            if let Some(q) = r {
                items.push(q)
            }
        }
        items.sort();
        items.dedup();
        items
    }

    pub fn entities_mut(&mut self) -> &mut entity_container::EntityContainer {
        &mut self.entities
    }

    fn get_p50s_from_item(item: &mut Entity) -> Vec<String> {
        let p50: Vec<String> = item
            .claims()
            .par_iter()
            .filter(|statement| statement.property() == "P50")
            .filter_map(|statement| match statement.main_snak().data_value() {
                Some(dv) => match dv.value() {
                    Value::Entity(entity) => Some(entity.id().to_string()),
                    _ => None,
                },
                _ => None,
            })
            .collect();
        p50
    }

    fn update_p2093_to_p50_statement(p50_statement: &Statement, p2093_statement: &mut Statement) {
        let mut p50_statement = p50_statement.to_owned();

        // Preserve qualifiers from the existing P2093 statement (e.g. P1545 ordinal).
        // P2093 qualifiers take precedence over the generated P50's qualifiers,
        // because the P2093 reflects what's already on Wikidata.
        let p2093_properties: HashSet<String> = p2093_statement
            .qualifiers()
            .iter()
            .map(|q| q.property().to_string())
            .collect();
        // Build merged qualifiers: start with P2093's, then add non-conflicting P50 ones.
        // Always exclude P1932 from the P50's qualifiers — we'll add the correct one
        // from the P2093's actual name string below.
        let mut merged_qualifiers: Vec<Snak> = p2093_statement.qualifiers().to_vec();
        for q in p50_statement.qualifiers() {
            if q.property() != "P1932" && !p2093_properties.contains(q.property()) {
                merged_qualifiers.push(q.clone());
            }
        }

        // P1932 "object named as" — always use the P2093's original author name string,
        // not the adapter's version (which may differ in accents, formatting, etc.)
        if let Some(dv) = p2093_statement.main_snak().data_value() {
            if let Value::StringValue(author_name_string) = dv.value() {
                merged_qualifiers.push(Snak::new_string("P1932", author_name_string));
            }
        }

        // Rebuild the P50 statement with merged qualifiers
        p50_statement = Statement::new_normal(
            p50_statement.main_snak().clone(),
            merged_qualifiers,
            p50_statement.references().to_vec(),
        );

        // Preserve references
        let references = p2093_statement.references().clone();
        // println!("{p2093_statement:?} =>\n{p50_statement:?}\n");
        *p2093_statement = p50_statement;
        p2093_statement.set_references(references);
    }

    fn remove_p2093_statement(p2093_statement: &mut Statement) {
        // HACK change to "no value", then remove downstream
        p2093_statement.set_main_snak(SNAK_REMOVE_STATEMENT.to_owned());
    }

    fn remove_statements_with_no_value(item: &mut Entity) {
        // Remove no-value P2093s
        item.claims_mut()
            .retain(|statement| *statement.main_snak() != *SNAK_REMOVE_STATEMENT);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: create a P2093 statement with name and ordinal qualifier P1545
    fn make_p2093(name: &str, ordinal: &str) -> Statement {
        Statement::new_normal(
            Snak::new_string("P2093", name),
            vec![Snak::new_string("P1545", ordinal)],
            vec![],
        )
    }

    /// Helper: create a WikidataPapers with no adapters (for unit testing)
    async fn make_wdp() -> WikidataPapers {
        let mw_api = Arc::new(tokio::sync::RwLock::new(
            wikibase::mediawiki::api::Api::new("https://www.wikidata.org/w/api.php")
                .await
                .unwrap(),
        ));
        let cache = Arc::new(WikidataStringCache::new(mw_api));
        WikidataPapers::new(cache)
    }

    /// Extract P1545 qualifier value from a statement
    fn get_ordinal(statement: &Statement) -> Option<String> {
        statement.qualifiers().iter().find_map(|q| {
            if q.property() == "P1545" {
                match q.data_value() {
                    Some(dv) => match dv.value() {
                        Value::StringValue(s) => Some(s.to_string()),
                        _ => None,
                    },
                    None => None,
                }
            } else {
                None
            }
        })
    }

    // === Bug reproduction: ordinal swapping (issue #3) ===
    //
    // When a P2093 "Giorgio Alimonti" at ordinal #42 is matched to adapter
    // author with the same name but ordinal #44, the resulting P50 should
    // keep ordinal #42 (from Wikidata), not #44 (from the adapter).

    #[tokio::test]
    async fn update_author_statements_preserves_p2093_ordinal() {
        let wdp = make_wdp().await;
        let mut item = Entity::new_empty_item();
        item.add_claim(make_p2093("Giorgio Alimonti", "42"));

        // Adapter author with same name but different ordinal
        let mut adapter_author = GenericAuthorInfo::new();
        adapter_author.name = Some("Giorgio Alimonti".to_string());
        adapter_author.wikidata_item = Some("Q64863661".to_string());
        adapter_author.list_number = Some("44".to_string()); // different ordinal!

        wdp.update_author_statements(&[adapter_author], &mut item);

        // Should have exactly one claim: a P50 for Q64863661
        assert_eq!(item.claims().len(), 1, "Expected exactly one claim");
        let claim = &item.claims()[0];
        assert_eq!(claim.main_snak().property(), "P50", "Should be P50");

        // The ordinal should be #42 (from the original P2093), NOT #44 (from the adapter)
        let ordinal = get_ordinal(claim);
        assert_eq!(
            ordinal,
            Some("42".to_string()),
            "Ordinal should be preserved from the original P2093 statement, not taken from adapter"
        );
    }

    // === Bug reproduction: duplicate Q-item assignment (issue #3) ===
    //
    // If two P2093 statements for name variants of the same person both
    // match the same adapter author, only one P50 should be created and
    // the other P2093 should be removed.

    #[tokio::test]
    async fn update_author_statements_prevents_duplicate_p50() {
        let wdp = make_wdp().await;
        let mut item = Entity::new_empty_item();
        // Two P2093s for what is effectively the same person (different name orderings)
        item.add_claim(make_p2093("Giorgio Alimonti", "42"));
        item.add_claim(make_p2093("Alimonti Giorgio", "44"));

        let mut adapter_author = GenericAuthorInfo::new();
        adapter_author.name = Some("Giorgio Alimonti".to_string());
        adapter_author.wikidata_item = Some("Q64863661".to_string());
        adapter_author.list_number = Some("42".to_string());

        wdp.update_author_statements(&[adapter_author], &mut item);

        // Count how many P50 statements reference Q64863661
        let p50_count = item
            .claims()
            .iter()
            .filter(|s| s.property() == "P50")
            .filter(|s| match s.main_snak().data_value() {
                Some(dv) => matches!(dv.value(), Value::Entity(e) if e.id() == "Q64863661"),
                None => false,
            })
            .count();

        assert!(
            p50_count <= 1,
            "Same Q-item should not appear in multiple P50 statements, got {}",
            p50_count
        );
    }

    // === Bug reproduction: conflicting ordinals (issue #3) ===
    //
    // When adapter gives two different authors the same ordinal but Wikidata
    // has them at different ordinals, the Wikidata ordinals should be preserved.

    #[tokio::test]
    async fn update_author_statements_no_conflicting_ordinals() {
        let wdp = make_wdp().await;
        let mut item = Entity::new_empty_item();
        // Two different authors at different positions on Wikidata
        item.add_claim(make_p2093("Jahred Adelman", "15"));
        item.add_claim(make_p2093("Stephanie Zimmermann", "2879"));

        // Adapter has both authors with Q-items but erroneously gives same ordinal
        let mut author1 = GenericAuthorInfo::new();
        author1.name = Some("Jahred Adelman".to_string());
        author1.wikidata_item = Some("Q100001".to_string());
        author1.list_number = Some("15".to_string());

        let mut author2 = GenericAuthorInfo::new();
        author2.name = Some("Stephanie Zimmermann".to_string());
        author2.wikidata_item = Some("Q100002".to_string());
        author2.list_number = Some("15".to_string()); // adapter erroneously gives same ordinal

        wdp.update_author_statements(&[author1, author2], &mut item);

        // Collect ordinals from all claims
        let ordinals: Vec<String> = item.claims().iter().filter_map(get_ordinal).collect();

        // Ordinals should not conflict - each should be unique
        let mut unique = ordinals.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(
            ordinals.len(),
            unique.len(),
            "Ordinals should be unique, but got duplicates: {:?}",
            ordinals
        );
    }

    /// Extract P1932 ("object named as") qualifier value from a statement
    fn get_named_as(statement: &Statement) -> Option<String> {
        statement.qualifiers().iter().find_map(|q| {
            if q.property() == "P1932" {
                match q.data_value() {
                    Some(dv) => match dv.value() {
                        Value::StringValue(s) => Some(s.to_string()),
                        _ => None,
                    },
                    None => None,
                }
            } else {
                None
            }
        })
    }

    // === Bug reproduction: P1932 uses adapter name instead of P2093 name (issue #5) ===
    //
    // When converting P2093 "José García" to P50, the P1932 ("object named as")
    // qualifier should preserve the original P2093 name, not the adapter's
    // version (which may differ in accents, formatting, etc.).

    #[tokio::test]
    async fn update_author_statements_p1932_preserves_original_name() {
        let wdp = make_wdp().await;
        let mut item = Entity::new_empty_item();
        // P2093 with accented name as it appears on Wikidata
        item.add_claim(make_p2093("José García", "5"));

        // Adapter has ASCII-ified version of the name
        let mut adapter_author = GenericAuthorInfo::new();
        adapter_author.name = Some("Jose Garcia".to_string()); // different from Wikidata!
        adapter_author.wikidata_item = Some("Q12345".to_string());
        adapter_author.list_number = Some("5".to_string());

        wdp.update_author_statements(&[adapter_author], &mut item);

        assert_eq!(item.claims().len(), 1);
        let claim = &item.claims()[0];
        assert_eq!(claim.main_snak().property(), "P50");

        // P1932 should preserve the original P2093 name "José García",
        // NOT the adapter's "Jose Garcia"
        let named_as = get_named_as(claim);
        assert_eq!(
            named_as,
            Some("José García".to_string()),
            "P1932 should preserve the original author name string from Wikidata"
        );
    }

    // Same issue but with a completely different adapter name (e.g. transliteration)
    #[tokio::test]
    async fn update_author_statements_p1932_preserves_original_name_transliteration() {
        let wdp = make_wdp().await;
        let mut item = Entity::new_empty_item();
        item.add_claim(make_p2093("Smith, John A.", "1"));

        // Adapter reformats the name
        let mut adapter_author = GenericAuthorInfo::new();
        adapter_author.name = Some("John Smith".to_string()); // reordered, no middle initial
        adapter_author.wikidata_item = Some("Q99999".to_string());
        adapter_author.list_number = Some("1".to_string());

        wdp.update_author_statements(&[adapter_author], &mut item);

        assert_eq!(item.claims().len(), 1);
        let claim = &item.claims()[0];
        assert_eq!(claim.main_snak().property(), "P50");

        // P1932 must be "Smith, John A." (the original), not "John Smith" (adapter)
        let named_as = get_named_as(claim);
        assert_eq!(
            named_as,
            Some("Smith, John A.".to_string()),
            "P1932 should preserve the original author name string from Wikidata"
        );
    }

    // === Bug reproduction: wrong P31 for non-article works (issue #14) ===

    #[tokio::test]
    async fn new_publication_item_has_no_p31() {
        // new_publication_item should NOT hardcode P31, so that adapters
        // can set it based on the actual work type.
        let wdp = make_wdp().await;
        let item = wdp.new_publication_item();
        assert!(
            !item.has_claims_with_property("P31"),
            "new_publication_item should not set P31; it should be set later based on work type"
        );
    }
}
