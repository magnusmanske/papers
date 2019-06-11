use crate::crossref2wikidata::Crossref2Wikidata;
use crate::orcid2wikidata::Orcid2Wikidata;
use crate::pubmed2wikidata::Pubmed2Wikidata;
use crate::semanticscholar2wikidata::Semanticscholar2Wikidata;
use crate::sourcemd_config::SourceMD;
use crate::wikidata_papers::WikidataPapers;

use regex::Regex;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone)]
pub struct SourceMDbot {
    mw_api: Option<mediawiki::api::Api>,
    config: Arc<Mutex<SourceMD>>,
    batch_id: i64,
}

impl SourceMDbot {
    pub fn new(config: Arc<Mutex<SourceMD>>, batch_id: i64) -> Self {
        Self {
            config,
            batch_id,
            mw_api: None,
        }
    }

    pub fn start(&self) -> Result<(), String> {
        Ok(())
    }

    pub fn run(self: &mut Self) -> Result<bool, String> {
        lazy_static! {
            static ref RE_WD: Regex = Regex::new(r#"^(Q\d+)$"#).unwrap();
            static ref RE_DOI: Regex = Regex::new(r#"^(.+/.+)$"#).unwrap();
            static ref RE_PMID: Regex = Regex::new(r#"(\d+)$"#).unwrap();
            static ref RE_PMCID: Regex = Regex::new(r#"PMCID(\d+)$"#).unwrap();
        }

        let _wdp = self.new_wdp();

        Ok(false)
    }

    fn new_wdp(&self) -> WikidataPapers {
        let mut wdp = WikidataPapers::new();
        wdp.add_adapter(Box::new(Pubmed2Wikidata::new()));
        wdp.add_adapter(Box::new(Crossref2Wikidata::new()));
        wdp.add_adapter(Box::new(Semanticscholar2Wikidata::new()));
        wdp.add_adapter(Box::new(Orcid2Wikidata::new()));
        wdp
    }
}
