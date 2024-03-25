extern crate crossref;
extern crate reqwest;
#[macro_use]
extern crate serde_json;
#[macro_use]
extern crate lazy_static;

use wikibase::entity_diff::*;
use wikibase::*;

pub mod crossref2wikidata;
pub mod generic_author_info;
pub mod identifiers;
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
