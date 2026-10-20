//! The Wikidata Query Service.
//!
//! Wikidata keeps scholarly articles in a graph of their own, apart from
//! everything else — authors and their labels included. A query goes to the
//! graph that holds what it matches, and one that needs both is split. This
//! is so on WDQS v1 (Blazegraph) and v2 (QLever) alike.
//!
//! The endpoints are v1 for now: v2 refuses user agents it has not
//! allow-listed until its public launch. Queries are plain SPARQL 1.1, with no
//! label service, so they run on both; switching is the two presets in
//! [`Wdqs::default`] (`SparqlEndpoint::wikidata_main` / `wikidata_scholarly`).

use wikimisc::sparql_client::SparqlClient;
use wikimisc::sparql_endpoint::SparqlEndpoint;

use crate::http_client::sparql_http_client;

/// Retries of a query that failed in a way that may go away on its own
/// (rate limiting, 5xx); timeouts and malformed queries are not retried.
const RETRIES: u32 = 2;

/// Clients for Wikidata's two query graphs.
#[derive(Debug, Clone)]
pub struct Wdqs {
    main: SparqlClient,
    scholarly: SparqlClient,
}

impl Default for Wdqs {
    fn default() -> Self {
        Self::new(SparqlEndpoint::wdqs(), SparqlEndpoint::wdqs_scholarly())
    }
}

impl Wdqs {
    pub fn new(main: SparqlEndpoint, scholarly: SparqlEndpoint) -> Self {
        let client = |endpoint| {
            SparqlClient::with_client(endpoint, sparql_http_client().clone()).with_retries(RETRIES)
        };
        Self {
            main: client(main),
            scholarly: client(scholarly),
        }
    }

    /// Everything but scholarly articles: people, their labels, languages.
    pub fn main(&self) -> &SparqlClient {
        &self.main
    }

    /// Scholarly articles and their statements, such as authors (P50) and
    /// author name strings (P2093).
    pub fn scholarly(&self) -> &SparqlClient {
        &self.scholarly
    }
}
