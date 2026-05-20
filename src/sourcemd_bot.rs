use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use tokio::sync::RwLock;

use self::sourcemd_command::SourceMDcommandMode;
use crate::{
    generic_author_info::GenericAuthorInfo,
    identifiers::{GenericWorkIdentifier, IdProp},
    sourcemd_command::SourceMDcommand,
    sourcemd_config::SourceMD,
    wikidata_papers::WikidataPapers,
    wikidata_string_cache::WikidataStringCache,
    *,
};

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
        let ret = Self { config, batch_id, cache };
        ret.start().await?;
        Ok(ret)
    }

    pub async fn start(&self) -> Result<()> {
        let config = self.config.read().await;
        config
            .restart_batch(self.batch_id)
            .await
            .with_context(|| format!("starting batch #{}", self.batch_id))?;
        config.set_batch_running(self.batch_id).await;
        Ok(())
    }

    pub async fn run(&self) -> Result<bool> {
        // Check if batch is still valid (STOP etc.)
        let mut command = match self.get_next_command().await? {
            Some(c) => c,
            None => {
                self.config
                    .read()
                    .await
                    .deactivate_batch_run(self.batch_id)
                    .await
                    .with_context(|| format!("deactivating batch #{}", self.batch_id))?;
                return Ok(false);
            },
        };

        self.set_command_status("RUNNING", None, &mut command).await?;
        match self.execute_command(&mut command).await {
            Ok(b) => {
                let status = if b { "DONE" } else { "DUNNO" };
                self.set_command_status(status, None, &mut command).await?;
                Ok(b)
            },
            Err(e) => {
                self.set_command_status("FAILED", Some(&e.to_string()), &mut command).await?;
                Err(e)
            },
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
            },
            SourceMDcommandMode::EditPaperForOrcidAuthor => Ok(false), // TODO
            SourceMDcommandMode::CreateBookFromIsbn => Ok(false),      // TODO
            other => Err(anyhow!("Unrecognized command '{}' on command #{}", other, command.id)),
        }
    }

    async fn get_author_item(&self, identifier: &str) -> Result<GenericAuthorInfo> {
        let mut author = GenericAuthorInfo::new();
        if crate::identifiers::is_qid(identifier) {
            author.set_wikidata_item(Some(identifier.to_owned()));
        } else if crate::identifiers::is_orcid(identifier) {
            author.prop2id_mut().insert("P496".to_string(), identifier.to_owned());
            author = author
                .get_or_create_author_item(
                    self.config.read().await.mw_api(),
                    self.cache.clone(),
                    false,
                )
                .await
        } else {
            return Err(anyhow!("Not a Wikidata item, nor an ORCID ID {}", identifier));
        }

        // Paranoia
        if author.wikidata_item().is_none() {
            return Err(anyhow!("Failed to get/create author item for {}", identifier));
        }

        Ok(author)
    }

    pub async fn process_author_metadata(&self, command: &mut SourceMDcommand) -> Result<bool> {
        let author = self.get_author_item(&command.identifier).await?;

        // Create paper object
        let mut wdp = self.new_wdp(command);
        wdp.set_edit_summary(Some(format!(
            "SourceMD [rust bot], [https://sourcemd.toolforge.org/?action=batch&batch={} batch #{}], command #{}",
            self.batch_id, self.batch_id, command.serial_number
        )));
        let config = self.config.read().await;
        wdp.update_author_items(&vec![author], config.mw_api()).await;
        Ok(true)
    }

    async fn process_paper(&self, command: &mut SourceMDcommand) -> Result<bool> {
        let mut wdp = self.new_wdp(command);
        wdp.set_edit_summary(Some(format!(
            "SourceMD [rust bot], [https://sourcemd.toolforge.org/?action=batch&batch={} batch #{}], command #{}",
            self.batch_id, self.batch_id, command.serial_number
        )));

        // Wikidata ID
        if crate::identifiers::is_qid(&command.identifier) {
            let result = wdp
                .create_or_update_item_from_q(
                    self.config.read().await.mw_api(),
                    &command.identifier,
                )
                .await
                .with_context(|| format!("update {}", command.identifier))?;
            return result
                .map(|_| true)
                .ok_or_else(|| anyhow!("Can't update {}", command.identifier));
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
        let result = wdp
            .create_or_update_item_from_ids(self.config.read().await.mw_api(), &ids)
            .await
            .with_context(|| format!("create_or_update for command #{}", command.id))?;
        match result {
            Some(er) => {
                if command.q.is_empty() {
                    command.q = er.q().to_string();
                }
                Ok(true)
            },
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
            .await
            .with_context(|| {
                format!(
                    "set_command_status({status}) for command {} in batch #{}",
                    command.id, self.batch_id
                )
            })
    }

    async fn get_next_command(&self) -> Result<Option<SourceMDcommand>> {
        self.config.read().await.get_next_command(self.batch_id).await
    }

    fn new_wdp(&self, _command: &SourceMDcommand) -> WikidataPapers {
        WikidataPapers::with_default_adapters(self.cache.clone())
    }
}

#[cfg(test)]
mod tests {
    use wikibase::mediawiki::api::Api;
    use wiremock::{
        matchers::{method, query_param},
        Mock, MockServer, ResponseTemplate,
    };

    use super::*;

    const SITEINFO: &str = include_str!("../test_data/api_siteinfo.json");

    async fn start_mock_server() -> MockServer {
        let mock_server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(query_param("meta", "siteinfo"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/json; charset=utf-8")
                    .set_body_string(SITEINFO),
            )
            .mount(&mock_server)
            .await;
        mock_server
    }

    /// Build a SourceMDbot directly from a wiremock-backed SourceMD with no DB
    /// pool — bypasses the normal `new()` which would call `restart_batch()`
    /// and fail without a DB.
    async fn make_bot(mock_server: &MockServer) -> SourceMDbot {
        let api = Api::new(&mock_server.uri()).await.unwrap();
        let mw_api = Arc::new(RwLock::new(api));
        let config = SourceMD::new_for_testing(mw_api.clone());
        let cache = Arc::new(WikidataStringCache::new(mw_api));
        SourceMDbot { config: Arc::new(RwLock::new(config)), cache, batch_id: 1 }
    }

    #[tokio::test]
    async fn execute_command_create_book_from_isbn_returns_false() {
        let mock_server = start_mock_server().await;
        let bot = make_bot(&mock_server).await;
        let mut cmd = SourceMDcommand::new_dummy("978-3-16-148410-0");
        cmd.mode = SourceMDcommandMode::CreateBookFromIsbn;
        let result = bot.execute_command(&mut cmd).await.unwrap();
        assert!(!result, "CreateBookFromIsbn is unimplemented and should return Ok(false)");
    }

    #[tokio::test]
    async fn execute_command_edit_paper_for_orcid_author_returns_false() {
        let mock_server = start_mock_server().await;
        let bot = make_bot(&mock_server).await;
        let mut cmd = SourceMDcommand::new_dummy("0000-0001-2345-6789");
        cmd.mode = SourceMDcommandMode::EditPaperForOrcidAuthor;
        let result = bot.execute_command(&mut cmd).await.unwrap();
        assert!(!result, "EditPaperForOrcidAuthor is unimplemented and should return Ok(false)");
    }

    #[tokio::test]
    async fn execute_command_add_orcid_metadata_returns_false() {
        // The current implementation short-circuits with `if true { Ok(false) }`,
        // so this should return false without touching the network.
        let mock_server = start_mock_server().await;
        let bot = make_bot(&mock_server).await;
        let mut cmd = SourceMDcommand::new_dummy("0000-0001-2345-6789");
        cmd.mode = SourceMDcommandMode::AddOrcidMetadataToAuthor;
        let result = bot.execute_command(&mut cmd).await.unwrap();
        assert!(!result);
    }

    #[tokio::test]
    async fn execute_command_dummy_mode_errors() {
        let mock_server = start_mock_server().await;
        let bot = make_bot(&mock_server).await;
        let mut cmd = SourceMDcommand::new_dummy("x");
        // mode defaults to Dummy, which is not a real command
        let err = bot.execute_command(&mut cmd).await.unwrap_err();
        assert!(err.to_string().contains("Unrecognized command"));
    }

    #[tokio::test]
    async fn get_next_command_with_no_pool_errors() {
        let mock_server = start_mock_server().await;
        let bot = make_bot(&mock_server).await;
        // After the P0-4 fix, "no pool" is an error rather than a silent None,
        // so the bot loop can distinguish "DB unavailable" from "no work".
        let err = bot.get_next_command().await.unwrap_err().to_string();
        assert!(err.contains("no MySQL pool"), "unexpected error: {err}");
    }

    #[tokio::test]
    async fn new_wdp_attaches_nine_adapters() {
        let mock_server = start_mock_server().await;
        let bot = make_bot(&mock_server).await;
        let cmd = SourceMDcommand::new_dummy("x");
        let mut wdp = bot.new_wdp(&cmd);
        // PMC, Pubmed, Crossref, Semanticscholar, Orcid, Arxiv, OpenAlex,
        // DataCite, EuropePMC = 9 adapters
        assert_eq!(wdp.adapters_mut().len(), 9);
    }

    #[tokio::test]
    async fn get_author_item_rejects_unknown_identifier() {
        let mock_server = start_mock_server().await;
        let bot = make_bot(&mock_server).await;
        // Not a Q-id and not an ORCID -> error
        let err = bot.get_author_item("not-an-id").await.unwrap_err();
        assert!(err.to_string().contains("Not a Wikidata item, nor an ORCID ID"));
    }

    #[tokio::test]
    async fn get_author_item_accepts_wikidata_qid() {
        let mock_server = start_mock_server().await;
        let bot = make_bot(&mock_server).await;
        // Pure-regex branch: a valid Q-id is set as the wikidata_item without
        // any further lookups.
        let author = bot.get_author_item("Q42").await.unwrap();
        assert_eq!(author.wikidata_item().map(str::to_string), Some("Q42".to_string()));
    }
}
