use std::collections::HashMap;

use async_trait::async_trait;
use tokio::sync::OnceCell;

use self::identifiers::{GenericWorkIdentifier, IdProp};
use crate::{
    adapter_helpers::{
        get_external_identifier_from_item, strip_html_tags, titles_are_equal, wb_time_from_partial,
    },
    generic_author_info::GenericAuthorInfo,
    *,
};

/// Wikidata work-type vocabulary shared between adapters that report
/// publication types via different upstream namespaces.
///
/// Crossref and OpenAlex both use Crossref's hyphen-separated strings
/// (e.g. `"journal-article"`); DataCite uses its own CamelCase set
/// (e.g. `"JournalArticle"`). The two vocabularies overlap on most
/// Q-IDs but the strings differ, so we previously duplicated the
/// Q-IDs across two parser functions and risked them drifting.
/// `WorkType` is the single source of truth for the Q-ID side; each
/// adapter family has its own `from_<vocab>` parser into this enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkType {
    JournalArticle,
    Book,
    Monograph,
    BookChapter,
    ConferencePaper,
    Proceedings,
    Dissertation,
    Preprint,
    Dataset,
    Report,
    Standard,
    Software,
    PeerReview,
}

impl WorkType {
    /// Returns the Wikidata Q-ID for this work type.
    pub fn as_q(self) -> &'static str {
        use WorkType::*;
        match self {
            JournalArticle => "Q13442814",
            Book => "Q571",
            Monograph => "Q193495",
            BookChapter => "Q1980247",
            ConferencePaper => "Q23927052",
            Proceedings => "Q1143604",
            Dissertation => "Q187685",
            Preprint => "Q580922",
            Dataset => "Q1172284",
            Report => "Q10870555",
            Standard => "Q317623",
            Software => "Q7397",
            PeerReview => "Q7161778",
        }
    }

    /// Parses a Crossref `type` field. OpenAlex's `type_crossref` field
    /// mirrors the same vocabulary. Returns `None` for unknown types.
    pub fn from_crossref(s: &str) -> Option<Self> {
        use WorkType::*;
        Some(match s {
            "journal-article" => JournalArticle,
            "book" | "edited-book" | "reference-book" => Book,
            "monograph" => Monograph,
            "book-chapter" | "book-section" => BookChapter,
            "proceedings-article" => ConferencePaper,
            "proceedings" => Proceedings,
            "dissertation" => Dissertation,
            "posted-content" => Preprint,
            "dataset" => Dataset,
            "report" | "report-series" => Report,
            "standard" => Standard,
            "peer-review" => PeerReview,
            _ => return None,
        })
    }

    /// Parses a DataCite `resourceTypeGeneral` value. Returns `None`
    /// for unknown types.
    pub fn from_datacite(s: &str) -> Option<Self> {
        use WorkType::*;
        Some(match s {
            "Book" => Book,
            "BookChapter" => BookChapter,
            "ConferencePaper" => ConferencePaper,
            "ConferenceProceeding" => Proceedings,
            "Dataset" => Dataset,
            "Dissertation" => Dissertation,
            "JournalArticle" | "Journal Article" => JournalArticle,
            "Preprint" => Preprint,
            "Report" => Report,
            "Standard" => Standard,
            "Software" => Software,
            _ => return None,
        })
    }
}

/// Maps a Crossref/OpenAlex work-type string to a Wikidata Q-item for P31.
/// Thin wrapper around [`WorkType::from_crossref`] + [`WorkType::as_q`];
/// kept for callers that want a `&'static str` directly.
pub fn crossref_work_type_to_q(type_: &str) -> Option<&'static str> {
    WorkType::from_crossref(type_).map(WorkType::as_q)
}

/// Parses a date string like "2023-01-15" or "2023-01-15T00:00:00Z" into (year,
/// month, day).
pub fn parse_date(date_str: &str) -> Option<(u32, Option<u8>, Option<u8>)> {
    let date_part = date_str.split('T').next()?;
    let parts: Vec<&str> = date_part.split('-').collect();
    let year: u32 = parts.first()?.parse().ok()?;
    let month: Option<u8> = parts.get(1).and_then(|s| s.parse().ok());
    let day: Option<u8> = parts.get(2).and_then(|s| s.parse().ok());
    Some((year, month, day))
}

#[async_trait(?Send)]
pub trait ScientificPublicationAdapter {
    // You will need to implement these yourself

    /// Returns the name of the resource; internal/debugging use only
    fn name(&self) -> &str;

    /// Returns a cache object reference for the author_id => wikidata_item
    /// mapping; this is handled automatically
    fn author_cache(&self) -> &HashMap<String, String>;

    /// Returns a mutable cache object reference for the author_id =>
    /// wikidata_item mapping; this is handled automatically
    fn author_cache_mut(&mut self) -> &mut HashMap<String, String>;

    /// Tries to determine the publication ID of the resource, from a Wikidata
    /// item
    async fn publication_id_from_item(&mut self, item: &Entity) -> Option<String> {
        match self.publication_property() {
            Some(self_prop) => match get_external_identifier_from_item(item, &self_prop) {
                Some(publication_id) => self.do_cache_work(&publication_id).await,
                None => None,
            },
            None => None,
        }
    }

    /// Emit a warning routed through `tracing` so it appears in structured
    /// logs in all build profiles (formerly a `cfg(debug_assertions)`
    /// no-op in release, which silently swallowed adapter complaints in
    /// production — see audit P3 polish).
    fn warn(&self, msg: &str) {
        tracing::warn!(adapter = self.name(), "{msg}");
    }

    /// Adds/updates "special" statements of an item from the resource, given
    /// the publication ID. Many common statements, title, aliases etc are
    /// automatically handeled via
    /// `update_statements_for_publication_id_default`
    async fn update_statements_for_publication_id(&self, publication_id: &str, item: &mut Entity);

    // You should implement these yourself, where applicable

    /// Returns a list of the authors, if available, with list number, name,
    /// catalog-specific author ID, and WIkidata ID, as available
    async fn get_author_list(&mut self, _publication_id: &str) -> Vec<GenericAuthorInfo> {
        vec![]
    }

    /// Returns a list of IDs for that paper (PMID, DOI etc.)
    async fn get_identifier_list(
        &mut self,
        _ids: &[GenericWorkIdentifier],
    ) -> Vec<GenericWorkIdentifier> {
        vec![]
    }

    /// Returns true if this adapter currently has a cached publication
    /// for `publication_id`. Default is `false` (no cache exists);
    /// adapters with an internal `work_cache` override with a one-line
    /// `self.get_cached_publication_from_id(id).is_some()`.
    ///
    /// This is the defensive guard that
    /// [`add_identifiers_from_cached_publication`] consults before
    /// pushing any IDs. Without it, a buggy caller asking for IDs
    /// from a publication that doesn't exist would silently fabricate
    /// a self-id claim for it.
    fn has_cached_publication(&self, _publication_id: &str) -> bool {
        false
    }

    /// Adapter-specific "self" identifier for a cached publication.
    ///
    /// Default: builds `(publication_property, publication_id_for_statement(publication_id))`
    /// if `publication_property` is set. Override when the cached work's
    /// canonical id differs from the lookup id — e.g. PMC stores the
    /// `pmcid` in the work even when the lookup was by PMID.
    fn extract_self_id(&self, publication_id: &str) -> Option<GenericWorkIdentifier> {
        let p = self.publication_property()?;
        let id = self.publication_id_for_statement(publication_id)?;
        Some(GenericWorkIdentifier::new_prop(p, &id))
    }

    /// Extra identifiers (DOI, PMID, PMCID, …) discoverable in this
    /// adapter's cached work, other than the self-id from
    /// [`extract_self_id`]. Empty by default. Override per-adapter.
    fn extract_extra_ids(&self, _publication_id: &str) -> Vec<GenericWorkIdentifier> {
        vec![]
    }

    /// Combines [`extract_self_id`] and [`extract_extra_ids`] into the
    /// shared shape used by every adapter that contributes IDs in
    /// `get_identifier_list`. Bails out if `has_cached_publication`
    /// returns false. Adapters that previously had their own inherent
    /// `add_identifiers_from_cached_publication` now override
    /// `has_cached_publication` (one-liner) + one of the two extract
    /// hooks above instead.
    fn add_identifiers_from_cached_publication(
        &self,
        publication_id: &str,
        ret: &mut Vec<GenericWorkIdentifier>,
    ) {
        if !self.has_cached_publication(publication_id) {
            return;
        }
        if let Some(self_id) = self.extract_self_id(publication_id) {
            ret.push(self_id);
        }
        ret.extend(self.extract_extra_ids(publication_id));
    }

    /// Returns a lanuage item identifier, or None
    async fn get_language_item(&self, _publication_id: &str) -> Option<String> {
        None
    }

    /// Returns a volume string, or None
    fn get_volume(&self, _publication_id: &str) -> Option<String> {
        None
    }

    /// Returns an issue string, or None
    fn get_issue(&self, _publication_id: &str) -> Option<String> {
        None
    }

    /// Returns the publication date, or None
    fn get_publication_date(&self, _publication_id: &str) -> Option<(u32, Option<u8>, Option<u8>)> {
        None
    }

    /// Returns the property for an author ID of the resource as a `String`,
    /// e.g. P4012 for Semantic Scholar
    fn author_property(&self) -> Option<String> {
        None
    }

    /// Returns the property for a publication ID of the resource as a `String`,
    /// e.g. P4011 for Semantic Scholar
    fn publication_property(&self) -> Option<IdProp> {
        None
    }

    /// Returns the property for a topic ID of the resource as a `String`, e.g.
    /// P6611 for Semantic Scholar
    fn topic_property(&self) -> Option<String> {
        None
    }

    /// Returns the Wikidata Q-item for the type of work (P31), if known.
    /// E.g. "Q571" for a book, "Q13442814" for a scientific article.
    fn get_work_type(&self, _publication_id: &str) -> Option<String> {
        None
    }

    // For a publication ID, return the ISSN as a `String`, if known
    fn get_work_issn(&self, _publication_id: &str) -> Option<String> {
        None
    }

    // For a publication ID, return all known titles as a `Vec<LocaleString>`, main
    // title first (per language)
    fn get_work_titles(&self, _publication_id: &str) -> Vec<LocaleString> {
        vec![]
    }

    // Pre-filled methods; no need to implement them unless there is a need

    async fn do_cache_work(&mut self, _publication_id: &str) -> Option<String> {
        None
    }

    fn reference(&self) -> Vec<Reference> {
        // TODO
        vec![]
    }

    /// Returns the sanitized (if required) publication ID to put in a statement
    fn publication_id_for_statement(&self, id: &str) -> Option<String> {
        Some(id.to_string())
    }

    async fn update_statements_for_publication_id_default(
        &self,
        publication_id: &str,
        item: &mut Entity,
        // cache: Arc<WikidataStringCache>,
    ) {
        self.update_work_item_with_title(publication_id, item);
        self.update_work_item_with_property(publication_id, item);
        self.update_work_item_with_journal(publication_id, item).await;
        self.update_work_item_with_volume(publication_id, item);
        self.update_work_item_with_issue(publication_id, item);
        self.update_work_item_with_publication_date(publication_id, item);
        self.update_work_item_with_language(publication_id, item).await;
    }

    async fn update_work_item_with_language(&self, publication_id: &str, item: &mut Entity) {
        if item.has_claims_with_property("P407") {
            return;
        }
        if let Some(language_q) = self.get_language_item(publication_id).await {
            item.add_claim(Statement::new_normal(
                Snak::new_item("P407", &language_q),
                vec![],
                self.reference(),
            ))
        }
    }

    fn update_work_item_with_volume(&self, publication_id: &str, item: &mut Entity) {
        if item.has_claims_with_property("P478") {
            return;
        }
        if let Some(volume) = self.get_volume(publication_id) {
            item.add_claim(Statement::new_normal(
                Snak::new_string("P478", &volume),
                vec![],
                self.reference(),
            ))
        }
    }

    fn update_work_item_with_issue(&self, publication_id: &str, item: &mut Entity) {
        if item.has_claims_with_property("P433") {
            return;
        }
        if let Some(issue) = self.get_issue(publication_id) {
            item.add_claim(Statement::new_normal(
                Snak::new_string("P433", &issue),
                vec![],
                self.reference(),
            ))
        }
    }

    fn update_work_item_with_publication_date(&self, publication_id: &str, item: &mut Entity) {
        if item.has_claims_with_property("P577") {
            return;
        }
        if let Some(pubdate) = self.get_publication_date(publication_id) {
            let statement =
                wb_time_from_partial("P577", pubdate.0, pubdate.1, pubdate.2, self.reference());
            item.add_claim(statement);
        }
    }

    fn update_work_item_with_title(&self, publication_id: &str, item: &mut Entity) {
        let titles = self.get_work_titles(publication_id);
        if titles.is_empty() {
            return;
        }

        // Re-org, stripping HTML tags from title values (APIs like Crossref
        // and PubMed may return titles with <i>, <b>, <sub>, <sup> etc.)
        let mut by_lang: HashMap<String, Vec<String>> = HashMap::new();
        titles.iter().for_each(|t| {
            let lv = by_lang.entry(t.language().to_string()).or_default();
            lv.push(strip_html_tags(t.value()))
        });
        for (language, titles) in by_lang.iter() {
            let mut titles = titles.clone();
            // Add title
            match item.label_in_locale(language) {
                Some(t) => titles.retain(|x| !titles_are_equal(x, t)), /* Title exists,
                                                                          * remove from title
                                                                          * list */
                None => item.set_label(LocaleString::new("en", &titles.swap_remove(0))), /* No title, add and remove from title list */
            }

            // Add other potential titles as aliases
            // let main_title = item.label_in_locale("en").unwrap_or("").to_string();
            // titles
            //     .iter()
            //     .filter(|t| !self.titles_are_equal(t, &main_title))
            //     .for_each(|t| {
            //         item.add_alias(LocaleString::new(language.to_string(),
            // t.to_string()))     });

            // Add P1476 (title)
            if !item.has_claims_with_property("P1476") {
                let label = item.label_in_locale(language).map(|s| s.to_owned());
                if let Some(title) = label {
                    item.add_claim(Statement::new_normal(
                        Snak::new_monolingual_text("P1476", &title, language),
                        vec![],
                        self.reference(),
                    ))
                }
            }
        }
    }

    async fn update_work_item_with_journal(
        &self,
        publication_id: &str,
        item: &mut Entity,
        // cdn_cache_control_cache: Arc<WikidataStringCache>,
    ) {
        if item.has_claims_with_property("P1433") {
            return;
        }
        if let Some(_issn) = self.get_work_issn(publication_id) {
            let r = Some("".to_string()); // cache.issn2q(&issn).await;
            if let Some(q) = r {
                item.add_claim(Statement::new_normal(
                    Snak::new_item("P1433", &q),
                    vec![],
                    self.reference(),
                ))
            }
        }
    }

    fn update_work_item_with_property(&self, publication_id: &str, item: &mut Entity) {
        if let Some(prop) = self.publication_property() {
            if !item.has_claims_with_property(prop.as_str()) {
                if let Some(pub_id) = self.publication_id_for_statement(publication_id) {
                    item.add_claim(Statement::new_normal(
                        Snak::new_external_id(prop.to_string(), pub_id),
                        vec![],
                        self.reference(),
                    ));
                }
            }
        }
    }

    fn set_author_cache_entry(&mut self, catalog_author_id: &str, q: &str) {
        self.author_cache_mut().insert(catalog_author_id.to_string(), q.to_string());
    }

    fn get_author_item_from_cache(&self, catalog_author_id: &str) -> Option<&String> {
        self.author_cache().get(catalog_author_id)
    }

    fn author_cache_is_empty(&self) -> bool {
        self.author_cache().is_empty()
    }

    /// Resolves an ISO language code to a Wikidata Q-item via the
    /// process-wide [`LanguageCache`]. Cache is lazily populated by a
    /// single SPARQL query the first time any adapter calls
    /// `language2q`; a network failure degrades to an empty cache
    /// (returns `None`) rather than panicking.
    async fn language2q(&self, language: &str) -> Option<String> {
        static CACHE: OnceCell<crate::language_cache::LanguageCache> = OnceCell::const_new();
        CACHE
            .get_or_init(|| async { crate::language_cache::LanguageCache::wikidata() })
            .await
            .get(language)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // === WorkType ===========================================================

    #[test]
    fn work_type_as_q_is_distinct_for_every_variant() {
        // Catches a future copy-paste where two variants accidentally share
        // the same Q-ID. Update the list when adding a new WorkType variant.
        let all = [
            WorkType::JournalArticle,
            WorkType::Book,
            WorkType::Monograph,
            WorkType::BookChapter,
            WorkType::ConferencePaper,
            WorkType::Proceedings,
            WorkType::Dissertation,
            WorkType::Preprint,
            WorkType::Dataset,
            WorkType::Report,
            WorkType::Standard,
            WorkType::Software,
            WorkType::PeerReview,
        ];
        let qs: Vec<&'static str> = all.iter().map(|w| w.as_q()).collect();
        let mut unique = qs.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(qs.len(), unique.len(), "duplicate Q-ID across WorkType variants: {qs:?}");
        // And every Q-ID must look like a Q-ID.
        for q in &qs {
            assert!(q.starts_with('Q') && q[1..].chars().all(|c| c.is_ascii_digit()), "bad Q: {q}");
        }
    }

    #[test]
    fn work_type_from_crossref_known_inputs() {
        assert_eq!(WorkType::from_crossref("journal-article"), Some(WorkType::JournalArticle));
        assert_eq!(WorkType::from_crossref("book"), Some(WorkType::Book));
        assert_eq!(WorkType::from_crossref("edited-book"), Some(WorkType::Book));
        assert_eq!(WorkType::from_crossref("reference-book"), Some(WorkType::Book));
        assert_eq!(WorkType::from_crossref("posted-content"), Some(WorkType::Preprint));
        assert_eq!(WorkType::from_crossref("report-series"), Some(WorkType::Report));
        assert_eq!(WorkType::from_crossref("peer-review"), Some(WorkType::PeerReview));
    }

    #[test]
    fn work_type_from_crossref_unknown_returns_none() {
        assert_eq!(WorkType::from_crossref(""), None);
        assert_eq!(WorkType::from_crossref("unknown-type"), None);
        // Case sensitivity: Crossref is lowercase-with-hyphens.
        assert_eq!(WorkType::from_crossref("Journal-Article"), None);
    }

    #[test]
    fn work_type_from_datacite_known_inputs() {
        assert_eq!(WorkType::from_datacite("JournalArticle"), Some(WorkType::JournalArticle));
        // DataCite also produces the spaced variant.
        assert_eq!(WorkType::from_datacite("Journal Article"), Some(WorkType::JournalArticle));
        assert_eq!(WorkType::from_datacite("ConferenceProceeding"), Some(WorkType::Proceedings));
        assert_eq!(WorkType::from_datacite("Software"), Some(WorkType::Software));
        assert_eq!(WorkType::from_datacite("Preprint"), Some(WorkType::Preprint));
    }

    #[test]
    fn work_type_from_datacite_unknown_returns_none() {
        assert_eq!(WorkType::from_datacite(""), None);
        assert_eq!(WorkType::from_datacite("nope"), None);
        // Case sensitivity: DataCite is CamelCase.
        assert_eq!(WorkType::from_datacite("journalarticle"), None);
    }

    #[test]
    fn work_type_crossref_and_datacite_agree_on_overlap() {
        // For every input where both vocabularies have a mapping, the
        // resulting WorkType (and therefore Q-ID) must match. Regression
        // guard for vocabulary drift between adapters.
        let pairs = [
            ("journal-article", "JournalArticle"),
            ("book", "Book"),
            ("book-chapter", "BookChapter"),
            ("proceedings-article", "ConferencePaper"),
            ("proceedings", "ConferenceProceeding"),
            ("dissertation", "Dissertation"),
            ("posted-content", "Preprint"),
            ("dataset", "Dataset"),
            ("report", "Report"),
            ("standard", "Standard"),
        ];
        for (crossref, datacite) in pairs {
            assert_eq!(
                WorkType::from_crossref(crossref),
                WorkType::from_datacite(datacite),
                "{crossref} (crossref) vs {datacite} (datacite) disagree"
            );
        }
    }

    // === ScientificPublicationAdapter helpers =================================

    /// Minimal test adapter that returns configurable titles
    struct TestAdapter {
        titles: Vec<LocaleString>,
        author_cache: HashMap<String, String>,
    }

    impl TestAdapter {
        fn with_titles(titles: Vec<&str>) -> Self {
            Self {
                titles: titles.into_iter().map(|t| LocaleString::new("en", t)).collect(),
                author_cache: HashMap::new(),
            }
        }
    }

    #[async_trait(?Send)]
    impl ScientificPublicationAdapter for TestAdapter {
        fn name(&self) -> &str {
            "test"
        }
        fn author_cache(&self) -> &HashMap<String, String> {
            &self.author_cache
        }
        fn author_cache_mut(&mut self) -> &mut HashMap<String, String> {
            &mut self.author_cache
        }
        async fn update_statements_for_publication_id(
            &self,
            _publication_id: &str,
            _item: &mut Entity,
        ) {
        }
        fn get_work_titles(&self, _publication_id: &str) -> Vec<LocaleString> {
            self.titles.clone()
        }
    }

    // Pure-helper tests (strip_html_tags, titles_are_equal,
    // sanitize_author_name, wb_time_from_partial,
    // get_external_identifier_from_item) live in `adapter_helpers::tests`
    // since the helpers themselves are now free functions there. The
    // tests below exercise the trait *templates* that still use them.

    // === update_work_item_with_title with HTML tags (issue #7) ===

    #[test]
    fn update_work_item_with_title_strips_html_from_label() {
        let adapter = TestAdapter::with_titles(vec![
            "Correction: <i>Accidental aspiration of a solid tablet of sodium hydroxide</i>",
        ]);
        let mut item = Entity::new_empty_item();
        adapter.update_work_item_with_title("test_id", &mut item);

        assert_eq!(
            item.label_in_locale("en"),
            Some("Correction: Accidental aspiration of a solid tablet of sodium hydroxide"),
        );
    }

    #[test]
    fn update_work_item_with_title_strips_html_from_species_names() {
        let adapter = TestAdapter::with_titles(vec![
            "Population genetics of <i>Drosophila melanogaster</i> in tropical environments",
        ]);
        let mut item = Entity::new_empty_item();
        adapter.update_work_item_with_title("test_id", &mut item);

        assert_eq!(
            item.label_in_locale("en"),
            Some("Population genetics of Drosophila melanogaster in tropical environments"),
        );
    }

    #[test]
    fn update_work_item_with_title_plain_text_unaffected() {
        let adapter = TestAdapter::with_titles(vec!["A plain text title"]);
        let mut item = Entity::new_empty_item();
        adapter.update_work_item_with_title("test_id", &mut item);

        assert_eq!(item.label_in_locale("en"), Some("A plain text title"));
    }
}
