//! Pure-function helpers used across the provider adapters.
//!
//! Extracted from the `ScientificPublicationAdapter` trait so that
//! - they're testable without an adapter instance,
//! - they can't be accidentally overridden per-adapter (they're not
//!   polymorphism points — they're plumbing), and
//! - the trait surface shrinks toward "true" per-adapter behaviour.
//!
//! See `audits/STATUS.md` P2-6.

use regex::Regex;
use wikibase::{Entity, EntityTrait, Reference, Snak, SnakType, Statement, Value};

use crate::identifiers::IdProp;

lazy_static::lazy_static! {
    static ref RE_HTML: Regex =
        Regex::new(r"<[^>]+>").expect("RE_HTML");
}

/// Strips HTML/XML tags from a string and collapses internal whitespace
/// to a single space. Upstream APIs (Crossref, PubMed) return titles
/// peppered with `<i>`, `<b>`, `<sub>` etc.; we strip those before
/// using the title as a label.
///
/// `"Correction: <i>Accidental aspiration</i>"` → `"Correction: Accidental aspiration"`.
pub fn strip_html_tags(s: &str) -> String {
    let result = RE_HTML.replace_all(s, "");
    result.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Loose equality for title strings: case-insensitive, ignoring trailing
/// `.`, and trimming whitespace.
pub fn titles_are_equal(t1: &str, t2: &str) -> bool {
    if t1 == t2 {
        return true;
    }
    let t1 = t1.to_lowercase();
    let t1 = t1.trim_end_matches('.').trim();
    let t2 = t2.to_lowercase();
    let t2 = t2.trim_end_matches('.').trim();
    t1 == t2
}

/// Removes the affiliation-marker glyphs (`†`, `‡`) that some
/// publishers append to author names, then trims whitespace.
pub fn sanitize_author_name(author_name: &str) -> String {
    author_name.replace(['†', '‡'], "").trim().to_string()
}

/// Returns a Wikidata `time` statement for the property `property` at the
/// precision dictated by which of `year`/`month`/`day` are provided:
/// 9 (year), 10 (month), 11 (day).
///
/// `references` is the per-adapter reference list to attach.
pub fn wb_time_from_partial(
    property: &str,
    year: u32,
    month: Option<u8>,
    day: Option<u8>,
    references: Vec<Reference>,
) -> Statement {
    let (month_str, precision) = match month {
        Some(m) => (format!("-{m:02}"), 10u64),
        None => ("-01".to_string(), 9),
    };
    let (day_str, precision) = match day {
        Some(d) => (format!("-{d:02}"), 11u64),
        None => ("-01".to_string(), precision),
    };
    let time = format!("+{year}{month_str}{day_str}T00:00:00Z");
    // Snak::new_time wants matching types for property and value via Into<String>.
    let snak = Snak::new_time(property.to_string(), time, precision);
    Statement::new_normal(snak, vec![], references)
}

/// Finds the first `string`-valued claim on `item` whose property matches
/// `property` (and whose snak type is `Value`, i.e. not "no value" or
/// "unknown value"), and returns its string.
pub fn get_external_identifier_from_item(item: &Entity, property: &IdProp) -> Option<String> {
    item.claims()
        .iter()
        .filter(|claim| {
            claim.main_snak().property() == property.as_str()
                && *claim.main_snak().snak_type() == SnakType::Value
        })
        .find_map(|claim| match claim.main_snak().data_value().as_ref()?.value() {
            Value::StringValue(s) => Some(s.to_string()),
            _ => None,
        })
}

/// Parses a date string like `"2023-01-15"` or `"2023-01-15T00:00:00Z"`
/// into `(year, month, day)`. Re-exported here so adapter modules can
/// import all pure helpers from a single place.
pub fn parse_date(date_str: &str) -> Option<(u32, Option<u8>, Option<u8>)> {
    crate::scientific_publication_adapter::parse_date(date_str)
}

/// Fetches a DOI's JSON record via the given [`JsonFetcher`] and returns
/// `(uppercased_doi, json)` on 2xx + parse success. Returns `None` for
/// transport failure, non-2xx response, or JSON-parse failure.
///
/// The `url_for` closure builds the per-adapter URL pattern. Adapter
/// callers do any post-fetch validation (e.g. checking for an expected
/// JSON path) on the returned value before caching it.
///
/// Used by the three DOI-based adapters
/// (`datacite2wikidata`, `europepmc2wikidata`, `openalex2wikidata`)
/// to avoid duplicating the URL+fetch+uppercase pattern. See audit P2-3.
pub async fn fetch_doi_json<F>(
    fetcher: &dyn crate::http_client::JsonFetcher,
    doi: &str,
    url_for: F,
) -> Option<(String, serde_json::Value)>
where
    F: FnOnce(&str) -> String,
{
    let url = url_for(doi);
    let json = fetcher.fetch_json(&url).await?;
    Some((doi.to_uppercase(), json))
}

#[cfg(test)]
mod tests {
    use super::*;

    // === strip_html_tags ===================================================

    #[test]
    fn strip_html_tags_removes_italic_tags() {
        assert_eq!(
            strip_html_tags(
                "Correction: <i>Accidental aspiration of a solid tablet of sodium hydroxide</i>"
            ),
            "Correction: Accidental aspiration of a solid tablet of sodium hydroxide"
        );
    }

    #[test]
    fn strip_html_tags_removes_various_tags() {
        assert_eq!(strip_html_tags("The <i>Drosophila</i> <b>gene</b>"), "The Drosophila gene");
    }

    #[test]
    fn strip_html_tags_handles_sub_sup() {
        assert_eq!(strip_html_tags("H<sub>2</sub>O and CO<sub>2</sub>"), "H2O and CO2");
        assert_eq!(strip_html_tags("x<sup>2</sup> + y<sup>2</sup>"), "x2 + y2");
    }

    #[test]
    fn strip_html_tags_no_tags_unchanged() {
        assert_eq!(strip_html_tags("A simple title"), "A simple title");
    }

    #[test]
    fn strip_html_tags_collapses_whitespace() {
        assert_eq!(strip_html_tags("Before  <i> middle </i>  after"), "Before middle after");
    }

    // === sanitize_author_name ==============================================

    #[test]
    fn sanitize_author_name_removes_dagger() {
        assert_eq!(sanitize_author_name("Smith†"), "Smith");
        assert_eq!(sanitize_author_name("Jones‡"), "Jones");
    }

    #[test]
    fn sanitize_author_name_trims_whitespace_after_removal() {
        assert_eq!(sanitize_author_name("Alice †"), "Alice");
        assert_eq!(sanitize_author_name("Bob ‡ "), "Bob");
    }

    #[test]
    fn sanitize_author_name_unchanged_when_no_special_chars() {
        assert_eq!(sanitize_author_name("Jane Doe"), "Jane Doe");
        assert_eq!(sanitize_author_name(""), "");
    }

    // === titles_are_equal ===================================================

    #[test]
    fn titles_are_equal_exact_match() {
        assert!(titles_are_equal("Hello World", "Hello World"));
    }

    #[test]
    fn titles_are_equal_case_insensitive() {
        assert!(titles_are_equal("hello world", "HELLO WORLD"));
        assert!(titles_are_equal("Hello World", "hello world"));
    }

    #[test]
    fn titles_are_equal_trailing_period_stripped() {
        assert!(titles_are_equal("A title.", "A title"));
        assert!(titles_are_equal("A title", "A title."));
        assert!(titles_are_equal("A title.", "A title."));
    }

    #[test]
    fn titles_are_equal_different_titles_return_false() {
        assert!(!titles_are_equal("Title One", "Title Two"));
    }

    // === wb_time_from_partial ===============================================

    #[test]
    fn wb_time_year_only_has_precision_9() {
        let stmt = wb_time_from_partial("P577", 2021, None, None, vec![]);
        assert_eq!(stmt.main_snak().property(), "P577");
        if let Some(dv) = stmt.main_snak().data_value() {
            if let Value::Time(tv) = dv.value() {
                assert_eq!(tv.time(), "+2021-01-01T00:00:00Z");
                assert_eq!(*tv.precision(), 9u64);
            } else {
                panic!("Expected Time value");
            }
        } else {
            panic!("Expected data value");
        }
    }

    #[test]
    fn wb_time_year_month_has_precision_10() {
        let stmt = wb_time_from_partial("P577", 2021, Some(6), None, vec![]);
        if let Some(dv) = stmt.main_snak().data_value() {
            if let Value::Time(tv) = dv.value() {
                assert_eq!(tv.time(), "+2021-06-01T00:00:00Z");
                assert_eq!(*tv.precision(), 10u64);
            } else {
                panic!("Expected Time value");
            }
        } else {
            panic!("Expected data value");
        }
    }

    #[test]
    fn wb_time_full_date_has_precision_11() {
        let stmt = wb_time_from_partial("P577", 2021, Some(3), Some(15), vec![]);
        if let Some(dv) = stmt.main_snak().data_value() {
            if let Value::Time(tv) = dv.value() {
                assert_eq!(tv.time(), "+2021-03-15T00:00:00Z");
                assert_eq!(*tv.precision(), 11u64);
            } else {
                panic!("Expected Time value");
            }
        } else {
            panic!("Expected data value");
        }
    }

    // === get_external_identifier_from_item ==================================

    #[test]
    fn get_external_identifier_finds_matching_property() {
        let mut item = Entity::new_empty_item();
        item.add_claim(Statement::new_normal(
            Snak::new_external_id("P356", "10.1234/TEST"),
            vec![],
            vec![],
        ));
        let result = get_external_identifier_from_item(&item, &IdProp::DOI);
        assert_eq!(result, Some("10.1234/TEST".to_string()));
    }

    #[test]
    fn get_external_identifier_returns_none_for_wrong_property() {
        let mut item = Entity::new_empty_item();
        item.add_claim(Statement::new_normal(
            Snak::new_external_id("P356", "10.1234/TEST"),
            vec![],
            vec![],
        ));
        let result = get_external_identifier_from_item(&item, &IdProp::PMID);
        assert_eq!(result, None);
    }

    #[test]
    fn get_external_identifier_returns_none_for_empty_item() {
        let item = Entity::new_empty_item();
        let result = get_external_identifier_from_item(&item, &IdProp::DOI);
        assert_eq!(result, None);
    }

    // === parse_date =========================================================

    #[test]
    fn parse_date_full() {
        assert_eq!(parse_date("2023-06-15"), Some((2023, Some(6), Some(15))));
    }

    #[test]
    fn parse_date_year_only() {
        assert_eq!(parse_date("2023"), Some((2023, None, None)));
    }

    #[test]
    fn parse_date_with_time_suffix() {
        assert_eq!(parse_date("2023-06-15T00:00:00Z"), Some((2023, Some(6), Some(15))));
    }

    #[test]
    fn parse_date_empty_returns_none() {
        assert_eq!(parse_date(""), None);
    }
}
