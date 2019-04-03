extern crate config;
extern crate mediawiki;
extern crate papers;
//#[macro_use]
extern crate lazy_static;
extern crate regex;
//#[macro_use]
extern crate serde_json;

use config::{Config, File};
//use papers::pubmed2wikidata::Crossref2Wikidata;
use papers::crossref2wikidata::Crossref2Wikidata;
use papers::orcid2wikidata::Orcid2Wikidata;
use papers::semanticscholar2wikidata::Semanticscholar2Wikidata;
use papers::wikidata_papers::WikidataPapers;

fn main() {
    let mut mw_api = mediawiki::api::Api::new("https://www.wikidata.org/w/api.php").unwrap();

    let mut settings = Config::default();
    // File::with_name(..) is shorthand for File::from(Path::new(..))
    settings.merge(File::with_name("test.ini")).unwrap();
    let lgname = settings.get_str("user.user").unwrap();
    let lgpass = settings.get_str("user.pass").unwrap();
    mw_api.login(lgname, lgpass).unwrap();

    let mut wdp = WikidataPapers::new();
    wdp.add_adapter(Box::new(Semanticscholar2Wikidata::new()));
    wdp.add_adapter(Box::new(Crossref2Wikidata::new()));
    wdp.add_adapter(Box::new(Orcid2Wikidata::new()));
    wdp.update_dois(
        &mut mw_api,
        &vec!["10.1016/j.bpj.2008.12.3951"], //"10.1038/nrn3241"
    );
}
