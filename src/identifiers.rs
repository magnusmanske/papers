use std::str::FromStr;

const PROP_PMID: &str = "P698";
const PROP_PMCID: &str = "P932";
const PROP_DOI: &str = "P356";
const PROP_ARXIV: &str = "P818";
const PROP_SEMATIC_SCHOLAR: &str = "P4011";

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum IdProp {
    PMID,
    PMCID,
    DOI,
    ARXIV,
    SematicScholar,
}

impl FromStr for IdProp {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_uppercase().trim() {
            PROP_PMID => Ok(IdProp::PMID),
            PROP_PMCID => Ok(IdProp::PMCID),
            PROP_DOI => Ok(IdProp::DOI),
            PROP_ARXIV => Ok(IdProp::ARXIV),
            PROP_SEMATIC_SCHOLAR => Ok(IdProp::SematicScholar),
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
                IdProp::SematicScholar => PROP_SEMATIC_SCHOLAR,
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
            IdProp::SematicScholar => PROP_SEMATIC_SCHOLAR,
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
            IdProp::SematicScholar => id.to_lowercase(), // Sematic Scholar IDs are always lowercase
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
            IdProp::from_str(PROP_SEMATIC_SCHOLAR).unwrap(),
            IdProp::SematicScholar
        );
        assert!(IdProp::from_str("P123").is_err());
    }

    #[test]
    fn test_idprop_display() {
        assert_eq!(IdProp::PMID.to_string(), PROP_PMID);
        assert_eq!(IdProp::PMCID.to_string(), PROP_PMCID);
        assert_eq!(IdProp::DOI.to_string(), PROP_DOI);
        assert_eq!(IdProp::ARXIV.to_string(), PROP_ARXIV);
        assert_eq!(IdProp::SematicScholar.to_string(), PROP_SEMATIC_SCHOLAR);
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
}
