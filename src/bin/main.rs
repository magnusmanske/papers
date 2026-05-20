#[macro_use]
extern crate lazy_static;

use std::{io, io::prelude::*, sync::Arc, time::Duration};

use futures::prelude::*;
use papers::{
    arxiv2wikidata::Arxiv2Wikidata, author_name_string::AuthorNameString,
    crossref2wikidata::Crossref2Wikidata, datacite2wikidata::DataCite2Wikidata,
    europepmc2wikidata::EuropePMC2Wikidata, identifiers::GenericWorkIdentifier,
    openalex2wikidata::OpenAlex2Wikidata, orcid2wikidata::Orcid2Wikidata,
    pmc2wikidata::PMC2Wikidata, pubmed2wikidata::Pubmed2Wikidata,
    semanticscholar2wikidata::Semanticscholar2Wikidata, sourcemd_bot::SourceMDbot,
    sourcemd_config::SourceMD, wikidata_papers::WikidataPapers, *,
};
use pico_args::Arguments;
use rand::seq::SliceRandom;
use regex::Regex;
use tokio::sync::RwLock;
use wikibase::mediawiki::api::Api;

use crate::{sourcemd_command::SourceMDcommand, wikidata_string_cache::WikidataStringCache};

const INI_FILE: &str = "bot.ini";

async fn command_authors(ini_file: &str) {
    let smd = Arc::new(RwLock::new(SourceMD::new(ini_file).await.unwrap()));
    let mw_api = smd.read().await.mw_api();
    let cache = Arc::new(WikidataStringCache::new(mw_api.clone()));
    let stdin = io::stdin();
    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l.trim().to_string(),
            Err(_) => break,
        };
        if line.is_empty() {
            continue;
        }
        println!("Processing {}", line);
        author_from_id(&line, cache.clone(), smd.clone()).await;
    }
}

async fn author_from_id(id: &str, cache: Arc<WikidataStringCache>, smd: Arc<RwLock<SourceMD>>) {
    let mut command = SourceMDcommand::new_dummy(id);
    let bot = SourceMDbot::new(smd.clone(), cache.clone(), 0).await.unwrap();
    bot.process_author_metadata(&mut command).await.unwrap();
}

async fn command_ans(ini_file: &str) {
    const MAX_AUTHORS_IN_PARALLEL: usize = 5;
    let smd = Arc::new(RwLock::new(SourceMD::new(ini_file).await.unwrap()));
    let mw_api = smd.read().await.mw_api();
    let cache = Arc::new(WikidataStringCache::new(mw_api.clone()));
    let ans = AuthorNameString { logging_level: 2 };

    let mut futures: Vec<_> = io::stdin()
        .lock()
        .lines()
        // trunk-ignore(clippy/lines_filter_map_ok)
        .map_while(Result::ok)
        .map(|line| line.trim().to_string())
        .filter(|line| !line.is_empty())
        .map(|line| ans.process_author_q(line, &mw_api, &cache))
        .collect();
    futures.shuffle(&mut rand::rng());

    let stream = futures::stream::iter(futures).buffer_unordered(MAX_AUTHORS_IN_PARALLEL);
    stream.collect::<Vec<_>>().await;
}

async fn command_papers(ini_file: &str) {
    let mw_api = Arc::new(RwLock::new(SourceMD::create_mw_api(ini_file).await.unwrap()));
    let stdin = io::stdin();
    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l.trim().to_string(),
            Err(_) => break,
        };
        if line.is_empty() {
            continue;
        }
        // println!("Processing {}", &line);
        paper_from_id(&line, mw_api.clone()).await;
    }
}

async fn paper_from_id(id: &str, mw_api: Arc<RwLock<Api>>) {
    lazy_static! {
        static ref RE_WD: Regex =
            Regex::new(r#"^(Q\d+)$"#).expect("main.rs::paper_from_id: RE_WD does not compile");
    }

    let cache = Arc::new(WikidataStringCache::new(mw_api.clone()));

    let mut wdp = WikidataPapers::new(cache.clone());
    // wdp.testing = true;
    wdp.add_adapter(Box::new(PMC2Wikidata::new()));
    wdp.add_adapter(Box::new(Pubmed2Wikidata::new()));
    wdp.add_adapter(Box::new(Crossref2Wikidata::new()));
    wdp.add_adapter(Box::new(Semanticscholar2Wikidata::new()));
    wdp.add_adapter(Box::new(Orcid2Wikidata::new()));
    wdp.add_adapter(Box::new(Arxiv2Wikidata::new()));
    wdp.add_adapter(Box::new(OpenAlex2Wikidata::new()));
    wdp.add_adapter(Box::new(DataCite2Wikidata::new()));
    wdp.add_adapter(Box::new(EuropePMC2Wikidata::new()));

    if let Some(q) = RE_WD.captures(id).and_then(|c| c.get(1)) {
        save_item_changes(&mut wdp, mw_api.clone(), q.as_str()).await;
        return;
    }

    let mut ids = GenericWorkIdentifier::parse_ids_from_str(id);

    // Paranoia
    ids.retain(|id| !id.id().is_empty());

    if ids.is_empty() {
        println!("Can't find a valid ID in '{}'", id);
        return;
    }
    // println!("IDs: {:?}", &ids);
    ids = wdp.update_from_paper_ids(&ids).await;

    match wdp.create_or_update_item_from_ids(mw_api, &ids).await {
        Ok(Some(er)) => {
            if er.edited() {
                println!("Created or updated https://www.wikidata.org/wiki/{}", er.q())
            } else {
                println!("Exists as https://www.wikidata.org/wiki/{}, no changes ", er.q())
            }
        },
        Ok(None) => println!("No item ID for '{}'!", id),
        Err(e) => eprintln!("Error processing '{}': {:#}", id, e),
    }
}

async fn save_item_changes(wdp: &mut WikidataPapers, mw_api: Arc<RwLock<Api>>, q: &str) {
    match wdp.create_or_update_item_from_q(mw_api, q).await {
        Ok(Some(er)) => {
            if er.edited() {
                println!("Created or updated https://www.wikidata.org/wiki/{}", er.q())
            } else {
                println!("https://www.wikidata.org/wiki/{}, no changes ", er.q())
            }
        },
        Ok(None) => println!("No item ID!"),
        Err(e) => eprintln!("Error saving {}: {:#}", q, e),
    }
}

fn usage(prog: &str) {
    println!("USAGE: {} [--config <file>] <subcommand>", prog);
    println!("Subcommands: papers, authors, bot, ans");
    println!("  --config <file>  Configuration file (default: {})", INI_FILE);
    println!("                   For the `bot` subcommand the file must also");
    println!("                   contain a [client] section with `user` and");
    println!("                   `password` for the SourceMD MySQL DB.");
}

/// Outcome of one tick of the bot driver.
enum BotTick {
    /// A batch was found and processed; loop should poll again soon.
    Worked,
    /// No batch was found; loop should sleep on the "idle" cadence.
    Idle,
    /// The DB query for the next batch failed; treat as transient and back off
    /// on the "idle" cadence rather than spin tight.
    DbError,
}

async fn run_bot(config: Arc<RwLock<SourceMD>>, cache: Arc<WikidataStringCache>) -> BotTick {
    let batch_id = match config.read().await.get_next_batch().await {
        Ok(Some(n)) => n,
        Ok(None) => return BotTick::Idle,
        Err(e) => {
            // P0-5: previously a DB failure was indistinguishable from "no
            // work" and the bot would silently spin on a broken DB. Now we
            // log and treat it as an idle tick so the caller backs off.
            tracing::error!(error = %e, "get_next_batch failed; backing off");
            return BotTick::DbError;
        },
    };

    tracing::info!(batch_id, "starting batch");
    let bot = match SourceMDbot::new(config.clone(), cache.clone(), batch_id).await {
        Ok(bot) => bot,
        Err(error) => {
            tracing::error!(batch_id, error = %error, "failed to start bot for batch");
            config.read().await.set_batch_failed(batch_id).await;
            return BotTick::Idle;
        },
    };

    tracing::info!(batch_id, "batch spawned");
    loop {
        match bot.run().await {
            Ok(true) => continue,
            Ok(false) => break, // No more commands for this batch.
            Err(e) => {
                tracing::error!(batch_id, error = %e, "bot run failed; ending batch tick");
                break;
            },
        }
    }
    BotTick::Worked
}

async fn command_bot(ini_file: &str) {
    tracing::info!("starting bot mode");
    let mut smd = SourceMD::new(ini_file).await.unwrap();
    if let Err(e) = smd.init(ini_file).await {
        tracing::error!(error = %e, "SourceMD::init failed; aborting bot");
        std::process::exit(1);
    }
    let smd = Arc::new(RwLock::new(smd));
    let mw_api = Arc::new(RwLock::new(SourceMD::create_mw_api(ini_file).await.unwrap()));
    let cache = Arc::new(WikidataStringCache::new(mw_api));
    loop {
        let delay = match run_bot(smd.clone(), cache.clone()).await {
            BotTick::Worked => Duration::from_millis(1000),
            // Idle and DB-error tick get the same long delay; DB errors are
            // already logged inside run_bot so a broken DB is no longer silent.
            BotTick::Idle | BotTick::DbError => Duration::from_millis(5000),
        };
        tokio::time::sleep(delay).await;
    }
}

// For local testing:
// ssh magnus@tools-login.wmflabs.org -L 3307:tools-db:3306 -N &

fn init_tracing() {
    use tracing_subscriber::EnvFilter;
    // Honour RUST_LOG if set; otherwise default to INFO for our crate.
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt().with_env_filter(filter).try_init();
}

#[tokio::main]
async fn main() {
    init_tracing();
    let prog = std::env::args().next().unwrap_or_else(|| "papers".to_string());
    let mut pargs = Arguments::from_env();

    let config: String = pargs
        .opt_value_from_str("--config")
        .unwrap_or(None)
        .unwrap_or_else(|| INI_FILE.to_string());

    match pargs.subcommand().unwrap_or_default().as_deref() {
        Some("papers") => command_papers(&config).await,
        Some("authors") => command_authors(&config).await,
        Some("bot") => command_bot(&config).await,
        Some("ans") => command_ans(&config).await,
        _ => usage(&prog),
    }
}
