use crate::crossref2wikidata::Crossref2Wikidata;
use crate::orcid2wikidata::Orcid2Wikidata;
use crate::pubmed2wikidata::Pubmed2Wikidata;
use crate::semanticscholar2wikidata::Semanticscholar2Wikidata;
use crate::sourcemd_command::SourceMDcommand;
use crate::sourcemd_config::SourceMD;
use crate::wikidata_papers::WikidataPapers;
use config::{Config, File};
use std::sync::{Arc, Mutex};
//use regex::Regex;

#[derive(Debug, Clone)]
pub struct SourceMDbot {
    mw_api: mediawiki::api::Api,
    config: Arc<Mutex<SourceMD>>,
    batch_id: i64,
}

impl SourceMDbot {
    pub fn new(config: Arc<Mutex<SourceMD>>, batch_id: i64) -> Self {
        Self {
            config,
            batch_id,
            mw_api: SourceMDbot::get_mw_api("bot.ini"),
        }
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
            None => return Ok(false),
        };

        self.set_command_status("RUNNING", None, &mut command)?;
        match self.execute_command(&mut command) {
            Ok(b) => {
                if b {
                    self.set_command_status("DONE", None, &mut command)?;
                    Ok(true)
                } else {
                    // TODO
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
        let wdp = self.new_wdp();

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
        println!("Processing command {:?}", &command);
        Ok(false)
    }

    fn set_command_status(
        self: &mut Self,
        status: &str,
        message: Option<&str>,
        command: &mut SourceMDcommand,
    ) -> Result<(), String> {
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

    fn new_wdp(&self) -> WikidataPapers {
        let mut wdp = WikidataPapers::new();
        wdp.add_adapter(Box::new(Pubmed2Wikidata::new()));
        wdp.add_adapter(Box::new(Crossref2Wikidata::new()));
        wdp.add_adapter(Box::new(Semanticscholar2Wikidata::new()));
        wdp.add_adapter(Box::new(Orcid2Wikidata::new()));
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
