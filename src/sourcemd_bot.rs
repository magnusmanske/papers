use crate::crossref2wikidata::Crossref2Wikidata;
use crate::orcid2wikidata::Orcid2Wikidata;
use crate::pubmed2wikidata::Pubmed2Wikidata;
use crate::semanticscholar2wikidata::Semanticscholar2Wikidata;
use crate::sourcemd_command::SourceMDcommand;
use crate::sourcemd_config::SourceMD;
use crate::wikidata_papers::WikidataPapers;
use crate::wikidata_string_cache::WikidataStringCache;
use crate::*;
use config::{Config, File};
use regex::Regex;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone)]
pub struct SourceMDbot {
    mw_api: mediawiki::api::Api,
    config: Arc<Mutex<SourceMD>>,
    cache: Arc<Mutex<WikidataStringCache>>,
    batch_id: i64,
}

impl SourceMDbot {
    pub fn new(
        config: Arc<Mutex<SourceMD>>,
        cache: Arc<Mutex<WikidataStringCache>>,
        batch_id: i64,
    ) -> Result<Self, String> {
        let ret = Self {
            config: config,
            batch_id: batch_id,
            mw_api: SourceMDbot::get_mw_api("bot.ini"),
            cache: cache,
        };
        ret.start()?;
        Ok(ret)
    }

    pub fn start(&self) -> Result<(), String> {
        let mut config = self.config.lock().map_err(|e| format!("{:?}", e))?;
        config
            .restart_batch(self.batch_id)
            .ok_or("Can't (re)start batch".to_string())?;
        config.set_batch_running(self.batch_id);
        Ok(())
    }

    pub fn run(self: &mut Self) -> Result<bool, String> {
        //println!("Running command from batch #{}", self.batch_id);
        //Check if batch is still valid (STOP etc)
        let command = match self.get_next_command() {
            Ok(c) => c,
            Err(_) => {
                let mut config = self.config.lock().map_err(|e| format!("{:?}", e))?;
                config
                    .deactivate_batch_run(self.batch_id)
                    .ok_or("Can't set batch as stopped".to_string())?;
                return Ok(false);
            }
        };
        let mut command = match command {
            Some(c) => c,
            None => {
                let mut config = self.config.lock().map_err(|e| format!("{:?}", e))?;
                config
                    .deactivate_batch_run(self.batch_id)
                    .ok_or("Can't set batch as stopped".to_string())?;
                return Ok(false);
            }
        };

        self.set_command_status("RUNNING", None, &mut command)?;
        match self.execute_command(&mut command) {
            Ok(b) => {
                if b {
                    self.set_command_status("DONE", None, &mut command)?;
                    Ok(true)
                } else {
                    self.set_command_status("DUNNO", None, &mut command)?;
                    Ok(false)
                }
            }
            Err(e) => {
                self.set_command_status("FAILED", Some(&e.clone()), &mut command)?;
                Err(e)
            }
        }
    }

    fn execute_command(self: &mut Self, command: &mut SourceMDcommand) -> Result<bool, String> {
        match command.mode.as_str() {
            "CREATE_PAPER_BY_ID" => self.process_paper(command),
            "ADD_AUTHOR_TO_PUBLICATION" => self.process_paper(command),
            "ADD_METADATA_FROM_ORCID_TO_AUTHOR" => Ok(false), // TODO
            "EDIT_PAPER_FOR_ORCID_AUTHOR" => Ok(false),       // TODO
            "CREATE_BOOK_FROM_ISBN" => Ok(false),             // TODO
            other => {
                return Err(format!(
                    "Unrecognized command '{}' on command #{}",
                    &other, &command.id
                ))
            }
        }
    }

    fn process_paper(self: &mut Self, command: &mut SourceMDcommand) -> Result<bool, String> {
        lazy_static! {
            static ref RE_WD: Regex = Regex::new(r#"^(Q\d+)$"#).unwrap();
            static ref RE_DOI: Regex = Regex::new(r#"^(.+/.+)$"#).unwrap();
            static ref RE_PMID: Regex = Regex::new(r#"^(\d+)$"#).unwrap();
            static ref RE_PMCID: Regex = Regex::new(r#"^PMCID(\d+)$"#).unwrap();
        }

        //println!("Processing command {:?}", &command);
        let mut wdp = self.new_wdp(&command);
        wdp.set_edit_summary(Some(format!(
            "SourceMD [rust bot], [https://tools.wmflabs.org/sourcemd/?action=batch&batch={} batch #{}]",
            self.batch_id, self.batch_id
        )));

        // Wikidata ID
        if RE_WD.is_match(&command.identifier) {
            return wdp
                .create_or_update_item_from_q(&mut self.mw_api, &command.identifier)
                .map(|_x| true)
                .ok_or(format!("Can't update {}", &command.identifier));
        }

        // Others
        let mut ids = vec![];
        match RE_DOI.captures(&command.identifier) {
            Some(caps) => ids.push(GenericWorkIdentifier::new_prop(
                PROP_DOI,
                caps.get(1).unwrap().as_str(),
            )),
            None => {}
        };
        match RE_PMID.captures(&command.identifier) {
            Some(caps) => ids.push(GenericWorkIdentifier::new_prop(
                PROP_PMID,
                caps.get(1).unwrap().as_str(),
            )),
            None => {}
        };
        match RE_PMCID.captures(&command.identifier) {
            Some(caps) => ids.push(GenericWorkIdentifier::new_prop(
                PROP_PMCID,
                caps.get(1).unwrap().as_str(),
            )),
            None => {}
        };
        match serde_json::from_str(&command.identifier) {
            Ok(j) => {
                let j: serde_json::Value = j;
                match j["doi"].as_str() {
                    Some(id) => {
                        ids.push(GenericWorkIdentifier::new_prop(PROP_DOI, id));
                    }
                    None => {}
                }
                match j["pmid"].as_str() {
                    Some(id) => {
                        ids.push(GenericWorkIdentifier::new_prop(PROP_PMID, id));
                    }
                    None => {}
                }
                match j["pmcid"].as_str() {
                    Some(id) => {
                        ids.push(GenericWorkIdentifier::new_prop(PROP_PMCID, id));
                    }
                    None => {}
                }
            }
            Err(_) => {}
        }

        if ids.len() == 0 {
            return Ok(false);
        }

        ids = wdp.update_from_paper_ids(&ids);
        match wdp.create_or_update_item_from_ids(&mut self.mw_api, &ids) {
            Some(er) => {
                if command.q == "" {
                    command.q = er.q;
                }
                Ok(true)
            }
            None => Ok(false),
        }
    }

    fn set_command_status(
        self: &mut Self,
        status: &str,
        message: Option<&str>,
        command: &mut SourceMDcommand,
    ) -> Result<(), String> {
        //println!("Setting {} to {}", &command.id, &status);
        let mut config = self.config.lock().map_err(|e| format!("{:?}", e))?;
        config
            .set_command_status(command, status, message.map(|s| s.to_string()))
            .ok_or(format!(
                "Can't config.set_command_status for batch #{}",
                self.batch_id
            ))?;
        Ok(())
    }

    fn get_next_command(&self) -> Result<Option<SourceMDcommand>, String> {
        let mut config = self.config.lock().map_err(|e| format!("{:?}", e))?;
        Ok(config.get_next_command(self.batch_id))
    }

    fn new_wdp(&self, command: &SourceMDcommand) -> WikidataPapers {
        let mut wdp = WikidataPapers::new(self.cache.clone());
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

    pub fn get_mw_api(ini_file: &str) -> mediawiki::api::Api {
        let mut mw_api = mediawiki::api::Api::new("https://www.wikidata.org/w/api.php").unwrap();

        let mut settings = Config::default();
        // File::with_name(..) is shorthand for File::from(Path::new(..))
        settings
            .merge(File::with_name(ini_file))
            .expect(format!("Config file '{}' can't be opened", ini_file).as_str());
        let lgname = settings.get_str("user.user").expect("No user.name");
        let lgpass = settings.get_str("user.pass").expect("No user.pass");
        mw_api.login(lgname, lgpass).unwrap();
        mw_api
    }
}

#[cfg(test)]
mod tests {
    //use super::*;
    //use mediawiki::api::Api;

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
    pub fn get_mw_api(ini_file: &str) -> mediawiki::api::Api {
    */
}
