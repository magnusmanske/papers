use crate::generic_author_info::GenericAuthorInfo;
use crate::scientific_publication_adapter::ScientificPublicationAdapter;
use crate::wikidata_string_cache::WikidataStringCache;
use crate::*;
use mediawiki::api::Api;
use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::{Arc, Mutex};

pub struct EditResult {
    pub q: String,
    pub edited: bool,
}

pub struct WikidataPapers {
    adapters: Vec<Box<dyn ScientificPublicationAdapter>>,
    cache: Arc<Mutex<WikidataStringCache>>,
    edit_summary: Option<String>,
    pub testing: bool,
}

impl WikidataInteraction for WikidataPapers {}

impl WikidataPapers {
    pub fn new(cache: Arc<Mutex<WikidataStringCache>>) -> WikidataPapers {
        WikidataPapers {
            adapters: vec![],
            cache: cache,
            edit_summary: None,
            testing: false,
        }
    }

    pub fn adapters_mut(&mut self) -> &mut Vec<Box<dyn ScientificPublicationAdapter>> {
        &mut self.adapters
    }

    pub fn add_adapter(&mut self, adapter_box: Box<dyn ScientificPublicationAdapter>) {
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

    fn update_author_statements(&self, authors: &Vec<GenericAuthorInfo>, item: &mut Entity) {
        let p50: Vec<String> = item
            .claims()
            .iter()
            .filter(|statement| statement.property() == "P50")
            .filter_map(|statement| match statement.main_snak().data_value() {
                Some(dv) => match dv.value() {
                    Value::Entity(entity) => Some(entity.id().to_string()),
                    _ => None,
                },
                _ => None,
            })
            .collect();

        // HACK used as "remove" tag
        let snak_remove_statement = Snak::new_no_value("P2093", SnakDataType::String);

        item.claims_mut().iter_mut().for_each(|statement| {
            if statement.property() != "P2093" {
                return;
            }
            match GenericAuthorInfo::new_from_statement(statement) {
                Some(author) => {
                    match author.find_best_match(authors) {
                        Some((candidate, _points)) => {
                            match &authors[candidate].wikidata_item {
                                Some(q) => {
                                    if p50.contains(&q) {
                                        // Strange, we already have this one, remove
                                        if author.list_number.is_some()
                                            && author.list_number == authors[candidate].list_number
                                        {
                                            // Same list number, remove P2093
                                            // HACK change to "no value", then remove downstream
                                            statement.set_main_snak(snak_remove_statement.clone());
                                        }
                                    } else {
                                        match &authors[candidate].generate_author_statement() {
                                            Some(s) => {
                                                // Preserve references
                                                let references = statement.references().clone();
                                                *statement = s.clone();
                                                statement.set_references(references);
                                            }
                                            None => {}
                                        }
                                    }
                                }
                                None => {}
                            }
                        }
                        None => {}
                    }
                }
                None => {}
            }
        });

        // Remove no-value P2093s
        item.claims_mut()
            .retain(|statement| *statement.main_snak() != snak_remove_statement);
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
        // Shortcut
        if authors.is_empty() {
            authors2
                .iter()
                .for_each(|author| authors.push(author.clone()));
            return;
        }

        for author in authors2.iter() {
            match author.find_best_match(authors) {
                Some((candidate, _points)) => authors[candidate].merge_from(&author),
                None => authors.push(author.clone()), // No match found, add the author
            }
        }
    }

    pub fn update_item_from_adapters(
        &mut self,
        mut item: &mut Entity,
        adapter2work_id: &mut HashMap<usize, String>,
        mw_api: &mut Api,
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
                self.cache.clone(),
            );
            adapter.update_statements_for_publication_id(&publication_id, &mut item);

            // Authors
            let authors2 = adapter.get_author_list(&publication_id);
            self.merge_authors(&mut authors, &authors2);
        }

        let authors: Vec<GenericAuthorInfo> = authors
            .iter()
            .map(|author| author.get_or_create_author_item(mw_api, self.cache.clone()))
            .collect();

        self.update_author_items(&authors, mw_api);

        self.create_or_update_author_statements(&mut item, &authors);
    }

    fn update_author_items(&self, authors: &Vec<GenericAuthorInfo>, mw_api: &mut Api) {
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
            author.update_author_item(&mut entities, mw_api);
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
        mw_api: &mut Api,
        ids: &Vec<GenericWorkIdentifier>,
    ) -> Option<EditResult> {
        if ids.is_empty() {
            return None;
        }
        let items = match self.testing {
            true => vec![],
            false => self.get_items_for_ids(&ids),
        };
        self.create_or_update_item_from_items(mw_api, ids, &items)
    }

    pub fn create_or_update_item_from_q(
        &mut self,
        mw_api: &mut Api,
        q: &String,
    ) -> Option<EditResult> {
        let items = vec![q.to_owned()];
        self.create_or_update_item_from_items(mw_api, &vec![], &items)
    }

    fn create_or_update_item_from_items(
        &mut self,
        mw_api: &mut Api,
        ids: &Vec<GenericWorkIdentifier>,
        items: &Vec<String>,
    ) -> Option<EditResult> {
        let mut entities = entity_container::EntityContainer::new();
        let mut item: wikibase::Entity;
        let original_item: wikibase::Entity;
        match items.get(0) {
            Some(q) => {
                item = entities.load_entity(mw_api, q.clone()).ok()?.to_owned();
                original_item = item.clone();
            }
            None => {
                original_item = Entity::new_empty_item();
                item = Entity::new_empty_item();
                item.add_claim(Statement::new_normal(
                    Snak::new_item("P31", "Q13442814"),
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
        params.claims.remove = EntityDiffParamState::some(&vec!["P2093"]);
        params.references.list = vec![(
            EntityDiffParamState::All,
            EntityDiffParamState::except(&vec!["P813"]),
        )];
        let mut diff = EntityDiff::new(&original_item, &item, &params);

        //println!("{}", ::serde_json::to_string_pretty(&json!(item)).unwrap());

        if diff.is_empty() {
            diff.set_edit_summary(self.edit_summary.to_owned());
            match original_item.id().as_str() {
                "" => return None,
                id => {
                    return Some(EditResult {
                        q: id.to_string(),
                        edited: false,
                    })
                }
            }
        }

        if self.testing {
            println!("{}", diff.to_string_pretty().unwrap());
            None
        } else {
            let new_json = diff.apply_diff(mw_api, &diff).ok()?;
            let q = EntityDiff::get_entity_id(&new_json)?;
            Some(EditResult {
                q: q.to_string(),
                edited: true,
            })
        }
    }

    // ID keys need to be uppercase (e.g. "PMID","DOI")
    pub fn update_from_paper_ids(
        &mut self,
        original_ids: &Vec<GenericWorkIdentifier>,
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
                let vids: Vec<GenericWorkIdentifier> = ids.iter().map(|x| x.to_owned()).collect();
                //println!("Adapter {}", adapter.name());
                adapter
                    .get_identifier_list(&vids)
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
        ids.iter()
            .filter(|id| id.is_legit())
            .map(|x| x.to_owned())
            .collect()
    }

    pub fn get_items_for_ids(&self, ids: &Vec<GenericWorkIdentifier>) -> Vec<String> {
        let mut items: Vec<String> = ids
            .iter()
            .filter_map(|id| match &id.work_type {
                GenericWorkType::Property(prop) => self.cache.lock().unwrap().get(prop, &id.id),
                GenericWorkType::Item => Some(id.id.to_owned()),
            })
            .collect();
        items.sort();
        items.dedup();
        items
    }
}

#[cfg(test)]
mod tests {
    //use super::*;
    //use mediawiki::api::Api;

    /*
    TODO:
    pub fn new(cache: Arc<Mutex<WikidataStringCache>>) -> WikidataPapers {
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
