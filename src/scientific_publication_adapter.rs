use crate::generic_author_info::GenericAuthorInfo;
use crate::wikidata_string_cache::WikidataStringCache;
use crate::*;
use std::collections::HashMap;
use std::sync::Arc;
use wikibase::mediawiki::api::Api;

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

    /// Returns a lanuage item identifier, or None
    fn get_language_item(&self, _publication_id: &String) -> Option<String> {
        None
    }

    /// Returns a volume string, or None
    fn get_volume(&self, _publication_id: &String) -> Option<String> {
        None
    }

    /// Returns an issue string, or None
    fn get_issue(&self, _publication_id: &String) -> Option<String> {
        None
    }

    /// Returns the publication date, or None
    fn get_publication_date(
        &self,
        _publication_id: &String,
    ) -> Option<(u32, Option<u8>, Option<u8>)> {
        None
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

    fn do_cache_work(&mut self, _publication_id: &String) -> Option<String> {
        None
    }

    fn reference(&self) -> Vec<Reference> {
        // TODO
        vec![]
    }

    /// Returns the sanitized (if required) publication ID to put in a statement
    fn publication_id_for_statement(&self, id: &String) -> Option<String> {
        Some(id.to_string())
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
        cache: Arc<WikidataStringCache>,
    ) {
        self.update_work_item_with_title(publication_id, item);
        self.update_work_item_with_property(publication_id, item);
        self.update_work_item_with_journal(publication_id, item, cache);
        self.update_work_item_with_volume(publication_id, item);
        self.update_work_item_with_issue(publication_id, item);
        self.update_work_item_with_publication_date(publication_id, item);
        self.update_work_item_with_language(publication_id, item);
    }

    fn update_work_item_with_language(&self, publication_id: &String, item: &mut Entity) {
        if item.has_claims_with_property("P407") {
            return;
        }
        match self.get_language_item(publication_id) {
            Some(language_q) => item.add_claim(Statement::new_normal(
                Snak::new_item("P407", &language_q),
                vec![],
                self.reference(),
            )),
            None => {}
        }
    }

    fn update_work_item_with_volume(&self, publication_id: &String, item: &mut Entity) {
        if item.has_claims_with_property("P478") {
            return;
        }
        match self.get_volume(publication_id) {
            Some(volume) => item.add_claim(Statement::new_normal(
                Snak::new_string("P478", &volume),
                vec![],
                self.reference(),
            )),
            None => {}
        }
    }

    fn update_work_item_with_issue(&self, publication_id: &String, item: &mut Entity) {
        if item.has_claims_with_property("P433") {
            return;
        }
        match self.get_issue(publication_id) {
            Some(issue) => item.add_claim(Statement::new_normal(
                Snak::new_string("P433", &issue),
                vec![],
                self.reference(),
            )),
            None => {}
        }
    }

    fn update_work_item_with_publication_date(&self, publication_id: &String, item: &mut Entity) {
        if item.has_claims_with_property("P577") {
            return;
        }
        match self.get_publication_date(publication_id) {
            Some(pubdate) => {
                let statement = self.get_wb_time_from_partial(
                    "P577".to_string(),
                    pubdate.0,
                    pubdate.1,
                    pubdate.2,
                );
                item.add_claim(statement);
            }
            None => {}
        }
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
                .filter(|t| !self.titles_are_equal(t, &main_title))
                .for_each(|t| {
                    item.add_alias(LocaleString::new(language.to_string(), t.to_string()))
                });

            // Add P1476 (title)
            if !item.has_claims_with_property("P1476") {
                let label = item.label_in_locale(&language).map(|s| s.to_owned());
                match label {
                    Some(title) => item.add_claim(Statement::new_normal(
                        Snak::new_monolingual_text("P1476", &title, &language),
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
        cache: Arc<WikidataStringCache>,
    ) {
        if item.has_claims_with_property("P1433") {
            return;
        }
        match self.get_work_issn(publication_id) {
            Some(issn) => match cache.issn2q(&issn) {
                Some(q) => item.add_claim(Statement::new_normal(
                    Snak::new_item("P1433", &q),
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
                if !item.has_claims_with_property(prop.to_owned()) {
                    match self.publication_id_for_statement(publication_id) {
                        Some(pub_id) => {
                            item.add_claim(Statement::new_normal(
                                Snak::new_external_id(prop.to_string(), pub_id),
                                vec![],
                                self.reference(),
                            ));
                        }
                        None => {}
                    }
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

    /// Caches language ISO codes and their mapping to Wikidata items
    fn language2q(&self, language: &str) -> Option<String> {
        lazy_static! {
            static ref MW_API: Api = Api::new("https://www.wikidata.org/w/api.php").expect("ScientificPublicationAdapter::language2q: Could not get Wikidata API");
            static ref L2Q: HashMap<String, String> = MW_API
                .sparql_query("SELECT DISTINCT ?l ?q { ?q wdt:P31/wdt:P279* wd:Q20162172; (wdt:P219|wdt:P220) ?l }")
                .unwrap()["results"]["bindings"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|j| {
                    let l = j["l"]["value"].as_str()?;
                    let q = MW_API
                        .extract_entity_from_uri(j["q"]["value"].as_str()?)
                        .ok()?;
                    Some((l.to_string(), q.to_string()))
                })
                .collect();
        }
        L2Q.get(language).map(|s| s.to_string())
    }
}

#[cfg(test)]
mod tests {
    //use super::*;
    //use wikibase::mediawiki::api::Api;

    /*
    TODO:
    fn name(&self) -> &str;
    fn author_cache(&self) -> &HashMap<String, String>;
    fn author_cache_mut(&mut self) -> &mut HashMap<String, String>;
    fn publication_id_from_item(&mut self, item: &Entity) -> Option<String> {
    fn update_statements_for_publication_id(&self, publication_id: &String, item: &mut Entity);
    fn get_author_list(&mut self, _publication_id: &String) -> Vec<GenericAuthorInfo> {
    fn get_identifier_list(
    fn author_property(&self) -> Option<String> {
    fn publication_property(&self) -> Option<String> {
    fn topic_property(&self) -> Option<String> {
    fn get_work_issn(&self, _publication_id: &String) -> Option<String> {
    fn get_work_titles(&self, _publication_id: &String) -> Vec<LocaleString> {
    fn do_cache_work(&mut self, _publication_id: &String) -> Option<String> {
    fn reference(&self) -> Vec<Reference> {
    fn sanitize_author_name(&self, author_name: &String) -> String {
    fn update_statements_for_publication_id_default(
    fn titles_are_equal(&self, t1: &String, t2: &String) -> bool {
    fn update_work_item_with_title(&self, publication_id: &String, item: &mut Entity) {
    fn update_work_item_with_journal(
    fn update_work_item_with_property(&self, publication_id: &String, item: &mut Entity) {
    fn get_wb_time_from_partial(
    fn get_external_identifier_from_item(&self, item: &Entity, property: &str) -> Option<String> {
    fn set_author_cache_entry(&mut self, catalog_author_id: &String, q: &String) {
    fn get_author_item_from_cache(&self, catalog_author_id: &String) -> Option<&String> {
    fn author_cache_is_empty(&self) -> bool {
    fn update_author_item(
    fn language2q
    */
}
