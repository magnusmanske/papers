//! Typed errors for the `papers` crate.
//!
//! Every fallible public entry point returns [`Result<T>`], which carries an
//! [`Error`] with a variant for each upstream service and each domain failure
//! mode this crate can produce. Upstream typed errors (Wikibase, MediaWiki,
//! Wikidata, SPARQL, MySQL, ToolsDB, `config`, Tokio) are preserved as
//! [`thiserror::Error`] sources rather than flattened into strings.

use thiserror::Error;
use wikibase::{mediawiki::MediaWikiError, WikibaseError};
use wikimisc::{
    mysql_async::Error as MySqlError, sparql_client::SparqlError, toolforge,
    wikidata::WikidataError,
};

/// The result type used throughout the crate.
pub type Result<T> = std::result::Result<T, Error>;

/// Everything that can go wrong in this crate.
#[derive(Debug, Error)]
pub enum Error {
    // --- Upstream services ----------------------------------------------
    /// A MediaWiki API call failed (for example, logging in).
    #[error(transparent)]
    MediaWiki(#[from] MediaWikiError),

    /// A Wikibase entity or diff operation failed.
    #[error(transparent)]
    Wikibase(#[from] WikibaseError),

    /// A `wikimisc` Wikidata client call failed.
    #[error(transparent)]
    Wikidata(#[from] WikidataError),

    /// A SPARQL query failed.
    #[error(transparent)]
    Sparql(#[from] SparqlError),

    /// Reading the configuration file failed.
    #[error(transparent)]
    Config(#[from] config::ConfigError),

    /// A blocking task panicked or was cancelled.
    #[error("background task failed: {0}")]
    Task(#[from] tokio::task::JoinError),

    // --- Database operations --------------------------------------------
    /// A named MySQL query or transaction failed.
    #[error("{operation}: {source}")]
    Query {
        /// What was being attempted, e.g. `"restart_batch: opening txn"`.
        operation: String,
        /// The underlying MySQL error.
        #[source]
        source: MySqlError,
    },

    /// Checking out a ToolsDB connection failed.
    #[error("{operation}: {source}")]
    Connection {
        /// What was being attempted.
        operation: String,
        /// The underlying ToolsDB error.
        #[source]
        source: toolforge::Error,
    },

    /// The configuration file could not be read or parsed.
    #[error("reading configuration from '{path}': {source}")]
    ConfigFile {
        /// The requested configuration file.
        path: String,
        /// The underlying `config` error.
        #[source]
        source: config::ConfigError,
    },

    // --- Domain errors ---------------------------------------------------
    /// A database-backed operation needs a pool, but `init()` has not run.
    #[error("no MySQL pool configured")]
    NoDatabasePool,

    /// A batch is no longer `RUNNING` or `TODO`, so the bot must stop.
    #[error("batch #{batch_id} is not RUNNING or TODO")]
    BatchNotRunning {
        /// The batch that was checked.
        batch_id: i64,
    },

    /// A command mode has no implementation in the bot.
    #[error("Unrecognized command '{mode}' on command #{command_id}")]
    UnrecognizedCommand {
        /// The command mode as stored in the database.
        mode: String,
        /// The command row id.
        command_id: i64,
    },

    /// An author identifier is neither a Wikidata item nor an ORCID.
    #[error("Not a Wikidata item, nor an ORCID ID {0}")]
    InvalidAuthorIdentifier(String),

    /// An author item could not be found or created.
    #[error("Failed to get/create author item for {0}")]
    AuthorItemCreation(String),

    /// An identifier could not be turned into a Wikidata item.
    #[error("Can't update {0}")]
    CannotUpdate(String),

    /// A SPARQL result is missing its `results.bindings` array.
    #[error("{context}: Not an array")]
    InvalidSparqlResult {
        /// The operation that produced the malformed result.
        context: &'static str,
    },

    /// A string that should be a Wikidata Q-id is not.
    #[error("not a Q-id: {0}")]
    InvalidQid(String),

    /// Two author records carry different Wikidata items and cannot merge.
    #[error("Cannot merge authors with different Wikidata items {first:?} and {second:?}")]
    ConflictingAuthorItems {
        /// The item already present on the receiver.
        first: Option<String>,
        /// The conflicting item on the other author.
        second: Option<String>,
    },

    /// A Wikidata entity could not be loaded.
    #[error("failed to load entity {q}: {source}")]
    LoadEntity {
        /// The entity that was requested.
        q: String,
        /// The underlying Wikibase error.
        #[source]
        source: WikibaseError,
    },
}

impl Error {
    /// Records a MySQL operation failure.
    pub(crate) fn query(operation: impl Into<String>, source: MySqlError) -> Self {
        Self::Query {
            operation: operation.into(),
            source,
        }
    }

    /// Records a ToolsDB connection failure.
    pub(crate) fn connection(operation: impl Into<String>, source: toolforge::Error) -> Self {
        Self::Connection {
            operation: operation.into(),
            source,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_keeps_domain_messages_stable() {
        assert_eq!(
            Error::NoDatabasePool.to_string(),
            "no MySQL pool configured"
        );
        assert_eq!(
            Error::BatchNotRunning { batch_id: 7 }.to_string(),
            "batch #7 is not RUNNING or TODO"
        );
        assert_eq!(
            Error::UnrecognizedCommand {
                mode: "DUMMY".to_string(),
                command_id: 3,
            }
            .to_string(),
            "Unrecognized command 'DUMMY' on command #3"
        );
        assert_eq!(
            Error::InvalidAuthorIdentifier("x".to_string()).to_string(),
            "Not a Wikidata item, nor an ORCID ID x"
        );
        assert_eq!(
            Error::InvalidSparqlResult {
                context: "get_coauthor_qs"
            }
            .to_string(),
            "get_coauthor_qs: Not an array"
        );
    }

    #[test]
    fn config_errors_convert() {
        let err: Error = config::ConfigError::Message("bad".to_string()).into();
        assert!(matches!(err, Error::Config(_)));
    }
}
