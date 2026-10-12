//! What tests share: a small world to hunt in. The web and WHOIS answer
//! from a script, the clock stands still and the cache lives in memory, so
//! a whole round runs in a test without touching the network.

use std::sync::Arc;

use serde_json::{Value, json};

use crate::cache::Cache;
use crate::check::{Checker, Method};
use crate::hacks::Affix;
use crate::hunt::{Hunter, Settings};
use crate::lang::Lang;
use crate::net::fake;
use crate::wiktionary::Wiktionary;

/// The syllable counts as the English Wiktionary files them.
pub const ONE: &str = "Category:English 1-syllable words";
pub const TWO: &str = "Category:English 2-syllable words";
pub const THREE: &str = "Category:English 3-syllable words";

const NO_SUCH_NAME: &str = r#"{"Status":3}"#;

/// A page of a dictionary answer: a title, its place in a search and the
/// categories it is in.
pub fn page(title: &str, rank: u64, categories: &[&str]) -> Value {
    let categories: Vec<Value> = categories.iter().map(|c| json!({ "title": c })).collect();
    json!({ "title": title, "index": rank, "categories": categories })
}

/// A complete answer of a dictionary with these pages.
pub fn answer(pages: Vec<Value>) -> String {
    json!({ "batchcomplete": true, "query": { "pages": pages } }).to_string()
}

/// An answer with English words of two syllables, best known first.
pub fn words(words: &[&str]) -> String {
    let pages = words.iter().enumerate();
    answer(
        pages
            .map(|(i, word)| page(word, i as u64 + 1, &[TWO]))
            .collect(),
    )
}

pub struct World {
    pub web: Arc<fake::Web>,
    pub whois: Arc<fake::Whois>,
    pub clock: Arc<fake::Clock>,
    pub cache: Arc<Cache>,
}

impl World {
    pub fn new() -> Self {
        let clock = Arc::new(fake::Clock::at(1_700_000_000));
        Self {
            web: Arc::new(fake::Web::new()),
            whois: Arc::new(fake::Whois::new()),
            cache: Arc::new(Cache::in_memory(clock.clone())),
            clock,
        }
    }

    /// The registry of an extension is known and answers: every name is
    /// free there, except the ones that are `taken`.
    pub fn registry(&self, tld: &str) -> &Self {
        let method = Method::Rdap {
            base: format!("https://rdap.test/{tld}/"),
        };
        let json = serde_json::to_string(&method).unwrap();
        self.cache.kv_put(&format!("method:{tld}"), &json).unwrap();
        self
    }

    /// These names are registered.
    pub fn taken(&self, domains: &[&str]) -> &Self {
        for domain in domains {
            let (_, tld) = domain.split_once('.').unwrap();
            let address = format!("rdap.test/{tld}/domain/{domain}");
            self.web
                .on(&address, 200, r#"{"objectClassName":"domain"}"#);
        }
        self
    }

    /// Searches of the dictionary with `part` in them find these English
    /// words of two syllables.
    pub fn finds(&self, part: &str, found: &[&str]) -> &Self {
        self.web.on(part, 200, &words(found));
        self
    }

    /// Everything that has no answer yet gets a plain one: the dictionaries
    /// know nothing more, and no name has name servers. Call it last, the
    /// first rule that fits answers.
    pub fn otherwise_quiet(&self) -> &Self {
        self.web
            .on("wiktionary.org/w/api.php", 200, &answer(Vec::new()))
            .on("dns.google/resolve", 200, NO_SUCH_NAME)
            .on("cloudflare-dns.com", 200, NO_SUCH_NAME);
        self
    }

    pub fn wiki(&self, syllables: &[u8]) -> Arc<Wiktionary> {
        Arc::new(Wiktionary::new(self.web.clone(), syllables, None))
    }

    pub fn checker(&self) -> Arc<Checker> {
        Arc::new(Checker::new(
            self.web.clone(),
            self.whois.clone(),
            self.cache.clone(),
            Default::default(),
        ))
    }

    /// A hunt in these extensions, picked by hand, for words of one to
    /// three syllables.
    pub fn hunter(&self, tlds: &[&str], settings: Settings) -> Hunter {
        Hunter::new(
            self.wiki(&[1, 2, 3]),
            self.checker(),
            self.cache.clone(),
            tlds.iter().map(|tld| tld.to_string()).collect(),
            true,
            settings,
        )
    }

    /// How often a registry was asked about names with `part` in them.
    pub fn lookups(&self, part: &str) -> usize {
        let asked = self.web.asked();
        let at_registry = asked.iter().filter(|a| a.contains("rdap.test"));
        at_registry.filter(|a| a.contains(part)).count()
    }
}

/// The settings of a hunt as it starts, without typos: one thing at a time.
pub fn settings() -> Settings {
    Settings {
        count: 10,
        max_checks: 150,
        min_label: 2,
        typos: false,
        category: None,
        lang: Lang::English,
        whole: false,
        rude: true,
        affix: Affix::default(),
    }
}
