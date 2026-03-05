#[macro_use]
extern crate serde_json;
#[macro_use]
extern crate lazy_static;

use wikibase::entity_diff::*;
use wikibase::*;

pub mod arxiv2wikidata;
pub mod author_name_string;
pub mod crossref2wikidata;
pub mod datacite2wikidata;
pub mod europepmc2wikidata;
pub mod generic_author_info;
pub mod identifiers;
pub mod openalex2wikidata;
pub mod orcid2wikidata;
pub mod pmc2wikidata;
pub mod pubmed2wikidata;
pub mod scientific_publication_adapter;
pub mod semanticscholar2wikidata;
pub mod sourcemd_bot;
pub mod sourcemd_command;
pub mod sourcemd_config;
pub mod wikidata_interaction;
pub mod wikidata_papers;
pub mod wikidata_string_cache;

pub fn make_edit_summary(base: &str) -> String {
    if base.is_empty() {
        "(automated edit by SourceMD)".to_string()
    } else {
        format!("{base} (automated edit by SourceMD)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn make_edit_summary_empty_base_returns_note_only() {
        assert_eq!(make_edit_summary(""), "(automated edit by SourceMD)");
    }

    #[test]
    fn make_edit_summary_non_empty_base_appends_note() {
        assert_eq!(
            make_edit_summary("SourceMD [rust bot]"),
            "SourceMD [rust bot] (automated edit by SourceMD)"
        );
    }

    #[test]
    fn make_edit_summary_always_ends_with_attribution() {
        let result = make_edit_summary("some note");
        assert!(result.ends_with("(automated edit by SourceMD)"));
    }
}
