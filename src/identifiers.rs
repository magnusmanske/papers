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

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct GenericWorkIdentifier {
    work_type: GenericWorkType,
    id: String,
}

impl GenericWorkIdentifier {
    pub fn new_prop(prop: IdProp, id: &str) -> Self {
        let id = match &prop {
            IdProp::DOI => id.to_uppercase(), // DOIs are always uppercase
            _other => id.to_string(),
        };
        Self {
            work_type: GenericWorkType::Property(prop),
            id,
        }
    }

    pub fn id(&self) -> String {
        match self.work_type {
            GenericWorkType::Property(IdProp::DOI) => self.id.to_uppercase(),
            _ => self.id.to_owned(),
        }
    }

    pub fn is_legit(&self) -> bool {
        !self.id.is_empty() && self.id != "0"
    }

    pub fn work_type(&self) -> &GenericWorkType {
        &self.work_type
    }
}
