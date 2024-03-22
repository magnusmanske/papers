use std::str::FromStr;

use mysql as my;

#[derive(Debug, Clone, PartialEq)]
pub enum SourceMDcommandMode {
    Dummy,
    CreatePaperById,
    AddAutthorToPublication,
    AddOrcidMetadataToAuthor,
    EditPaperForOrcidAuthor,
    CreateBookFromIsbn,
}

impl FromStr for SourceMDcommandMode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_uppercase().trim() {
            "CREATE_PAPER_BY_ID" => Ok(SourceMDcommandMode::CreatePaperById),
            "ADD_AUTHOR_TO_PUBLICATION" => Ok(SourceMDcommandMode::AddAutthorToPublication),
            "ADD_METADATA_FROM_ORCID_TO_AUTHOR" => {
                Ok(SourceMDcommandMode::AddOrcidMetadataToAuthor)
            }
            "EDIT_PAPER_FOR_ORCID_AUTHOR" => Ok(SourceMDcommandMode::EditPaperForOrcidAuthor),
            "CREATE_BOOK_FROM_ISBN" => Ok(SourceMDcommandMode::CreateBookFromIsbn),
            _ => Err(format!("Invalid command: {s}")),
        }
    }
}

impl std::fmt::Display for SourceMDcommandMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            match self {
                SourceMDcommandMode::Dummy => "DUMMY",
                SourceMDcommandMode::CreatePaperById => "CREATE_PAPER_BY_ID",
                SourceMDcommandMode::AddAutthorToPublication => "ADD_AUTHOR_TO_PUBLICATION",
                SourceMDcommandMode::AddOrcidMetadataToAuthor =>
                    "ADD_METADATA_FROM_ORCID_TO_AUTHOR",
                SourceMDcommandMode::EditPaperForOrcidAuthor => "EDIT_PAPER_FOR_ORCID_AUTHOR",
                SourceMDcommandMode::CreateBookFromIsbn => "CREATE_BOOK_FROM_ISBN",
            }
        )
    }
}

#[derive(Debug, Clone)]
pub struct SourceMDcommand {
    pub id: i64,
    pub batch_id: i64,
    pub serial_number: i64,
    pub mode: SourceMDcommandMode,
    pub identifier: String,
    pub status: String,
    pub note: String,
    pub q: String,
    pub auto_escalate: bool,
}

impl SourceMDcommand {
    pub fn new_dummy(identifier: &str) -> Self {
        Self {
            id: 0,
            batch_id: 0,
            serial_number: 0,
            mode: SourceMDcommandMode::Dummy,
            identifier: identifier.to_string(),
            status: "TODO".to_string(),
            note: "".to_string(),
            q: "".to_string(),
            auto_escalate: false,
        }
    }

    pub fn new_from_row(row: my::Row) -> Option<Self> {
        Some(Self {
            id: SourceMDcommand::rowvalue_as_i64(&row["id"]),
            batch_id: SourceMDcommand::rowvalue_as_i64(&row["batch_id"]),
            serial_number: SourceMDcommand::rowvalue_as_i64(&row["serial_number"]),
            mode: SourceMDcommandMode::from_str(&SourceMDcommand::rowvalue_as_string(&row["mode"]))
                .ok()?,
            identifier: SourceMDcommand::rowvalue_as_string(&row["identifier"]),
            status: SourceMDcommand::rowvalue_as_string(&row["status"]),
            note: SourceMDcommand::rowvalue_as_string(&row["note"]),
            q: SourceMDcommand::rowvalue_as_string(&row["q"]),
            auto_escalate: SourceMDcommand::rowvalue_as_i64(&row["auto_escalate"]) == 1,
        })
    }

    fn rowvalue_as_i64(v: &my::Value) -> i64 {
        match v {
            my::Value::Int(x) => *x,
            _ => 0,
        }
    }

    fn rowvalue_as_string(v: &my::Value) -> String {
        match v {
            my::Value::Bytes(x) => String::from_utf8_lossy(x).to_string(),
            _ => String::from(""),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    //use wikibase::mediawiki::api::Api;

    #[test]
    fn test_new_dummy() {
        let cmd = SourceMDcommand::new_dummy("123");
        assert_eq!(cmd.id, 0);
        assert_eq!(cmd.batch_id, 0);
        assert_eq!(cmd.serial_number, 0);
        assert_eq!(cmd.mode, SourceMDcommandMode::Dummy);
        assert_eq!(cmd.identifier, "123");
        assert_eq!(cmd.status, "TODO");
        assert_eq!(cmd.note, "");
        assert_eq!(cmd.q, "");
        assert_eq!(cmd.auto_escalate, false);
    }

    #[test]
    fn test_rowvalue_as_i64() {
        let v = my::Value::Int(123);
        assert_eq!(SourceMDcommand::rowvalue_as_i64(&v), 123);
    }

    #[test]
    fn test_rowvalue_as_string() {
        let v = my::Value::Bytes(b"abc".to_vec());
        assert_eq!(SourceMDcommand::rowvalue_as_string(&v), "abc");
    }

    /*
    TODO:
    pub fn new_from_row(row: my::Row) -> Self {
    */
}
