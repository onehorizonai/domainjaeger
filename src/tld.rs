//! Which TLDs exist and which of them make sense for a domain hack. All of it
//! is downloaded from IANA and ICANN (and cached for a week), nothing is
//! maintained by hand.

use std::collections::{HashMap, HashSet};

use anyhow::{Context, Result, anyhow, bail};
use serde_json::Value;

use crate::cache::Cache;
use crate::lang::Lang;
use crate::net::{Request, Web};

const TLD_LIST: &str = "https://data.iana.org/TLD/tlds-alpha-by-domain.txt";
const RDAP_BOOTSTRAP: &str = "https://data.iana.org/rdap/dns.json";
const ICANN_GTLDS: &str = "https://www.icann.org/resources/registries/gtlds/v2/gtlds.json";
const TTL_SECS: i64 = 7 * 24 * 3600;

/// The crowd pleasers. They get a turn in every cycle of the hunt. Only TLDs
/// that many Dutch words end in and where most of those names are still
/// free: diminutives (huis.je), kun.st, leni.ng, len.ing, beu.rs, mac.ht,
/// pale.is. Nothing ends in -nl or -dev, and .io, .co and .be turned out to
/// be nearly sold out, so those only come by with the other TLDs.
const SPOTLIGHT: &[&str] = &["ai", "app", "me", "je", "st", "ng", "ing", "rs", "ht", "is"];
/// The same for English words: endings like -ing, -ly, -st and -sh, plus
/// .ai and .app, which English reaches with a small typo (displ.ai, sn.app).
const SPOTLIGHT_ENGLISH: &[&str] = &[
    "ing", "ly", "st", "sh", "me", "in", "se", "re", "it", "ai", "app",
];

/// Latin: nouns on -is, -io and -men, adverbs on -im, and verbs, which a
/// dictionary lists in the first person (canto, libero, dico). Most Latin
/// words end in -us, but .us is for people with a tie to the United States,
/// so it only comes by with the other TLDs. The registries of .es and .as
/// do not answer lookups, and .do gives unclear answers.
const SPOTLIGHT_LATIN: &[&str] = &["is", "io", "men", "im", "la", "ma", "to", "ro", "co"];
/// Spanish: nouns and adjectives on -to, -ro, -io, -la, -so, -co, -ma, -re,
/// -dad (ciud.ad) and -aje (via.je). Thousands of verbs end in -ar, but .ar
/// takes an Argentine tax number, so it is left to the other TLDs as well.
const SPOTLIGHT_SPANISH: &[&str] = &["to", "ro", "io", "la", "so", "co", "ma", "re", "ad", "je"];

/// The TLDs that get a turn in every cycle of a hunt in `lang`.
pub fn spotlight(lang: Lang) -> &'static [&'static str] {
    match lang {
        Lang::Dutch => SPOTLIGHT,
        Lang::English => SPOTLIGHT_ENGLISH,
        Lang::Latin => SPOTLIGHT_LATIN,
        Lang::Spanish => SPOTLIGHT_SPANISH,
    }
}

/// Sponsored TLDs you cannot simply register a name in.
const RESTRICTED: &[&str] = &[
    "gov", "edu", "mil", "int", "arpa", "aero", "coop", "post", "bank",
];
/// Longer TLDs swallow most of a short word, so they are opt-in via --tlds.
const MAX_DEFAULT_LEN: usize = 4;

pub struct Catalog {
    /// Every delegated ASCII TLD.
    existing: HashSet<String>,
    /// .brand TLDs (ICANN "Specification 13"): only the brand owner gets names.
    brands: HashSet<String>,
    /// RDAP base URL per TLD.
    pub bootstrap: HashMap<String, String>,
    pub warnings: Vec<String>,
}

impl Catalog {
    pub async fn load(web: &dyn Web, cache: &Cache) -> Result<Self> {
        let mut warnings = Vec::new();

        let list = fetch(web, cache, "iana:tlds", TLD_LIST)
            .await
            .context("could not download the TLD list from IANA")?;
        let existing: HashSet<String> = list
            .lines()
            .filter(|l| !l.starts_with('#'))
            .map(|l| l.trim().to_ascii_lowercase())
            .filter(|l| !l.is_empty() && l.bytes().all(|b| b.is_ascii_lowercase()))
            .collect();
        if existing.is_empty() {
            bail!("the TLD list from IANA came back empty");
        }

        let bootstrap = match fetch(web, cache, "iana:rdap", RDAP_BOOTSTRAP).await {
            Ok(json) => parse_bootstrap(&json),
            Err(e) => {
                warnings.push(format!(
                    "no RDAP registry from IANA ({e}), relying on discovery"
                ));
                HashMap::new()
            }
        };
        let brands = match fetch(web, cache, "icann:gtlds", ICANN_GTLDS).await {
            Ok(json) => parse_brands(&json),
            Err(e) => {
                warnings.push(format!(
                    "no gTLD details from ICANN ({e}), brand TLDs are not filtered"
                ));
                HashSet::new()
            }
        };
        Ok(Self {
            existing,
            brands,
            bootstrap,
            warnings,
        })
    }

    /// A catalogue of just these extensions, for tests.
    #[cfg(test)]
    pub fn of(tlds: &[&str]) -> Self {
        Self {
            existing: tlds.iter().map(|tld| tld.to_string()).collect(),
            brands: HashSet::new(),
            bootstrap: HashMap::new(),
            warnings: Vec::new(),
        }
    }

    /// The default hunting ground: short, public TLDs.
    pub fn defaults(&self) -> Vec<String> {
        let mut tlds: Vec<String> = self
            .existing
            .iter()
            .filter(|t| (2..=MAX_DEFAULT_LEN).contains(&t.len()))
            .filter(|t| !self.brands.contains(*t) && !RESTRICTED.contains(&t.as_str()))
            .cloned()
            .collect();
        tlds.sort();
        tlds
    }

    /// Validate a user supplied selection such as "ai, .io,app".
    pub fn select(&self, wanted: &str) -> Result<Vec<String>> {
        let mut out = Vec::new();
        for raw in wanted.split([',', ' ']).filter(|s| !s.is_empty()) {
            let tld = raw.trim_start_matches('.').to_ascii_lowercase();
            if !self.existing.contains(&tld) {
                bail!(".{tld} is not a domain extension that exists");
            }
            if self.brands.contains(&tld) {
                bail!(".{tld} belongs to a brand, nobody else can register names there");
            }
            if !out.contains(&tld) {
                out.push(tld);
            }
        }
        if out.is_empty() {
            bail!("no extensions given");
        }
        Ok(out)
    }
}

/// A list that seldom changes: from the cache while it is fresh, or else
/// from the web.
async fn fetch(web: &dyn Web, cache: &Cache, key: &str, url: &str) -> Result<String> {
    if let Some(body) = cache.kv_get(key, TTL_SECS)? {
        return Ok(body);
    }
    let problem = match web.get(Request::get(url)).await {
        Ok(reply) if reply.is_success() => {
            cache.kv_put(key, &reply.body)?;
            return Ok(reply.body);
        }
        Ok(reply) => anyhow!("{url} answered HTTP {}", reply.status),
        Err(reason) => anyhow!("{url}: {reason}"),
    };
    // Rather an old copy than none at all.
    cache.kv_get(key, i64::MAX)?.ok_or(problem)
}

fn parse_bootstrap(json: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    let Ok(body) = serde_json::from_str::<Value>(json) else {
        return map;
    };
    for service in body
        .get("services")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let urls = service.get(1).and_then(Value::as_array);
        // Prefer https when a registry lists several endpoints.
        let Some(url) = urls.and_then(|urls| {
            let urls: Vec<&str> = urls.iter().filter_map(Value::as_str).collect();
            urls.iter()
                .find(|u| u.starts_with("https://"))
                .or(urls.first())
                .copied()
        }) else {
            continue;
        };
        for tld in service
            .get(0)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(tld) = tld.as_str() {
                map.insert(tld.to_ascii_lowercase(), url.to_string());
            }
        }
    }
    map
}

fn parse_brands(json: &str) -> HashSet<String> {
    let Ok(body) = serde_json::from_str::<Value>(json) else {
        return HashSet::new();
    };
    body.get("gTLDs")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|g| g.get("specification13").and_then(Value::as_bool) == Some(true))
        .filter_map(|g| g.get("gTLD").and_then(Value::as_str))
        .map(str::to_ascii_lowercase)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use crate::net::fake;

    const LIST: &str =
        "# Version 2026101200\nAI\nAPP\nBANK\nCOM\nGOOGLE\nING\nMUSEUM\nNL\nXN--P1AI\n";
    const BOOTSTRAP: &str = r#"{"services":[[["nl"],["https://rdap.sidn.nl/"]]]}"#;
    const GTLDS: &str = r#"{"gTLDs":[{"gTLD":"google","specification13":true},{"gTLD":"app","specification13":false}]}"#;

    fn world() -> (fake::Web, Cache, Arc<fake::Clock>) {
        let clock = Arc::new(fake::Clock::at(0));
        (fake::Web::new(), Cache::in_memory(clock.clone()), clock)
    }

    #[tokio::test]
    async fn the_catalogue_knows_what_exists_and_what_is_open() {
        let (web, cache, _) = world();
        web.on("tlds-alpha-by-domain", 200, LIST)
            .on("rdap/dns.json", 200, BOOTSTRAP)
            .on("gtlds.json", 200, GTLDS);
        let catalog = Catalog::load(&web, &cache).await.unwrap();
        assert!(catalog.warnings.is_empty());
        assert_eq!(catalog.bootstrap["nl"], "https://rdap.sidn.nl/");
        // Short and open to everyone: no brands, no .bank, nothing long,
        // nothing that is not plain letters.
        assert_eq!(catalog.defaults(), vec!["ai", "app", "com", "ing", "nl"]);

        // People type extensions in every way.
        assert_eq!(
            catalog.select("ai, .IO,app").unwrap_err().to_string(),
            ".io is not a domain extension that exists"
        );
        assert_eq!(catalog.select(".AI,app  ai").unwrap(), vec!["ai", "app"]);
        // A long one is there for the asking.
        assert_eq!(catalog.select("museum").unwrap(), vec!["museum"]);
        let brand = catalog.select("google").unwrap_err().to_string();
        assert!(brand.contains("belongs to a brand"), "{brand}");
        assert_eq!(
            catalog.select(" , ").unwrap_err().to_string(),
            "no extensions given"
        );
    }

    #[tokio::test]
    async fn the_lists_are_kept_for_a_week_and_an_old_copy_beats_none() {
        let (web, cache, clock) = world();
        web.on("tlds-alpha-by-domain", 200, "AI\nNL\n")
            .on("rdap/dns.json", 200, BOOTSTRAP)
            .on("gtlds.json", 200, GTLDS);
        Catalog::load(&web, &cache).await.unwrap();
        assert_eq!(web.asked().len(), 3);
        // The next start asks nobody.
        Catalog::load(&web, &cache).await.unwrap();
        assert_eq!(web.asked().len(), 3);

        // A week on the lists are due, but IANA is down: the old copy serves.
        clock.advance(TTL_SECS);
        let offline = fake::Web::new();
        offline.down("iana.org").down("icann.org");
        let catalog = Catalog::load(&offline, &cache).await.unwrap();
        assert_eq!(offline.asked().len(), 3);
        assert_eq!(catalog.defaults(), vec!["ai", "nl"]);
        assert!(catalog.warnings.is_empty());
    }

    #[tokio::test]
    async fn without_the_list_of_extensions_there_is_no_hunt() {
        let (web, cache, _) = world();
        web.down("iana.org").down("icann.org");
        let problem = Catalog::load(&web, &cache).await.err().unwrap();
        assert!(
            format!("{problem:#}").contains("could not download the TLD list"),
            "{problem:#}"
        );

        // An empty list is no list.
        let (web, cache, _) = world();
        web.on("tlds-alpha-by-domain", 200, "# nothing today\n");
        let problem = Catalog::load(&web, &cache).await.err().unwrap();
        assert!(problem.to_string().contains("came back empty"));
    }

    #[tokio::test]
    async fn the_other_lists_are_only_missed_with_a_warning() {
        let (web, cache, _) = world();
        web.on("tlds-alpha-by-domain", 200, LIST)
            .on("rdap/dns.json", 503, "")
            .down("icann.org");
        let catalog = Catalog::load(&web, &cache).await.unwrap();
        assert_eq!(catalog.warnings.len(), 2);
        assert!(catalog.warnings[0].contains("relying on discovery"));
        assert!(catalog.warnings[1].contains("brand TLDs are not filtered"));
        assert!(catalog.bootstrap.is_empty());
        // Without the brands list a brand looks like any other extension.
        assert!(catalog.select("google").is_ok());
    }

    #[test]
    fn every_language_has_extensions_in_the_spotlight() {
        for lang in Lang::ALL {
            let stars = spotlight(lang);
            assert!(stars.len() >= 9, "{lang:?}");
            assert!(
                stars
                    .iter()
                    .all(|tld| (2..=MAX_DEFAULT_LEN).contains(&tld.len()))
            );
            assert!(stars.iter().all(|tld| !RESTRICTED.contains(tld)));
        }
        assert!(spotlight(Lang::English).contains(&"ing"));
        // Unreadable lists are empty lists.
        assert!(parse_bootstrap("<html>").is_empty());
        assert!(parse_brands("").is_empty());
    }

    #[test]
    fn bootstrap_maps_tlds_to_servers() {
        let map = parse_bootstrap(
            r#"{"services":[[["nl"],["https://rdap.sidn.nl/"]],[["app","dev"],["http://x/","https://pubapi.registry.google/rdap/"]]]}"#,
        );
        assert_eq!(map["nl"], "https://rdap.sidn.nl/");
        assert_eq!(map["dev"], "https://pubapi.registry.google/rdap/");
    }

    #[test]
    fn brands_are_recognised() {
        let brands = parse_brands(
            r#"{"gTLDs":[{"gTLD":"man","specification13":true},{"gTLD":"app","specification13":false}]}"#,
        );
        assert!(brands.contains("man") && !brands.contains("app"));
    }
}
