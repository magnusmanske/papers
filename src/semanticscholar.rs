/// Implements an author
#[derive(Debug, Clone)]
pub struct Author {
    pub author_id: Option<String>,
    pub name: Option<String>,
    pub url: Option<String>,
}

impl Author {
    pub fn new_from_json(j: &serde_json::Value) -> Result<Author, Box<::std::error::Error>> {
        if !j.is_object() {
            return Err(From::from(format!(
                "JSON for Author::new_from_json is not an object: {}",
                &j
            )));
        }
        Ok(Author {
            author_id: j["authorId"].as_str().map_or(None, |s| Some(s.to_string())),
            name: j["name"].as_str().map_or(None, |s| Some(s.to_string())),
            url: j["url"].as_str().map_or(None, |s| Some(s.to_string())),
        })
    }
}

/// Implements a topic
#[derive(Debug, Clone)]
pub struct Topic {
    pub topic: Option<String>,
    pub topic_id: Option<String>,
    pub url: Option<String>,
}

impl Topic {
    pub fn new_from_json(j: &serde_json::Value) -> Result<Topic, Box<::std::error::Error>> {
        if !j.is_object() {
            return Err(From::from(format!(
                "JSON for Topic::new_from_json is not an object: {}",
                &j
            )));
        }
        Ok(Topic {
            topic_id: j["topicId"].as_str().map_or(None, |s| Some(s.to_string())),
            topic: j["topic"].as_str().map_or(None, |s| Some(s.to_string())),
            url: j["url"].as_str().map_or(None, |s| Some(s.to_string())),
        })
    }
}

/// Implements a work (=paper)
#[derive(Debug, Clone)]
pub struct Work {
    pub arxiv_id: Option<String>,
    pub authors: Vec<Author>,
    pub citation_velocity: Option<u64>,
    pub citations: Vec<Work>,
    pub doi: Option<String>,
    pub influential_citation_count: Option<u64>,
    pub paper_id: Option<String>,
    pub references: Vec<Work>,
    pub title: Option<String>,
    pub topics: Vec<Topic>,
    pub url: Option<String>,
    pub venue: Option<String>,
    pub year: Option<u64>,
}

impl Work {
    pub fn new_from_json(j: &serde_json::Value) -> Result<Work, Box<::std::error::Error>> {
        if !j.is_object() {
            return Err(From::from(format!(
                "JSON for Work::new_from_json is not an object: {}",
                &j
            )));
        }
        let mut ret = Work {
            arxiv_id: j["arxivId"].as_str().map_or(None, |s| Some(s.to_string())),
            authors: vec![],
            citation_velocity: j["citationVelocity"].as_u64(),
            citations: vec![],
            doi: j["doi"].as_str().map_or(None, |s| Some(s.to_string())),
            influential_citation_count: j["influentialCitationCount"].as_u64(),
            paper_id: j["paperId"].as_str().map_or(None, |s| Some(s.to_string())),
            references: vec![],
            title: j["title"].as_str().map_or(None, |s| Some(s.to_string())),
            topics: vec![],
            url: j["url"].as_str().map_or(None, |s| Some(s.to_string())),
            venue: j["venue"].as_str().map_or(None, |s| Some(s.to_string())),
            year: j["year"].as_u64(),
        };
        for author in j["authors"].as_array().unwrap_or(&vec![]) {
            ret.authors.push(Author::new_from_json(author)?);
        }
        for topic in j["topics"].as_array().unwrap_or(&vec![]) {
            ret.topics.push(Topic::new_from_json(topic)?);
        }
        if let Some(citations) = j["citations"].as_array() {
            for paper in citations {
                if let Ok(paper) = Work::new_from_json(paper) {
                    ret.citations.push(paper);
                }
            }
        }
        if let Some(references) = j["references"].as_array() {
            for paper in references {
                ret.references.push(Work::new_from_json(paper)?);
            }
        }
        Ok(ret)
    }
}

#[derive(Debug, Clone)]
pub struct Client {}

impl Client {
    pub fn new() -> Client {
        Client {}
    }

    pub fn work(&self, id: &str) -> Result<Work, Box<::std::error::Error>> {
        let api_url = String::from("http://api.semanticscholar.org/v1");
        let url = api_url + "/paper/" + id;
        let json: serde_json::Value = reqwest::get(url.as_str())?.json()?;
        match json["error"].as_str() {
            Some(error_string) => Err(From::from(format!("{}:{}", error_string, id))),
            None => Work::new_from_json(&json),
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn it_works() {
        assert_eq!(2 + 2, 4);
    }
}
