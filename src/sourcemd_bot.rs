use crate::crossref2wikidata::Crossref2Wikidata;
use crate::generic_author_info::GenericAuthorInfo;
use crate::identifiers::{GenericWorkIdentifier, IdProp};
use crate::orcid2wikidata::Orcid2Wikidata;
use crate::pmc2wikidata::PMC2Wikidata;
use crate::pubmed2wikidata::Pubmed2Wikidata;
use crate::semanticscholar2wikidata::Semanticscholar2Wikidata;
use crate::sourcemd_command::SourceMDcommand;
use crate::sourcemd_config::SourceMD;
use crate::wikidata_papers::WikidataPapers;
use crate::wikidata_string_cache::WikidataStringCache;
use crate::*;
use regex::Regex;
use std::sync::Arc;
use tokio::sync::RwLock;

use self::sourcemd_command::SourceMDcommandMode;

#[derive(Debug, Clone)]
pub struct SourceMDbot {
    config: Arc<RwLock<SourceMD>>,
    cache: Arc<WikidataStringCache>,
    batch_id: i64,
}

impl SourceMDbot {
    pub async fn new(
        config: Arc<RwLock<SourceMD>>,
        cache: Arc<WikidataStringCache>,
        batch_id: i64,
    ) -> Result<Self, String> {
        let ret = Self {
            config,
            batch_id,
            cache,
        };
        ret.start().await?;
        Ok(ret)
    }

    pub async fn start(&self) -> Result<(), String> {
        let config = self.config.read().await;
        config
            .restart_batch(self.batch_id)
            .ok_or("Can't (re)start batch".to_string())?;
        config.set_batch_running(self.batch_id).await;
        Ok(())
    }

    pub async fn run(&self) -> Result<bool, String> {
        //println!("Running command from batch #{}", self.batch_id);

        //Check if batch is still valid (STOP etc)
        let command = self.get_next_command().await;
        let mut command = match command {
            Some(c) => c,
            None => {
                self.config
                    .read()
                    .await
                    .deactivate_batch_run(self.batch_id)
                    .await
                    .ok_or("Can't set batch as stopped".to_string())?;
                return Ok(false);
            }
        };

        self.set_command_status("RUNNING", None, &mut command)
            .await?;
        match self.execute_command(&mut command).await {
            Ok(b) => {
                if b {
                    self.set_command_status("DONE", None, &mut command).await?;
                    Ok(true)
                } else {
                    self.set_command_status("DUNNO", None, &mut command).await?;
                    Ok(false)
                }
            }
            Err(e) => {
                self.set_command_status("FAILED", Some(&e.clone()), &mut command)
                    .await?;
                Err(e)
            }
        }
    }

    async fn execute_command(&self, command: &mut SourceMDcommand) -> Result<bool, String> {
        match &command.mode {
            SourceMDcommandMode::CreatePaperById => self.process_paper(command).await,
            SourceMDcommandMode::AddAutthorToPublication => self.process_paper(command).await,
            SourceMDcommandMode::AddOrcidMetadataToAuthor => {
                // TODO
                if true {
                    Ok(false)
                } else {
                    self.process_author_metadata(command).await
                }
            }
            SourceMDcommandMode::EditPaperForOrcidAuthor => Ok(false), // TODO
            SourceMDcommandMode::CreateBookFromIsbn => Ok(false),      // TODO
            other => Err(format!(
                "Unrecognized command '{}' on command #{}",
                &other, &command.id
            )),
        }
    }

    async fn get_author_item(&self, identifier: &str) -> Result<GenericAuthorInfo, String> {
        lazy_static! {
            static ref RE_WD: Regex = Regex::new(r#"^(Q\d+)$"#)
                .expect("SourceMDbot::process_author: RE_WD does not compile");
            static ref RE_ORCID: Regex = Regex::new(r#"^(\d{4}-\d{4}-\d{4}-\d{4})$"#)
                .expect("SourceMDbot::process_author: RE_ORCID does not compile");
        }

        let mut author = GenericAuthorInfo::new();
        if RE_WD.is_match(identifier) {
            author.wikidata_item = Some(identifier.to_owned());
        } else if RE_ORCID.is_match(identifier) {
            author
                .prop2id
                .insert("P496".to_string(), identifier.to_owned());
            author = author
                .get_or_create_author_item(self.config.read().await.mw_api(), self.cache.clone())
                .await
        } else {
            return Err(format!(
                "Not a Wikidata item, nor an ORCID ID {}",
                identifier
            ));
        }

        // Paranoia
        if author.wikidata_item.is_none() {
            return Err(format!(
                "Failed to get/create author item for {}",
                identifier
            ));
        }

        Ok(author)
    }

    pub async fn process_author_metadata(
        &self,
        command: &mut SourceMDcommand,
    ) -> Result<bool, String> {
        let author = self.get_author_item(&command.identifier).await?;

        // Create paper object
        let mut wdp = self.new_wdp(command);
        wdp.set_edit_summary(Some(format!(
            "SourceMD [rust bot], [https://tools.wmflabs.org/sourcemd/?action=batch&batch={} batch #{}], command #{}",
            self.batch_id, self.batch_id, command.serial_number
        )));
        let config = self.config.read().await;
        wdp.update_author_items(&vec![author], config.mw_api())
            .await;
        Ok(true)
    }

    async fn process_paper(&self, command: &mut SourceMDcommand) -> Result<bool, String> {
        lazy_static! {
            static ref RE_WD: Regex = Regex::new(r#"^(Q\d+)$"#)
                .expect("SourceMDbot::process_paper: RE_WD does not compile");
            static ref RE_DOI: Regex = Regex::new(r#"^(.+/.+)$"#)
                .expect("SourceMDbot::process_paper: RE_DOI does not compile");
            static ref RE_PMID: Regex = Regex::new(r#"^(\d+)$"#)
                .expect("SourceMDbot::process_paper: RE_PMID does not compile");
            static ref RE_PMCID: Regex = Regex::new(r#"^(PMC\d+)$"#)
                .expect("SourceMDbot::process_paper: RE_PMCID does not compile");
        }

        //println!("Processing command {:?}", &command);
        let mut wdp = self.new_wdp(command);
        wdp.set_edit_summary(Some(format!(
            "SourceMD [rust bot], [https://tools.wmflabs.org/sourcemd/?action=batch&batch={} batch #{}], command #{}",
            self.batch_id, self.batch_id, command.serial_number
        )));

        // Wikidata ID
        if RE_WD.is_match(&command.identifier) {
            return wdp
                .create_or_update_item_from_q(
                    self.config.read().await.mw_api(),
                    &command.identifier,
                )
                .await
                .map(|_x| true)
                .ok_or(format!("Can't update {}", &command.identifier));
        }

        // Others
        let mut ids = vec![];
        if let Some(caps) = RE_DOI.captures(&command.identifier) {
            if let Some(x) = caps.get(1) {
                ids.push(GenericWorkIdentifier::new_prop(IdProp::DOI, x.as_str()))
            }
        };
        if let Some(caps) = RE_PMID.captures(&command.identifier) {
            if let Some(x) = caps.get(1) {
                ids.push(GenericWorkIdentifier::new_prop(IdProp::PMID, x.as_str()))
            }
        };
        if let Some(caps) = RE_PMCID.captures(&command.identifier) {
            if let Some(x) = caps.get(1) {
                ids.push(GenericWorkIdentifier::new_prop(IdProp::PMCID, x.as_str()))
            }
        };
        if let Ok(j) = serde_json::from_str(&command.identifier) {
            let j: serde_json::Value = j;
            if let Some(id) = j["doi"].as_str() {
                let id = id.replace("doi: ", "");
                ids.push(GenericWorkIdentifier::new_prop(IdProp::DOI, &id));
            }
            if let Some(id) = j["pmid"].as_str() {
                ids.push(GenericWorkIdentifier::new_prop(IdProp::PMID, id));
            }
            if let Some(id) = j["pmc"].as_str() {
                let id = id.replace("PMCID: ", "");
                ids.push(GenericWorkIdentifier::new_prop(IdProp::PMCID, &id));
            }
            if let Some(id) = j["pmcid"].as_str() {
                let id = id.replace("PMCID: ", "");
                ids.push(GenericWorkIdentifier::new_prop(IdProp::PMCID, &id));
            }
        }

        if ids.is_empty() {
            return Ok(false);
        }

        ids = wdp.update_from_paper_ids(&ids).await;
        match wdp
            .create_or_update_item_from_ids(self.config.read().await.mw_api(), &ids)
            .await
        {
            Some(er) => {
                if command.q.is_empty() {
                    command.q = er.q;
                }
                Ok(true)
            }
            None => Ok(false),
        }
    }

    async fn set_command_status(
        &self,
        status: &str,
        message: Option<&str>,
        command: &mut SourceMDcommand,
    ) -> Result<(), String> {
        //println!("Setting {} to {}", &command.id, &status);
        self.config
            .read()
            .await
            .set_command_status(command, status, message.map(|s| s.to_string()))
            .ok_or(format!(
                "Can't config.set_command_status for batch #{}",
                self.batch_id
            ))?;
        Ok(())
    }

    async fn get_next_command(&self) -> Option<SourceMDcommand> {
        self.config.read().await.get_next_command(self.batch_id)
    }

    fn new_wdp(&self, command: &SourceMDcommand) -> WikidataPapers {
        let mut wdp = WikidataPapers::new(self.cache.clone());
        wdp.add_adapter(Box::new(PMC2Wikidata::new()));
        wdp.add_adapter(Box::new(Pubmed2Wikidata::new()));
        wdp.add_adapter(Box::new(Crossref2Wikidata::new()));
        wdp.add_adapter(Box::new(Semanticscholar2Wikidata::new()));
        wdp.add_adapter(Box::new(Orcid2Wikidata::new()));
        wdp.set_edit_summary(Some(format!(
            "SourceMD [rust bot], [https://tools.wmflabs.org/sourcemd/?action=batch&batch={} batch #{}], command #{}",
            command.batch_id, command.batch_id, command.id
        )));
        wdp
    }
}

#[cfg(test)]
mod tests {
    //use super::*;
    //use wikibase::mediawiki::api::Api;

    /*
    TODO:
    pub fn new(
    pub fn start(&self) -> Result<(), String> {
    pub fn run(self: &mut Self) -> Result<bool, String> {
    fn execute_command(self: &mut Self, command: &mut SourceMDcommand) -> Result<bool, String> {
    fn process_paper(self: &mut Self, command: &mut SourceMDcommand) -> Result<bool, String> {
    fn set_command_status(
    fn get_next_command(&self) -> Result<Option<SourceMDcommand>, String> {
    fn new_wdp(&self, command: &SourceMDcommand) -> WikidataPapers {
    */
}
