use regex::Regex;
use std::str::FromStr;

const PROP_PMID: &str = "P698";
const PROP_PMCID: &str = "P932";
const PROP_DOI: &str = "P356";
const PROP_ARXIV: &str = "P818";
const PROP_SEMANTIC_SCHOLAR: &str = "P4011";

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum IdProp {
    PMID,
    PMCID,
    DOI,
    ARXIV,
    SemanticScholar,
}

impl FromStr for IdProp {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_uppercase().trim() {
            PROP_PMID => Ok(IdProp::PMID),
            PROP_PMCID => Ok(IdProp::PMCID),
            PROP_DOI => Ok(IdProp::DOI),
            PROP_ARXIV => Ok(IdProp::ARXIV),
            PROP_SEMANTIC_SCHOLAR => Ok(IdProp::SemanticScholar),
            _ => Err(format!("Invalid ID property: {s}")),
        }
    }
}

impl std::fmt::Display for IdProp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            match self {
                IdProp::PMID => PROP_PMID,
                IdProp::PMCID => PROP_PMCID,
                IdProp::DOI => PROP_DOI,
                IdProp::ARXIV => PROP_ARXIV,
                IdProp::SemanticScholar => PROP_SEMANTIC_SCHOLAR,
            }
        )
    }
}

impl IdProp {
    pub fn as_str(&self) -> &str {
        match self {
            IdProp::PMID => PROP_PMID,
            IdProp::PMCID => PROP_PMCID,
            IdProp::DOI => PROP_DOI,
            IdProp::ARXIV => PROP_ARXIV,
            IdProp::SemanticScholar => PROP_SEMANTIC_SCHOLAR,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum GenericWorkType {
    Property(IdProp),
    Item,
}

impl std::fmt::Display for GenericWorkType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            match self {
                GenericWorkType::Property(prop) => prop.to_string(),
                GenericWorkType::Item => "Item".to_string(),
            }
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct GenericWorkIdentifier {
    work_type: GenericWorkType,
    id: String,
}

impl GenericWorkIdentifier {
    pub fn new_prop(prop: IdProp, id: &str) -> Self {
        let id = match &prop {
            IdProp::DOI => id.to_uppercase(), // DOIs are always uppercase
            IdProp::SemanticScholar => id.to_lowercase(), // Semantic Scholar IDs are always lowercase
            _other => id.to_string(),
        };
        Self {
            work_type: GenericWorkType::Property(prop),
            id,
        }
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn is_legit(&self) -> bool {
        !self.id.is_empty() && self.id != "0"
    }

    pub fn work_type(&self) -> &GenericWorkType {
        &self.work_type
    }

    /// Parses a free-form identifier string into zero or more `GenericWorkIdentifier`s.
    /// Recognises DOIs (`xx/yy`), PubMed IDs (digits only) and PMC IDs (`PMCnnn`).
    /// Q-items are intentionally excluded; callers handle those separately.
    pub fn parse_ids_from_str(s: &str) -> Vec<Self> {
        lazy_static::lazy_static! {
            static ref RE_DOI:   Regex = Regex::new(r#"^(.+/.+)$"#).expect("RE_DOI");
            static ref RE_PMID:  Regex = Regex::new(r#"^(\d+)$"#).expect("RE_PMID");
            static ref RE_PMCID: Regex = Regex::new(r#"^(PMC\d+)$"#).expect("RE_PMCID");
        }
        let mut ids = vec![];
        if let Some(x) = RE_DOI.captures(s).and_then(|c| c.get(1)) {
            ids.push(Self::new_prop(IdProp::DOI, x.as_str()));
        }
        if let Some(x) = RE_PMID.captures(s).and_then(|c| c.get(1)) {
            ids.push(Self::new_prop(IdProp::PMID, x.as_str()));
        }
        if let Some(x) = RE_PMCID.captures(s).and_then(|c| c.get(1)) {
            ids.push(Self::new_prop(IdProp::PMCID, x.as_str()));
        }
        ids
    }
}

/// Returns `true` if `id` consists entirely of ASCII digits (i.e. is a raw PubMed ID).
pub fn is_pubmed_id(id: &str) -> bool {
    !id.is_empty() && id.chars().all(|c| c.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_idprop_from_str() {
        assert_eq!(IdProp::from_str(PROP_PMID).unwrap(), IdProp::PMID);
        assert_eq!(IdProp::from_str(PROP_PMCID).unwrap(), IdProp::PMCID);
        assert_eq!(IdProp::from_str(PROP_DOI).unwrap(), IdProp::DOI);
        assert_eq!(IdProp::from_str(PROP_ARXIV).unwrap(), IdProp::ARXIV);
        assert_eq!(
            IdProp::from_str(PROP_SEMANTIC_SCHOLAR).unwrap(),
            IdProp::SemanticScholar
        );
        assert!(IdProp::from_str("P123").is_err());
    }

    #[test]
    fn test_idprop_display() {
        assert_eq!(IdProp::PMID.to_string(), PROP_PMID);
        assert_eq!(IdProp::PMCID.to_string(), PROP_PMCID);
        assert_eq!(IdProp::DOI.to_string(), PROP_DOI);
        assert_eq!(IdProp::ARXIV.to_string(), PROP_ARXIV);
        assert_eq!(IdProp::SemanticScholar.to_string(), PROP_SEMANTIC_SCHOLAR);
    }

    #[test]
    fn test_genericworkidentifier_new_prop() {
        let prop = IdProp::DOI;
        let id = "10.1234/foobar";
        let gwi = GenericWorkIdentifier::new_prop(prop.to_owned(), id);
        assert_eq!(gwi.id(), id.to_uppercase());
        assert_eq!(gwi.work_type(), &GenericWorkType::Property(prop));
    }

    #[test]
    fn test_genericworkidentifier_is_legit() {
        let prop = IdProp::DOI;
        let id = "10.1234/foobar";
        let gwi = GenericWorkIdentifier::new_prop(prop.to_owned(), id);
        assert!(gwi.is_legit());
        let gwi = GenericWorkIdentifier::new_prop(prop.to_owned(), "");
        assert!(!gwi.is_legit());
        let gwi = GenericWorkIdentifier::new_prop(prop.to_owned(), "0");
        assert!(!gwi.is_legit());
    }

    #[test]
    fn test_genericworkidentifier_work_type() {
        let prop = IdProp::DOI;
        let id = "10.1234/foobar";
        let gwi = GenericWorkIdentifier::new_prop(prop.to_owned(), id);
        assert_eq!(gwi.work_type(), &GenericWorkType::Property(prop));
    }

    #[test]
    fn test_genericworktype_display() {
        assert_eq!(GenericWorkType::Item.to_string(), "Item");
        assert_eq!(GenericWorkType::Property(IdProp::DOI).to_string(), PROP_DOI);
    }

    // === IdProp::as_str ===

    #[test]
    fn test_idprop_as_str() {
        assert_eq!(IdProp::PMID.as_str(), PROP_PMID);
        assert_eq!(IdProp::PMCID.as_str(), PROP_PMCID);
        assert_eq!(IdProp::DOI.as_str(), PROP_DOI);
        assert_eq!(IdProp::ARXIV.as_str(), PROP_ARXIV);
        assert_eq!(IdProp::SemanticScholar.as_str(), PROP_SEMANTIC_SCHOLAR);
    }

    // === parse_ids_from_str ===

    #[test]
    fn parse_ids_from_str_doi() {
        let ids = GenericWorkIdentifier::parse_ids_from_str("10.1234/foobar");
        assert_eq!(ids.len(), 1);
        assert_eq!(ids[0].id(), "10.1234/FOOBAR"); // DOIs are uppercased
        assert_eq!(
            ids[0].work_type(),
            &GenericWorkType::Property(IdProp::DOI)
        );
    }

    #[test]
    fn parse_ids_from_str_pmid() {
        let ids = GenericWorkIdentifier::parse_ids_from_str("12345678");
        assert_eq!(ids.len(), 1);
        assert_eq!(ids[0].id(), "12345678");
        assert_eq!(
            ids[0].work_type(),
            &GenericWorkType::Property(IdProp::PMID)
        );
    }

    #[test]
    fn parse_ids_from_str_pmcid() {
        let ids = GenericWorkIdentifier::parse_ids_from_str("PMC12345");
        assert_eq!(ids.len(), 1);
        assert_eq!(ids[0].id(), "PMC12345");
        assert_eq!(
            ids[0].work_type(),
            &GenericWorkType::Property(IdProp::PMCID)
        );
    }

    #[test]
    fn parse_ids_from_str_empty_returns_nothing() {
        let ids = GenericWorkIdentifier::parse_ids_from_str("");
        assert!(ids.is_empty());
    }

    #[test]
    fn parse_ids_from_str_q_item_excluded() {
        // Q-items are intentionally not parsed by this function
        let ids = GenericWorkIdentifier::parse_ids_from_str("Q12345");
        assert!(ids.is_empty());
    }

    #[test]
    fn parse_ids_from_str_doi_is_uppercased() {
        let ids = GenericWorkIdentifier::parse_ids_from_str("10.1000/xyz123");
        assert!(ids.iter().any(|id| id.id() == "10.1000/XYZ123"));
    }

    #[test]
    fn parse_ids_from_str_doi_with_slash_only() {
        // A slash-containing string should be recognised as a DOI
        let ids = GenericWorkIdentifier::parse_ids_from_str("a/b");
        assert!(ids
            .iter()
            .any(|id| id.work_type() == &GenericWorkType::Property(IdProp::DOI)));
    }
}
