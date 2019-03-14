extern crate config;
extern crate mediawiki;
extern crate papers;

//use crossref::Crossref;

fn get_wikidata_item_for_doi(mw_api: &mediawiki::api::Api, doi: &String) -> Option<String> {
    let sparql = format!(
        "SELECT DISTINCT ?q {{ VALUES ?doi {{ '{}' '{}' '{}' }} . ?q wdt:P356 ?doi }}",
        doi,
        doi.to_uppercase(),
        doi.to_lowercase()
    ); // DOIs in Wikidata can be any upper/lowercase :-(
    let res = match mw_api.sparql_query(&sparql) {
        Ok(res) => res,
        _ => return None,
    };
    let qs = mw_api.entities_from_sparql_result(&res, "q");

    match qs.len() {
        0 => None,
        1 => Some(qs[0].clone()),
        _ => {
            println!(
                "Multiple Wikidata items for DOI '{}' : {}",
                &doi,
                qs.join(", ")
            );
            None
        }
    }
}

fn main() {
    /*
        let mut settings = Config::default();
        // File::with_name(..) is shorthand for File::from(Path::new(..))
        settings.merge(File::with_name("test.ini")).unwrap();
        let lgname = settings.get_str("user.user").unwrap();
    */

    /*
        let client = Crossref::builder().build().unwrap();
        let work = client.work("10.1037/0003-066X.59.1.29").unwrap();
        dbg!(work);
    */

    let _ss_client = papers::semanticscholar::Client::new();
    let mw_api = mediawiki::api::Api::new("https://www.wikidata.org/w/api.php").unwrap();
    let mut entities = mediawiki::entity_container::EntityContainer::new();

    let dois = vec!["10.1038/nrn3241"];
    for doi in dois {
        /*
                let work = match ss_client.work(doi) {
                    Ok(work) => work,
                    _ => continue,
                };
        */
        let q = match get_wikidata_item_for_doi(&mw_api, &doi.to_string()) {
            Some(i) => i,
            None => continue,
        };
        if entities.load_entities(&mw_api, &vec![q.clone()]).is_err() {
            continue;
        }
        let item = match entities.get_entity(&q) {
            Some(i) => i,
            None => continue,
        };
        dbg!(item);
    }
}
