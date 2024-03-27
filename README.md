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

### `ans` (author name string, P2093)

Processes `STDIN` as author QIDs, one per line. Gets all P50 and P2093 co-authors for each author.
For each P2093 co-author on at least 2 papers, it will

- change it to P50 if there is a P50 co-author with the same name already
- create a new author item if this name does not have a search hit, and change it to the new P50 author

```
echo '0000-0001-5916-0947' | cargo run --release -- authors
```

**Note** This currently requires a Toolforge database connection and does not work outside that ecosystem.

### `authors`

Processes `STDIN` as author IDs (eg ORCID), one per line. It will update or create the respective Wikidata items. Example:

```
echo '0000-0001-5916-0947' | cargo run --release -- authors
```

**Note** This currently requires a Toolforge database connection and does not work outside that ecosystem.

### `bot`

Runs a bot processing command batches in a database. Requires additional setup, not intended to be an end user functionality at this point.
