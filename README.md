# Papers

**Papers** is a Rust crate and binary to create and update Wikidata items about scientific publications, and their authors, from third-party sources.

## Install

- [Install Rust](https://www.rust-lang.org/tools/install)
- Clone this repo and `cd` into it
- Create a `bot.ini` file with a `[user]` section, and values for `user`(name) and `pass`(word) on Wikidata, preferably a bot user
- Run with `cargo run --release -- COMMAND`

## Sources

Currently, these sources are used by Papers:

- CrossRef
- ORCID
- PubMed
- PubMedCentral
- Semantic Scholar

## Commands

Available commands are:

### `papers`

Processes `STDIN` as publication IDs (eg DOIs), one per line. It will update or create the respective Wikidata items. Example:

```
echo '10.2147/JMDH.S446508' | cargo run --release -- papers
```

### `authors`

Processes `STDIN` as author IDs (eg ORCID), one per line. It will update or create the respective Wikidata items. Example:

```
echo '0000-0001-5916-0947' | cargo run --release -- authors
```

**Note** This currently requires a Toolforge database connection and does not work outside that ecosystem.

### `bot`

Runs a bot processing command batches in a database. Requires additional setup, not intended to be an end user functionality at this point.
