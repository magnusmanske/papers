use std::collections::HashMap;

use async_trait::async_trait;
use chrono::prelude::*;
use crossref::{response::work::PartialDate, Crossref};

use self::identifiers::{GenericWorkIdentifier, GenericWorkType, IdProp};
use crate::{
    adapter_helpers::{get_external_identifier_from_item, wb_time_from_partial},
    scientific_publication_adapter::{crossref_work_type_to_q, ScientificPublicationAdapter},
    *,
};

pub struct Crossref2Wikidata {
    author_cache: HashMap<String, String>,
    work_cache: HashMap<String, crossref::Work>,
    client: Crossref,
}

impl Default for Crossref2Wikidata {
    fn default() -> Self {
        // Share the bot-wide reqwest::Client with the Crossref SDK so the
        // adapter inherits the same connection pool, User-Agent, and
        // timeout config as the rest of `papers`. Both `http_client` and
        // `base_url` live on `CrossrefBuilder`, so we chain on the
        // builder before `.build()`.
        let client = Crossref::builder()
            .http_client(crate::http_client::http_client().clone())
            .build()
            .expect("Crossref2Wikidata::default: Crossref::builder().build() failed");
        Self::new_with_client(client)
    }
}

impl Crossref2Wikidata {
    pub fn new() -> Self {
        Self::default()
    }

    /// New adapter with a caller-provided SDK client. Tests construct
    /// a `Crossref::builder().base_url(mock.uri()).build()?` and pass
    /// it here; production goes through `Default::default()`.
    pub fn new_with_client(client: Crossref) -> Self {
        Crossref2Wikidata {
            author_cache: HashMap::new(),
            work_cache: HashMap::new(),
            client,
        }
    }

    pub fn get_cached_publication_from_id(&self, publication_id: &str) -> Option<&crossref::Work> {
        self.work_cache.get(publication_id)
    }

    fn should_add_string(&self, s: &str) -> bool {
        if s == "n/a" || s == "n/a-n/a" {
            return false;
        }
        true
    }
}

fn parse_crossref_date(issued: &PartialDate) -> Option<(u32, Option<u8>, Option<u8>)> {
    let j = json!(issued);
    let dp = j["date-parts"][0].as_array()?;
    if dp.is_empty() {
        return None;
    }
    let year = dp[0].as_u64()? as u32;
    let month = if dp.len() >= 2 {
        dp[1].as_u64().map(|x| x as u8)
    } else {
        None
    };
    let day = if dp.len() >= 3 {
        dp[2].as_u64().map(|x| x as u8)
    } else {
        None
    };
    Some((year, month, day))
}

#[async_trait(?Send)]
impl ScientificPublicationAdapter for Crossref2Wikidata {
    fn name(&self) -> &str {
        "Crossref2Wikidata"
    }

    fn get_work_type(&self, publication_id: &str) -> Option<String> {
        let work = self.get_cached_publication_from_id(publication_id)?;
        crossref_work_type_to_q(&work.type_).map(|s| s.to_string())
    }

    fn get_work_issn(&self, publication_id: &str) -> Option<String> {
        self.get_cached_publication_from_id(publication_id)?
            .issn
            .as_ref()
            .and_then(|a| a.first().cloned())
    }

    fn author_cache(&self) -> &HashMap<String, String> {
        &self.author_cache
    }

    fn author_cache_mut(&mut self) -> &mut HashMap<String, String> {
        &mut self.author_cache
    }

    fn has_cached_publication(&self, publication_id: &str) -> bool {
        self.get_cached_publication_from_id(publication_id).is_some()
    }

    fn extract_extra_ids(&self, publication_id: &str) -> Vec<GenericWorkIdentifier> {
        let Some(work) = self.get_cached_publication_from_id(publication_id) else {
            return vec![];
        };
        if work.doi.is_empty() {
            return vec![];
        }
        vec![GenericWorkIdentifier::new_prop(IdProp::DOI, &work.doi)]
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
                // Crossref derives Clone; the underlying reqwest::Client is
                // Arc-wrapped so this is a cheap refcount bump per future.
                let client = self.client.clone();
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
        let doi = get_external_identifier_from_item(item, &IdProp::DOI)?;
        let work = match self.client.work(&doi).await {
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
            Some(work) => work.title.iter().map(|t| LocaleString::new("en", t)).collect(),
            None => vec![],
        }
    }

    fn get_publication_date(&self, publication_id: &str) -> Option<(u32, Option<u8>, Option<u8>)> {
        let work = self.get_cached_publication_from_id(publication_id)?;
        parse_crossref_date(&work.issued)
    }

    async fn update_statements_for_publication_id(&self, publication_id: &str, item: &mut Entity) {
        let work = match self.get_cached_publication_from_id(publication_id) {
            Some(w) => w,
            None => return,
        };

        // Date
        if !item.has_claims_with_property("P577") {
            if let Some((year, month, day)) = self.get_publication_date(publication_id) {
                let statement = wb_time_from_partial("P577", year, month, day, self.reference());
                item.add_claim(statement);
            }
        }

        // Issue/volume/page
        let string_options =
            vec![("P433", &work.issue), ("P478", &work.volume), ("P304", &work.page)];
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
                // println!("Subject:{}", &subject);
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
    use crate::scientific_publication_adapter::crossref_work_type_to_q;

    #[test]
    fn test_crossref_type_to_q_journal_article() {
        assert_eq!(crossref_work_type_to_q("journal-article"), Some("Q13442814"));
    }

    #[test]
    fn test_crossref_type_to_q_book() {
        assert_eq!(crossref_work_type_to_q("book"), Some("Q571"));
        assert_eq!(crossref_work_type_to_q("edited-book"), Some("Q571"));
        assert_eq!(crossref_work_type_to_q("reference-book"), Some("Q571"));
    }

    #[test]
    fn test_crossref_type_to_q_monograph() {
        assert_eq!(crossref_work_type_to_q("monograph"), Some("Q193495"));
    }

    #[test]
    fn test_crossref_type_to_q_chapter() {
        assert_eq!(crossref_work_type_to_q("book-chapter"), Some("Q1980247"));
        assert_eq!(crossref_work_type_to_q("book-section"), Some("Q1980247"));
    }

    #[test]
    fn test_crossref_type_to_q_unknown_returns_none() {
        assert_eq!(crossref_work_type_to_q("unknown-type"), None);
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
        assert!(c.should_add_string("N/A")); // case-sensitive: uppercase is
                                             // accepted
    }

    #[test]
    fn should_add_string_accepts_empty_string() {
        let c = Crossref2Wikidata::new();
        assert!(c.should_add_string(""));
    }

    #[test]
    fn test_crossref_type_to_q_proceedings() {
        assert_eq!(crossref_work_type_to_q("proceedings-article"), Some("Q23927052"));
        assert_eq!(crossref_work_type_to_q("proceedings"), Some("Q1143604"));
    }

    // === P2-10b: SDK DI via base_url ======================================
    //
    // Crossref now uses reqwest 0.13 (matching papers), so production also
    // shares the bot-wide HTTP client via `.http_client(...)`. Tests
    // configure `base_url` on the builder.

    use wiremock::matchers::method as wm_method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn publication_id_from_item_routes_through_injected_base_url() {
        let server = MockServer::start().await;
        Mock::given(wm_method("GET"))
            .respond_with(ResponseTemplate::new(500))
            .expect(1..)
            .mount(&server)
            .await;

        // base_url is now on the builder; chain before .build().
        let sdk = Crossref::builder()
            .base_url(server.uri())
            .build()
            .expect("Crossref::builder().base_url(mock).build()");
        let mut adapter = Crossref2Wikidata::new_with_client(sdk);

        // Build an item with a DOI; the adapter looks at P356 (DOI) on the
        // item, then asks the SDK for the work.
        let mut item = wikibase::Entity::new_empty_item();
        item.add_claim(wikibase::Statement::new_normal(
            wikibase::Snak::new_external_id("P356", "10.1234/test"),
            vec![],
            vec![],
        ));

        let pub_id = adapter.publication_id_from_item(&item).await;
        assert!(pub_id.is_none(), "expected None on SDK 500, got {pub_id:?}");
        // .expect(1..) ensures the SDK actually hit the mock URL.
    }
}
