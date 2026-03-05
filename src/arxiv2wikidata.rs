use crate::generic_author_info::GenericAuthorInfo;
use crate::scientific_publication_adapter::ScientificPublicationAdapter;
use crate::*;
use async_trait::async_trait;
use std::collections::HashMap;

use self::identifiers::{GenericWorkIdentifier, GenericWorkType, IdProp};

pub struct Arxiv2Wikidata {
    author_cache: HashMap<String, String>,
    work_cache: HashMap<String, arxiv::Arxiv>,
}

impl Default for Arxiv2Wikidata {
    fn default() -> Self {
        Self::new()
    }
}

impl Arxiv2Wikidata {
    pub fn new() -> Self {
        Arxiv2Wikidata {
            author_cache: HashMap::new(),
            work_cache: HashMap::new(),
        }
    }

    pub fn get_cached_publication_from_id(&self, publication_id: &str) -> Option<&arxiv::Arxiv> {
        self.work_cache.get(publication_id)
    }

    /// Extracts the short arXiv ID from the full URL returned by the API.
    /// E.g. "http://arxiv.org/abs/2301.12345v1" -> "2301.12345"
    ///      "http://arxiv.org/abs/hep-th/9901001v1" -> "hep-th/9901001"
    fn extract_arxiv_id(id_url: &str) -> String {
        // Strip the base URL prefix (e.g. "http://arxiv.org/abs/")
        let id = if let Some(pos) = id_url.find("/abs/") {
            &id_url[pos + 5..]
        } else {
            id_url
        };
        // Strip version suffix (e.g. "v1", "v2")
        match id.rfind('v') {
            Some(pos)
                if pos > 0
                    && id[pos + 1..].chars().all(|c| c.is_ascii_digit())
                    && !id[pos + 1..].is_empty() =>
            {
                id[..pos].to_string()
            }
            _ => id.to_string(),
        }
    }

}

#[async_trait(?Send)]
impl ScientificPublicationAdapter for Arxiv2Wikidata {
    fn name(&self) -> &str {
        "Arxiv2Wikidata"
    }

    fn publication_property(&self) -> Option<IdProp> {
        Some(IdProp::ARXIV)
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
        let arxiv_ids: Vec<&str> = ids
            .iter()
            .filter_map(|id| {
                if let GenericWorkType::Property(prop) = id.work_type() {
                    if *prop == IdProp::ARXIV {
                        return Some(id.id());
                    }
                }
                None
            })
            .collect();
        let futures: Vec<_> = arxiv_ids
            .iter()
            .map(|arxiv_id| {
                let query = arxiv::ArxivQueryBuilder::new()
                    .id_list(arxiv_id)
                    .max_results(1)
                    .build();
                arxiv::fetch_arxivs(query)
            })
            .collect();
        let results = futures::future::join_all(futures).await;

        let mut ret = vec![];
        results
            .into_iter()
            .flatten()
            .filter_map(|result| result.into_iter().next())
            .for_each(|arxiv| {
                let arxiv_id = Self::extract_arxiv_id(&arxiv.id);
                ret.push(GenericWorkIdentifier::new_prop(IdProp::ARXIV, &arxiv_id));
                self.work_cache.insert(arxiv_id, arxiv);
            });
        ret
    }

    async fn publication_id_from_item(&mut self, item: &Entity) -> Option<String> {
        let arxiv_id = self.get_external_identifier_from_item(item, &IdProp::ARXIV)?;
        self.do_cache_work(&arxiv_id).await
    }

    async fn do_cache_work(&mut self, publication_id: &str) -> Option<String> {
        if self.work_cache.contains_key(publication_id) {
            return Some(publication_id.to_string());
        }
        let query = arxiv::ArxivQueryBuilder::new()
            .id_list(publication_id)
            .max_results(1)
            .build();
        let results = arxiv::fetch_arxivs(query).await.ok()?;
        let arxiv = results.into_iter().next()?;
        let arxiv_id = Self::extract_arxiv_id(&arxiv.id);
        self.work_cache.insert(arxiv_id.clone(), arxiv);
        Some(arxiv_id)
    }

    fn get_work_titles(&self, publication_id: &str) -> Vec<LocaleString> {
        match self.get_cached_publication_from_id(publication_id) {
            Some(arxiv) if !arxiv.title.is_empty() => {
                vec![LocaleString::new("en", &arxiv.title)]
            }
            _ => vec![],
        }
    }

    fn get_publication_date(&self, publication_id: &str) -> Option<(u32, Option<u8>, Option<u8>)> {
        let arxiv = self.get_cached_publication_from_id(publication_id)?;
        crate::scientific_publication_adapter::parse_date(&arxiv.published)
    }

    async fn get_author_list(&mut self, publication_id: &str) -> Vec<GenericAuthorInfo> {
        match self.get_cached_publication_from_id(publication_id) {
            Some(arxiv) => arxiv
                .authors
                .iter()
                .enumerate()
                .map(|(num, name)| GenericAuthorInfo::new_from_name_num(name, num + 1))
                .collect(),
            None => vec![],
        }
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

    fn make_arxiv(id: &str, title: &str, authors: Vec<&str>, published: &str) -> arxiv::Arxiv {
        arxiv::Arxiv {
            id: format!("http://arxiv.org/abs/{}v1", id),
            title: title.to_string(),
            authors: authors.into_iter().map(|s| s.to_string()).collect(),
            published: published.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn test_extract_arxiv_id_with_version() {
        assert_eq!(
            Arxiv2Wikidata::extract_arxiv_id("http://arxiv.org/abs/2301.12345v1"),
            "2301.12345"
        );
        assert_eq!(
            Arxiv2Wikidata::extract_arxiv_id("http://arxiv.org/abs/2301.12345v23"),
            "2301.12345"
        );
    }

    #[test]
    fn test_extract_arxiv_id_without_version() {
        assert_eq!(
            Arxiv2Wikidata::extract_arxiv_id("http://arxiv.org/abs/2301.12345"),
            "2301.12345"
        );
    }

    #[test]
    fn test_extract_arxiv_id_old_format() {
        assert_eq!(
            Arxiv2Wikidata::extract_arxiv_id("http://arxiv.org/abs/hep-th/9901001v1"),
            "hep-th/9901001"
        );
    }

    #[test]
    fn test_parse_date_full() {
        assert_eq!(
            crate::scientific_publication_adapter::parse_date("2023-01-15T00:00:00Z"),
            Some((2023, Some(1), Some(15)))
        );
    }

    #[test]
    fn test_parse_date_partial() {
        assert_eq!(
            crate::scientific_publication_adapter::parse_date("2023-06"),
            Some((2023, Some(6), None))
        );
        assert_eq!(crate::scientific_publication_adapter::parse_date("2023"), Some((2023, None, None)));
    }

    #[test]
    fn test_parse_date_invalid() {
        assert_eq!(crate::scientific_publication_adapter::parse_date(""), None);
        assert_eq!(crate::scientific_publication_adapter::parse_date("not-a-date"), None);
    }

    #[test]
    fn test_get_work_titles() {
        let mut adapter = Arxiv2Wikidata::new();
        adapter.work_cache.insert(
            "2301.12345".to_string(),
            make_arxiv(
                "2301.12345",
                "Test Paper Title",
                vec![],
                "2023-01-15T00:00:00Z",
            ),
        );
        let titles = adapter.get_work_titles("2301.12345");
        assert_eq!(titles.len(), 1);
        assert_eq!(titles[0].value(), "Test Paper Title");
        assert_eq!(titles[0].language(), "en");
    }

    #[test]
    fn test_get_work_titles_empty() {
        let mut adapter = Arxiv2Wikidata::new();
        adapter.work_cache.insert(
            "2301.12345".to_string(),
            make_arxiv("2301.12345", "", vec![], "2023-01-15T00:00:00Z"),
        );
        assert!(adapter.get_work_titles("2301.12345").is_empty());
    }

    #[test]
    fn test_get_work_titles_missing() {
        let adapter = Arxiv2Wikidata::new();
        assert!(adapter.get_work_titles("nonexistent").is_empty());
    }

    #[test]
    fn test_get_publication_date() {
        let mut adapter = Arxiv2Wikidata::new();
        adapter.work_cache.insert(
            "2301.12345".to_string(),
            make_arxiv("2301.12345", "Title", vec![], "2023-01-15T00:00:00Z"),
        );
        assert_eq!(
            adapter.get_publication_date("2301.12345"),
            Some((2023, Some(1), Some(15)))
        );
    }

    #[tokio::test]
    async fn test_get_author_list() {
        let mut adapter = Arxiv2Wikidata::new();
        adapter.work_cache.insert(
            "2301.12345".to_string(),
            make_arxiv(
                "2301.12345",
                "Title",
                vec!["Alice Smith", "Bob Jones"],
                "2023-01-15T00:00:00Z",
            ),
        );
        let authors = adapter.get_author_list("2301.12345").await;
        assert_eq!(authors.len(), 2);
        assert_eq!(authors[0].name(), Some("Alice Smith"));
        assert_eq!(authors[0].list_number(), Some("1"));
        assert_eq!(authors[1].name(), Some("Bob Jones"));
        assert_eq!(authors[1].list_number(), Some("2"));
    }

    #[tokio::test]
    async fn test_get_author_list_empty() {
        let mut adapter = Arxiv2Wikidata::new();
        adapter.work_cache.insert(
            "2301.12345".to_string(),
            make_arxiv("2301.12345", "Title", vec![], "2023-01-15T00:00:00Z"),
        );
        assert!(adapter.get_author_list("2301.12345").await.is_empty());
    }

    #[test]
    fn test_publication_property() {
        let adapter = Arxiv2Wikidata::new();
        assert_eq!(adapter.publication_property(), Some(IdProp::ARXIV));
    }

    #[test]
    fn test_name() {
        let adapter = Arxiv2Wikidata::new();
        assert_eq!(adapter.name(), "Arxiv2Wikidata");
    }
}
