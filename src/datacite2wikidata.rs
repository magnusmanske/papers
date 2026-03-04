use crate::generic_author_info::GenericAuthorInfo;
use crate::scientific_publication_adapter::ScientificPublicationAdapter;
use crate::*;
use async_trait::async_trait;
use std::collections::HashMap;

use self::identifiers::{GenericWorkIdentifier, GenericWorkType, IdProp};

pub struct DataCite2Wikidata {
    author_cache: HashMap<String, String>,
    work_cache: HashMap<String, serde_json::Value>,
}

impl Default for DataCite2Wikidata {
    fn default() -> Self {
        Self::new()
    }
}

impl DataCite2Wikidata {
    pub fn new() -> Self {
        DataCite2Wikidata {
            author_cache: HashMap::new(),
            work_cache: HashMap::new(),
        }
    }

    pub fn get_cached_publication_from_id(
        &self,
        publication_id: &str,
    ) -> Option<&serde_json::Value> {
        self.work_cache.get(publication_id)
    }

    /// Returns the `data.attributes` object from the cached JSON:API response.
    fn get_attributes(&self, publication_id: &str) -> Option<&serde_json::Value> {
        let work = self.get_cached_publication_from_id(publication_id)?;
        work["data"]["attributes"]
            .as_object()
            .map(|_| &work["data"]["attributes"])
    }

    async fn fetch_work_by_doi(&mut self, doi: &str) -> Option<String> {
        let url = format!("https://api.datacite.org/dois/{}", doi);
        let json: serde_json::Value = reqwest::get(&url).await.ok()?.json().await.ok()?;
        // Verify we got a valid response with data
        json["data"]["attributes"].as_object()?;
        let pub_id = doi.to_uppercase();
        self.work_cache.insert(pub_id.clone(), json);
        Some(pub_id)
    }

    /// Maps a DataCite resourceTypeGeneral value to a Wikidata Q-item.
    fn datacite_type_to_q(resource_type: &str) -> Option<&'static str> {
        match resource_type {
            "Book" => Some("Q571"),
            "BookChapter" => Some("Q1980247"),
            "ConferencePaper" => Some("Q23927052"),
            "ConferenceProceeding" => Some("Q1143604"),
            "Dataset" => Some("Q1172284"),
            "Dissertation" => Some("Q187685"),
            "JournalArticle" | "Journal Article" => Some("Q13442814"),
            "Preprint" => Some("Q580922"),
            "Report" => Some("Q10870555"),
            "Standard" => Some("Q317623"),
            "Software" => Some("Q7397"),
            _ => None,
        }
    }
}

#[async_trait(?Send)]
impl ScientificPublicationAdapter for DataCite2Wikidata {
    fn name(&self) -> &str {
        "DataCite2Wikidata"
    }

    fn author_cache(&self) -> &HashMap<String, String> {
        &self.author_cache
    }

    fn author_cache_mut(&mut self) -> &mut HashMap<String, String> {
        &mut self.author_cache
    }

    async fn get_identifier_list(
        &mut self,
        ids: &[GenericWorkIdentifier],
    ) -> Vec<GenericWorkIdentifier> {
        let mut ret: Vec<GenericWorkIdentifier> = vec![];
        for id in ids {
            if let GenericWorkType::Property(prop) = id.work_type() {
                if *prop == IdProp::DOI {
                    if let Some(pub_id) = self.fetch_work_by_doi(id.id()).await {
                        // DataCite only has DOI
                        ret.push(GenericWorkIdentifier::new_prop(IdProp::DOI, &pub_id));
                    }
                }
            }
        }
        ret
    }

    async fn publication_id_from_item(&mut self, item: &Entity) -> Option<String> {
        let doi = self.get_external_identifier_from_item(item, &IdProp::DOI)?;
        self.fetch_work_by_doi(&doi).await
    }

    fn get_work_titles(&self, publication_id: &str) -> Vec<LocaleString> {
        let attrs = match self.get_attributes(publication_id) {
            Some(a) => a,
            None => return vec![],
        };
        match attrs["titles"].as_array() {
            Some(titles) if !titles.is_empty() => {
                if let Some(title) = titles[0]["title"].as_str() {
                    if !title.is_empty() {
                        return vec![LocaleString::new("en", title)];
                    }
                }
                vec![]
            }
            _ => vec![],
        }
    }

    fn get_work_type(&self, publication_id: &str) -> Option<String> {
        let attrs = self.get_attributes(publication_id)?;
        let resource_type = attrs["types"]["resourceTypeGeneral"].as_str()?;
        Self::datacite_type_to_q(resource_type).map(|s| s.to_string())
    }

    fn get_publication_date(&self, publication_id: &str) -> Option<(u32, Option<u8>, Option<u8>)> {
        let attrs = self.get_attributes(publication_id)?;
        // Try publicationYear first (always available)
        let year: u32 = attrs["publicationYear"].as_u64()? as u32;
        // Try to get more precise date from dates array
        if let Some(dates) = attrs["dates"].as_array() {
            for date_entry in dates {
                if date_entry["dateType"].as_str() == Some("Issued") {
                    if let Some(date_str) = date_entry["date"].as_str() {
                        let parts: Vec<&str> = date_str.split('-').collect();
                        if parts.len() >= 2 {
                            let month: Option<u8> = parts.get(1).and_then(|s| s.parse().ok());
                            let day: Option<u8> = parts.get(2).and_then(|s| s.parse().ok());
                            return Some((year, month, day));
                        }
                    }
                }
            }
        }
        Some((year, None, None))
    }

    async fn get_author_list(&mut self, publication_id: &str) -> Vec<GenericAuthorInfo> {
        let attrs = match self.get_attributes(publication_id) {
            Some(a) => a.clone(),
            None => return vec![],
        };
        let creators = match attrs["creators"].as_array() {
            Some(c) => c,
            None => return vec![],
        };
        creators
            .iter()
            .enumerate()
            .filter_map(|(num, creator)| {
                // Try familyName + givenName, fall back to name
                let name = match (
                    creator["givenName"].as_str(),
                    creator["familyName"].as_str(),
                ) {
                    (Some(given), Some(family)) => format!("{} {}", given, family),
                    _ => creator["name"].as_str()?.to_string(),
                };
                if name.is_empty() {
                    return None;
                }
                let mut entry = GenericAuthorInfo::new_from_name_num(&name, num + 1);
                // Check for ORCID in nameIdentifiers
                if let Some(identifiers) = creator["nameIdentifiers"].as_array() {
                    for ni in identifiers {
                        if ni["nameIdentifierScheme"].as_str() == Some("ORCID") {
                            if let Some(orcid) = ni["nameIdentifier"].as_str() {
                                // May be full URL or bare ID
                                let orcid =
                                    orcid.strip_prefix("https://orcid.org/").unwrap_or(orcid);
                                if !orcid.is_empty() {
                                    entry.prop2id.insert("P496".to_string(), orcid.to_string());
                                }
                            }
                        }
                    }
                }
                Some(entry)
            })
            .collect()
    }

    async fn update_statements_for_publication_id(
        &self,
        _publication_id: &str,
        _item: &mut Entity,
    ) {
        // No extra statements beyond the defaults
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_datacite_work() -> serde_json::Value {
        json!({
            "data": {
                "attributes": {
                    "doi": "10.5281/zenodo.1234567",
                    "titles": [
                        {"title": "Test Dataset Title"}
                    ],
                    "types": {
                        "resourceTypeGeneral": "Dataset",
                        "resourceType": "Dataset"
                    },
                    "publicationYear": 2023,
                    "dates": [
                        {"date": "2023-03-15", "dateType": "Issued"},
                        {"date": "2023-03-10", "dateType": "Created"}
                    ],
                    "creators": [
                        {
                            "givenName": "Alice",
                            "familyName": "Smith",
                            "name": "Smith, Alice",
                            "nameIdentifiers": [
                                {
                                    "nameIdentifier": "https://orcid.org/0000-0001-2345-6789",
                                    "nameIdentifierScheme": "ORCID"
                                }
                            ]
                        },
                        {
                            "givenName": "Bob",
                            "familyName": "Jones",
                            "name": "Jones, Bob",
                            "nameIdentifiers": []
                        }
                    ]
                }
            }
        })
    }

    #[test]
    fn test_name() {
        let adapter = DataCite2Wikidata::new();
        assert_eq!(adapter.name(), "DataCite2Wikidata");
    }

    #[test]
    fn test_get_work_titles() {
        let mut adapter = DataCite2Wikidata::new();
        adapter
            .work_cache
            .insert("10.5281/ZENODO.1234567".to_string(), make_datacite_work());
        let titles = adapter.get_work_titles("10.5281/ZENODO.1234567");
        assert_eq!(titles.len(), 1);
        assert_eq!(titles[0].value(), "Test Dataset Title");
    }

    #[test]
    fn test_get_work_titles_missing() {
        let adapter = DataCite2Wikidata::new();
        assert!(adapter.get_work_titles("nonexistent").is_empty());
    }

    #[test]
    fn test_get_work_titles_empty_titles_array() {
        let mut adapter = DataCite2Wikidata::new();
        let mut work = make_datacite_work();
        work["data"]["attributes"]["titles"] = json!([]);
        adapter
            .work_cache
            .insert("10.5281/ZENODO.1234567".to_string(), work);
        assert!(adapter.get_work_titles("10.5281/ZENODO.1234567").is_empty());
    }

    #[test]
    fn test_get_work_type_dataset() {
        let mut adapter = DataCite2Wikidata::new();
        adapter
            .work_cache
            .insert("10.5281/ZENODO.1234567".to_string(), make_datacite_work());
        assert_eq!(
            adapter.get_work_type("10.5281/ZENODO.1234567"),
            Some("Q1172284".to_string())
        );
    }

    #[test]
    fn test_get_work_type_software() {
        let mut adapter = DataCite2Wikidata::new();
        let mut work = make_datacite_work();
        work["data"]["attributes"]["types"]["resourceTypeGeneral"] = json!("Software");
        adapter
            .work_cache
            .insert("10.5281/ZENODO.1234567".to_string(), work);
        assert_eq!(
            adapter.get_work_type("10.5281/ZENODO.1234567"),
            Some("Q7397".to_string())
        );
    }

    #[test]
    fn test_datacite_type_to_q() {
        assert_eq!(DataCite2Wikidata::datacite_type_to_q("Book"), Some("Q571"));
        assert_eq!(
            DataCite2Wikidata::datacite_type_to_q("Dataset"),
            Some("Q1172284")
        );
        assert_eq!(
            DataCite2Wikidata::datacite_type_to_q("JournalArticle"),
            Some("Q13442814")
        );
        assert_eq!(DataCite2Wikidata::datacite_type_to_q("Unknown"), None);
    }

    #[test]
    fn test_get_publication_date_with_issued_date() {
        let mut adapter = DataCite2Wikidata::new();
        adapter
            .work_cache
            .insert("10.5281/ZENODO.1234567".to_string(), make_datacite_work());
        assert_eq!(
            adapter.get_publication_date("10.5281/ZENODO.1234567"),
            Some((2023, Some(3), Some(15)))
        );
    }

    #[test]
    fn test_get_publication_date_year_only() {
        let mut adapter = DataCite2Wikidata::new();
        let mut work = make_datacite_work();
        work["data"]["attributes"]["dates"] = json!([]);
        adapter
            .work_cache
            .insert("10.5281/ZENODO.1234567".to_string(), work);
        assert_eq!(
            adapter.get_publication_date("10.5281/ZENODO.1234567"),
            Some((2023, None, None))
        );
    }

    #[tokio::test]
    async fn test_get_author_list() {
        let mut adapter = DataCite2Wikidata::new();
        adapter
            .work_cache
            .insert("10.5281/ZENODO.1234567".to_string(), make_datacite_work());
        let authors = adapter.get_author_list("10.5281/ZENODO.1234567").await;
        assert_eq!(authors.len(), 2);
        assert_eq!(authors[0].name, Some("Alice Smith".to_string()));
        assert_eq!(authors[0].list_number, Some("1".to_string()));
        assert_eq!(
            authors[0].prop2id.get("P496"),
            Some(&"0000-0001-2345-6789".to_string())
        );
        assert_eq!(authors[1].name, Some("Bob Jones".to_string()));
        assert_eq!(authors[1].list_number, Some("2".to_string()));
        assert!(!authors[1].prop2id.contains_key("P496"));
    }

    #[tokio::test]
    async fn test_get_author_list_name_only() {
        let mut adapter = DataCite2Wikidata::new();
        let mut work = make_datacite_work();
        work["data"]["attributes"]["creators"] = json!([
            {
                "name": "CERN Data Team",
                "nameIdentifiers": []
            }
        ]);
        adapter
            .work_cache
            .insert("10.5281/ZENODO.1234567".to_string(), work);
        let authors = adapter.get_author_list("10.5281/ZENODO.1234567").await;
        assert_eq!(authors.len(), 1);
        assert_eq!(authors[0].name, Some("CERN Data Team".to_string()));
    }

    #[tokio::test]
    async fn test_get_author_list_empty() {
        let mut adapter = DataCite2Wikidata::new();
        let mut work = make_datacite_work();
        work["data"]["attributes"]["creators"] = json!([]);
        adapter
            .work_cache
            .insert("10.5281/ZENODO.1234567".to_string(), work);
        assert!(adapter
            .get_author_list("10.5281/ZENODO.1234567")
            .await
            .is_empty());
    }
}
