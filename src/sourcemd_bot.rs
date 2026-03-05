use crate::arxiv2wikidata::Arxiv2Wikidata;
use crate::crossref2wikidata::Crossref2Wikidata;
use crate::datacite2wikidata::DataCite2Wikidata;
use crate::europepmc2wikidata::EuropePMC2Wikidata;
use crate::generic_author_info::GenericAuthorInfo;
use crate::identifiers::{GenericWorkIdentifier, IdProp};
use crate::openalex2wikidata::OpenAlex2Wikidata;
use crate::orcid2wikidata::Orcid2Wikidata;
use crate::pmc2wikidata::PMC2Wikidata;
use crate::pubmed2wikidata::Pubmed2Wikidata;
use crate::semanticscholar2wikidata::Semanticscholar2Wikidata;
use crate::sourcemd_command::SourceMDcommand;
use crate::sourcemd_config::SourceMD;
use crate::wikidata_papers::WikidataPapers;
use crate::wikidata_string_cache::WikidataStringCache;
use crate::*;
use anyhow::{anyhow, Result};
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
    ) -> Result<Self> {
        let ret = Self {
            config,
            batch_id,
            cache,
        };
        ret.start().await?;
        Ok(ret)
    }

    pub async fn start(&self) -> Result<()> {
        let config = self.config.read().await;
        config
            .restart_batch(self.batch_id)
            .ok_or(anyhow!("Can't (re)start batch"))?;
        config.set_batch_running(self.batch_id).await;
        Ok(())
    }

    pub async fn run(&self) -> Result<bool> {
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
                    .ok_or(anyhow!("Can't set batch as stopped"))?;
                return Ok(false);
            }
        };

        self.set_command_status("RUNNING", None, &mut command)
            .await?;
        match self.execute_command(&mut command).await {
            Ok(b) => {
                if b {
                    self.set_command_status("DONE", None, &mut command).await?;
                } else {
                    self.set_command_status("DUNNO", None, &mut command).await?;
                }
                Ok(b)
            }
            Err(e) => {
                self.set_command_status("FAILED", Some(&e.to_string()), &mut command)
                    .await?;
                Err(e)
            }
        }
    }

    async fn execute_command(&self, command: &mut SourceMDcommand) -> Result<bool> {
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
            other => Err(anyhow!(
                "Unrecognized command '{}' on command #{}",
                &other,
                &command.id
            )),
        }
    }

    async fn get_author_item(&self, identifier: &str) -> Result<GenericAuthorInfo> {
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
                .get_or_create_author_item(
                    self.config.read().await.mw_api(),
                    self.cache.clone(),
                    false,
                )
                .await
        } else {
            return Err(anyhow!(
                "Not a Wikidata item, nor an ORCID ID {}",
                identifier
            ));
        }

        // Paranoia
        if author.wikidata_item.is_none() {
            return Err(anyhow!(
                "Failed to get/create author item for {}",
                identifier
            ));
        }

        Ok(author)
    }

    pub async fn process_author_metadata(&self, command: &mut SourceMDcommand) -> Result<bool> {
        let author = self.get_author_item(&command.identifier).await?;

        // Create paper object
        let mut wdp = self.new_wdp(command);
        wdp.set_edit_summary(Some(make_edit_summary(&format!(
            "SourceMD [rust bot], [https://tools.wmflabs.org/sourcemd/?action=batch&batch={} batch #{}], command #{}",
            self.batch_id, self.batch_id, command.serial_number
        ))));
        let config = self.config.read().await;
        wdp.update_author_items(&vec![author], config.mw_api())
            .await;
        Ok(true)
    }

    async fn process_paper(&self, command: &mut SourceMDcommand) -> Result<bool> {
        lazy_static! {
            static ref RE_WD: Regex = Regex::new(r#"^(Q\d+)$"#)
                .expect("SourceMDbot::process_paper: RE_WD does not compile");
        }

        let mut wdp = self.new_wdp(command);
        wdp.set_edit_summary(Some(make_edit_summary(&format!(
            "SourceMD [rust bot], [https://tools.wmflabs.org/sourcemd/?action=batch&batch={} batch #{}], command #{}",
            self.batch_id, self.batch_id, command.serial_number
        ))));

        // Wikidata ID
        if RE_WD.is_match(&command.identifier) {
            return wdp
                .create_or_update_item_from_q(
                    self.config.read().await.mw_api(),
                    &command.identifier,
                )
                .await
                .map(|_x| true)
                .ok_or(anyhow!("Can't update {}", &command.identifier));
        }

        // Others: regex-recognised formats
        let mut ids = GenericWorkIdentifier::parse_ids_from_str(&command.identifier);

        // JSON format: {"doi":..., "pmid":..., "pmc":..., "pmcid":...}
        if let Ok(j) = serde_json::from_str::<serde_json::Value>(&command.identifier) {
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
    ) -> Result<()> {
        self.config
            .read()
            .await
            .set_command_status(command, status, message.map(|s| s.to_string()))
            .ok_or(anyhow!(
                "Can't config.set_command_status for batch #{}",
                self.batch_id
            ))?;
        Ok(())
    }

    async fn get_next_command(&self) -> Option<SourceMDcommand> {
        self.config.read().await.get_next_command(self.batch_id)
    }

    fn new_wdp(&self, _command: &SourceMDcommand) -> WikidataPapers {
        let mut wdp = WikidataPapers::new(self.cache.clone());
        wdp.add_adapter(Box::new(PMC2Wikidata::new()));
        wdp.add_adapter(Box::new(Pubmed2Wikidata::new()));
        wdp.add_adapter(Box::new(Crossref2Wikidata::new()));
        wdp.add_adapter(Box::new(Semanticscholar2Wikidata::new()));
        wdp.add_adapter(Box::new(Orcid2Wikidata::new()));
        wdp.add_adapter(Box::new(Arxiv2Wikidata::new()));
        wdp.add_adapter(Box::new(OpenAlex2Wikidata::new()));
        wdp.add_adapter(Box::new(DataCite2Wikidata::new()));
        wdp.add_adapter(Box::new(EuropePMC2Wikidata::new()));
        wdp
    }
}

#[cfg(test)]
mod tests {}
