use crate::scientific_publication_adapter::ScientificPublicationAdapter;
use crate::*;
use async_trait::async_trait;
use chrono::prelude::*;
use crossref::Crossref;
use std::collections::HashMap;

use self::identifiers::{GenericWorkIdentifier, GenericWorkType, IdProp};

pub struct Crossref2Wikidata {
    author_cache: HashMap<String, String>,
    work_cache: HashMap<String, crossref::Work>,
}

impl Default for Crossref2Wikidata {
    fn default() -> Self {
        Self::new()
    }
}

impl Crossref2Wikidata {
    pub fn new() -> Self {
        Crossref2Wikidata {
            author_cache: HashMap::new(),
            work_cache: HashMap::new(),
        }
    }

    fn get_client(&self) -> crossref::Crossref {
        Crossref::builder()
            .build()
            .expect("Crossref2Wikidata::new: Could not build Crossref client")
    }

    pub fn get_cached_publication_from_id(&self, publication_id: &str) -> Option<&crossref::Work> {
        self.work_cache.get(publication_id)
    }

    fn add_identifiers_from_cached_publication(
        &mut self,
        publication_id: &str,
        ret: &mut Vec<GenericWorkIdentifier>,
    ) {
        let work = match self.get_cached_publication_from_id(publication_id) {
            Some(w) => w,
            None => return,
        };

        if !work.doi.is_empty() {
            ret.push(GenericWorkIdentifier::new_prop(IdProp::DOI, &work.doi));
        }
    }

    fn should_add_string(&self, s: &str) -> bool {
        if s == "n/a" || s == "n/a-n/a" {
            return false;
        }
        true
    }

    /// Maps a Crossref work type string to a Wikidata Q-item for P31 (instance of).
    fn crossref_type_to_q(type_: &str) -> Option<&'static str> {
        match type_ {
            "journal-article" => Some("Q13442814"),       // scientific article
            "book" | "edited-book" | "reference-book" => Some("Q571"), // book
            "monograph" => Some("Q193495"),                // monograph
            "book-chapter" | "book-section" => Some("Q1980247"), // chapter
            "proceedings-article" => Some("Q23927052"),    // conference paper
            "proceedings" => Some("Q1143604"),              // proceedings
            "dissertation" => Some("Q187685"),             // doctoral thesis
            "posted-content" => Some("Q580922"),           // preprint
            "dataset" => Some("Q1172284"),                 // dataset
            "report" | "report-series" => Some("Q10870555"), // report
            "standard" => Some("Q317623"),                 // standard
            "peer-review" => Some("Q7161778"),             // peer review (the article)
            _ => None,
        }
    }
}

#[async_trait(?Send)]
impl ScientificPublicationAdapter for Crossref2Wikidata {
    fn name(&self) -> &str {
        "Crossref2Wikidata"
    }

    fn get_work_type(&self, publication_id: &str) -> Option<String> {
        let work = self.get_cached_publication_from_id(publication_id)?;
        Self::crossref_type_to_q(&work.type_).map(|s| s.to_string())
    }

    fn get_work_issn(&self, publication_id: &str) -> Option<String> {
        match self.get_cached_publication_from_id(publication_id) {
            Some(work) => match &work.issn {
                Some(array) => match array.len() {
                    0 => None,
                    _ => Some(array[0].clone()),
                },
                None => None,
            },
            None => None,
        }
    }

    fn author_cache(&self) -> &HashMap<String, String> {
        &self.author_cache
    }

    fn author_cache_mut(&mut self) -> &mut HashMap<String, String> {
        &mut self.author_cache
    }

    async fn get_identifier_list(
        &mut self,
        ids: &[GenericWorkIdentifier],
    ) -> Vec<GenericWorkIdentifier> {
        let dois: Vec<String> = ids
            .iter()
            .filter_map(|id| {
                if let GenericWorkType::Property(prop) = &id.work_type() {
                    if *prop == IdProp::DOI {
                        return Some(id.id().to_string());
                    }
                }
                None
            })
            .collect();
        let futures: Vec<_> = dois
            .iter()
            .map(|doi| {
                let client = self.get_client();
                let doi = doi.clone();
                async move { client.work(&doi).await.ok() }
            })
            .collect();
        for work in futures::future::join_all(futures).await.into_iter().flatten() {
            self.work_cache.insert(work.doi.clone(), work);
        }
        let mut ret = vec![];
        for doi in &dois {
            self.add_identifiers_from_cached_publication(doi, &mut ret);
        }
        ret
    }

    async fn publication_id_from_item(&mut self, item: &Entity) -> Option<String> {
        let doi = self.get_external_identifier_from_item(item, &IdProp::DOI)?;
        let work = match self.get_client().work(&doi).await {
            Ok(w) => w,
            _ => return None, // No such work
        };

        let publication_id = doi;
        self.work_cache.insert(publication_id.clone(), work);
        Some(publication_id)
    }

    fn reference(&self) -> Vec<Reference> {
        let now = Utc::now().format("+%Y-%m-%dT00:00:00Z").to_string();
        vec![Reference::new(vec![Snak::new_time("P813", &now, 11)])]
    }

    fn get_work_titles(&self, publication_id: &str) -> Vec<LocaleString> {
        match self.get_cached_publication_from_id(publication_id) {
            Some(work) => work
                .title
                .iter()
                .map(|t| LocaleString::new("en", t))
                .collect(),
            None => vec![],
        }
    }

    async fn update_statements_for_publication_id(&self, publication_id: &str, item: &mut Entity) {
        let work = match self.get_cached_publication_from_id(publication_id) {
            Some(w) => w,
            None => return,
        };

        // Date
        if !item.has_claims_with_property("P577") {
            let j = json!(work.issued);
            if let Some(dp) = j["date-parts"][0].as_array() {
                if !dp.is_empty() {
                    if let Some(year) = dp[0].as_u64() {
                        let month: Option<u8> = match dp.len() {
                            1 => None,
                            _ => dp[1].as_u64().map(|x| x as u8),
                        };
                        let day: Option<u8> = match dp.len() {
                            3 => dp[2].as_u64().map(|x| x as u8),
                            _ => None,
                        };
                        let statement = self.get_wb_time_from_partial(
                            "P577".to_string(),
                            year as u32,
                            month,
                            day,
                        );
                        item.add_claim(statement);
                    }
                }
            }
        }

        // Issue/volume/page
        let string_options = vec![
            ("P433", &work.issue),
            ("P478", &work.volume),
            ("P304", &work.page),
        ];
        for option in string_options {
            if !item.has_claims_with_property(option.0) {
                if let Some(v) = option.1 {
                    if self.should_add_string(v) {
                        item.add_claim(Statement::new_normal(
                            Snak::new_string(option.0, v),
                            vec![],
                            self.reference(),
                        ));
                    }
                }
            }
        }

        if let Some(subjects) = &work.subject {
            for _subject in subjects {
                //println!("Subject:{}", &subject);
                // TODO
            }
        }

        // TODO journal (already done via ISSN?)
        // TODO ISBN
        // TODO authors
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_crossref_type_to_q_journal_article() {
        assert_eq!(
            Crossref2Wikidata::crossref_type_to_q("journal-article"),
            Some("Q13442814")
        );
    }

    #[test]
    fn test_crossref_type_to_q_book() {
        assert_eq!(Crossref2Wikidata::crossref_type_to_q("book"), Some("Q571"));
        assert_eq!(
            Crossref2Wikidata::crossref_type_to_q("edited-book"),
            Some("Q571")
        );
        assert_eq!(
            Crossref2Wikidata::crossref_type_to_q("reference-book"),
            Some("Q571")
        );
    }

    #[test]
    fn test_crossref_type_to_q_monograph() {
        assert_eq!(
            Crossref2Wikidata::crossref_type_to_q("monograph"),
            Some("Q193495")
        );
    }

    #[test]
    fn test_crossref_type_to_q_chapter() {
        assert_eq!(
            Crossref2Wikidata::crossref_type_to_q("book-chapter"),
            Some("Q1980247")
        );
        assert_eq!(
            Crossref2Wikidata::crossref_type_to_q("book-section"),
            Some("Q1980247")
        );
    }

    #[test]
    fn test_crossref_type_to_q_unknown_returns_none() {
        assert_eq!(
            Crossref2Wikidata::crossref_type_to_q("unknown-type"),
            None
        );
    }

    // === should_add_string ===

    #[test]
    fn should_add_string_rejects_na() {
        let c = Crossref2Wikidata::new();
        assert!(!c.should_add_string("n/a"));
        assert!(!c.should_add_string("n/a-n/a"));
    }

    #[test]
    fn should_add_string_accepts_valid_strings() {
        let c = Crossref2Wikidata::new();
        assert!(c.should_add_string("1"));
        assert!(c.should_add_string("some-value"));
        assert!(c.should_add_string("N/A")); // case-sensitive: uppercase is accepted
    }

    #[test]
    fn should_add_string_accepts_empty_string() {
        let c = Crossref2Wikidata::new();
        assert!(c.should_add_string(""));
    }

    #[test]
    fn test_crossref_type_to_q_proceedings() {
        assert_eq!(
            Crossref2Wikidata::crossref_type_to_q("proceedings-article"),
            Some("Q23927052")
        );
        assert_eq!(
            Crossref2Wikidata::crossref_type_to_q("proceedings"),
            Some("Q1143604")
        );
    }
}
