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
        for author in authors {
            author.create_author_statement_in_paper_item(item);
        }
    }

    pub fn update_author_name_statement(
        &self,
        asn: &str,
        new_author: &GenericAuthorInfo,
        item: &mut Entity,
    ) {
        let author_q = new_author.wikidata_item.as_ref().unwrap();
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

        item.claims_mut()
            .par_iter_mut()
            .filter(|statement| statement.property() == "P2093")
            .filter_map(|statement| {
                let author = GenericAuthorInfo::new_from_statement(statement)?;
                Some((author, statement))
            })
            .for_each(|(author, p2093_statement)| {
                if let Some((candidate, _points)) = author.find_best_match(authors) {
                    match &authors[candidate].wikidata_item {
                        Some(q) => {
                            if p50.contains(q) {
                                // Strange, we already have this one, remove
                                if author.list_number.is_some()
                                    && author.list_number == authors[candidate].list_number
                                {
                                    println!(
                                        "REMOVING AUTHOR {:?}\nBECAUSE:\n{:?}\n{:?}",
                                        &p2093_statement, &author, &authors[candidate]
                                    );
                                    // Same list number, remove P2093
                                    Self::remove_p2093_statement(p2093_statement);
                                } else {
                                    println!("NOT REMOVING AUTHOR {:?}", &p2093_statement);
                                }
                            } else if let Some(p50_statement) =
                                &authors[candidate].generate_author_statement()
                            {
                                Self::update_p2093_to_p50_statement(p50_statement, p2093_statement);
                            }
                        }
                        None => {}
                    }
                }
            });

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

    fn merge_authors(
        &self,
        authors: &mut Vec<GenericAuthorInfo>,
        authors2: &Vec<GenericAuthorInfo>,
    ) {
        // Shortcut
        if authors.is_empty() {
            *authors = authors2.par_iter().cloned().collect();
            return;
        }

        for author in authors2.iter() {
            match author.find_best_match(authors) {
                Some((candidate, _points)) => match authors[candidate].merge_from(author) {
                    Ok(_) => {}
                    Err(e) => eprintln!("{:?}: {}", &author, e),
                },
                None => authors.push(author.clone()), // No match found, add the author
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
                .nth(0);
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
        let mut item = Entity::new_empty_item();
        item.add_claim(Statement::new_normal(
            Snak::new_item("P31", "Q13442814"),
            vec![],
            vec![],
        ));
        item
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

        // Preserve qualifiers
        p2093_statement.qualifiers().iter().for_each(|q1| {
            if !p50_statement
                .qualifiers()
                .iter()
                .any(|q2| q1.property() == q2.property())
            {
                p50_statement.add_qualifier_snak(q1.clone())
            }
        });

        // P1932 "object named as"
        if let Some(dv) = p2093_statement.main_snak().data_value() {
            if let Value::StringValue(author_name_string) = dv.value() {
                let p1932_qualifier = Snak::new_string("P1932", author_name_string);
                p50_statement.add_qualifier_snak(p1932_qualifier);
            }
        }

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
    //use super::*;
    //use wikibase::mediawiki::api::Api;

    /*
    TODO:
    pub fn new(cache: Arc<WikidataStringCache>) -> WikidataPapers {
    pub fn adapters_mut(&mut self) -> &mut Vec<Box<dyn ScientificPublicationAdapter>> {
    pub fn add_adapter(&mut self, adapter_box: Box<dyn ScientificPublicationAdapter>) {
    pub fn edit_summary(&self) -> &Option<String> {
    pub fn set_edit_summary(&mut self, edit_summary: Option<String>) {
    fn create_author_statements(&mut self, authors: &Vec<GenericAuthorInfo>, item: &mut Entity) {
    fn update_author_statements(&self, authors: &Vec<GenericAuthorInfo>, item: &mut Entity) {
    fn create_or_update_author_statements(
    fn merge_authors(
    pub fn update_item_from_adapters(
    fn update_author_items(&self, authors: &Vec<GenericAuthorInfo>, mw_api: &mut Api) {
    fn update_item_with_ids(&self, item: &mut wikibase::Entity, ids: &Vec<GenericWorkIdentifier>) {
    pub fn create_or_update_item_from_ids(
    pub fn create_or_update_item_from_q(
    fn create_or_update_item_from_items(
    pub fn update_from_paper_ids(
    pub fn get_items_for_ids(&self, ids: &Vec<GenericWorkIdentifier>) -> Vec<String> {
    */
}
