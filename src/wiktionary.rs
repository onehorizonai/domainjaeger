//! The word source: WikiWoordenboek (nl.wiktionary.org), queried live.
//!
//! Nothing is downloaded up front. WikiWoordenboek keeps a *retrograde*
//! index of all Dutch words (sorted by their reversed spelling), so "all words
//! ending in -ai" is a single range query. The same request returns the
//! dictionary's own syllable count, how well known the word is, whether it is
//! in the official Woordenlijst Nederlandse Taal and what it is about.
//!
//! Words are also filed by topic ("Financieel", "Informatica"), which is how
//! a hunt can be limited to a category: see `topics`.
//!
//! English words come from the English Wiktionary. It has no retrograde
//! index, but its search can match the end of a title, and it lists the best
//! known words first. It knows syllable counts and topics too, but not how
//! well known a word is. It describes the words of other languages the same
//! way, which is where Latin and Spanish words come from.
//!
//! It also says what a word means, whatever its language (see `meaning`),
//! and it lists the first names of every language, Dutch ones included.
//! Both dictionaries label vulgar words and the like: see `tone`.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::time::Instant;

use crate::hacks::is_plain;
use crate::lang::{Lang, Wiki};
use crate::net::{Request, Web};
use crate::{tone, topics};

/// Where the project lives: part of how it introduces itself to a server.
pub const HOMEPAGE: &str = env!("CARGO_PKG_REPOSITORY");
const RETROGRADE: &str = "Categorie:Retrograad van het Nederlands";
const OFFICIAL: &str = "Categorie:Woordenlijst Nederlandse Taal";
/// Its subcategories are the topics Dutch words are filed under.
const TOPICS: &str = "Categorie:Onderwerpen in het Nederlands";
/// Words per slice of a topic: small enough for the first names to show up
/// after a request or two.
const TOPIC_SLICE: usize = 200;
/// Words per search for an ending in the English Wiktionary. Their
/// categories fit in one request, as there is little to ask about a word.
const SEARCH_SLICE: usize = 300;
/// The English Wiktionary says what a word means in a request per word.
const DEFINITIONS: &str = "https://en.wiktionary.org/api/rest_v1/page/definition";
/// A meaning is a few words, not the whole entry.
const MEANING_LENGTH: usize = 60;
/// A meaning is nice to have: not worth as many attempts as a list of words.
const MEANING_ATTEMPTS: u64 = 3;
/// Requests a slice of a topic may take (its categories can spill over).
pub const SLICE_REQUESTS: usize = 3;
/// The API accepts at most 50 category filters per request.
const MAX_CATEGORY_FILTERS: usize = 50;
/// Be a polite API client: one request at a time with a little air in between.
const MIN_GAP: Duration = Duration::from_millis(150);
/// Wikimedia allows 200 requests a minute to clients that say who they are in
/// the User-Agent and 10 to everybody else. We stay a little below both.
const IDENTIFIED_PER_MINUTE: usize = 170;
const ANONYMOUS_PER_MINUTE: usize = 9;
const MINUTE: Duration = Duration::from_secs(60);
const ATTEMPTS: u64 = 6;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Word {
    pub text: String,
    pub syllables: Option<u8>,
    pub prevalence: Option<u8>,
    pub official: bool,
    /// The topics it is filed under, out of the ones we ask about.
    #[serde(default)]
    pub topics: Vec<String>,
    /// Why it may not be for everyone ("vulgar", "sexual"), see `tone`.
    #[serde(default)]
    pub tone: Vec<String>,
    /// A first name: `text` is in lower case all the same, as that is what
    /// goes into a domain.
    #[serde(default)]
    pub name: bool,
}

impl Word {
    /// "Economie, Financieel", the way topics are stored with a domain.
    pub fn topic_list(&self) -> String {
        self.topics.join(", ")
    }

    /// "vulgar, sexual", or nothing for most words.
    pub fn tone_list(&self) -> String {
        let flags: Vec<&str> = self.tone.iter().map(String::as_str).collect();
        tone::list(&flags)
    }

    /// The way the dictionary spells it: "Mike" for a name, "mike" for the
    /// microphone.
    pub fn shown(&self) -> String {
        let mut letters = self.text.chars();
        match letters.next() {
            Some(first) if self.name => first.to_uppercase().chain(letters).collect(),
            _ => self.text.clone(),
        }
    }
}

/// A list of words that can be read slice by slice, whatever they end in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WordList {
    /// The words filed under a topic.
    About(String),
    /// The Dutch words that this percentage of the Dutch know.
    KnownBy(u8),
    /// The words of the English Wiktionary, the best known first.
    Popular,
    /// The first names of a language.
    Names,
}

impl WordList {
    /// For the status line: "asking the dictionary for ...".
    pub fn describe(&self) -> String {
        match self {
            WordList::About(topic) => format!("words about {}", topic.to_lowercase()),
            WordList::KnownBy(share) => format!("words that {share}% of the Dutch know"),
            WordList::Popular => "its best known words".to_string(),
            WordList::Names => "first names".to_string(),
        }
    }
}

/// One slice of the retrograde index, or of a list of words.
#[derive(Debug)]
pub struct Page {
    /// Words that passed the syllable filter.
    pub words: Vec<Word>,
    /// Opaque position of the next slice, `None` when the ending is exhausted.
    pub next: Option<String>,
}

pub struct Wiktionary {
    web: Arc<dyn Web>,
    /// One request at a time.
    gate: tokio::sync::Mutex<()>,
    /// When the requests of the last minute were sent.
    recent: Mutex<VecDeque<Instant>>,
    /// E-mail address or URL of whoever runs this, sent in the User-Agent.
    contact: RwLock<Option<String>>,
    /// The categories we ask about for every word of a language.
    filters: HashMap<Lang, String>,
    /// And for every first name, which the English Wiktionary is asked
    /// about whatever the language.
    name_filters: HashMap<Lang, String>,
    syllables: Vec<u8>,
}

/// What a list is made of.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Words,
    Names,
}

/// A page as the API returned it.
#[derive(Default)]
struct Found {
    categories: Vec<String>,
    /// Its place in the search results, when it came from a search.
    rank: Option<u64>,
}

/// Wikimedia's User-Agent policy asks for an e-mail address or a URL.
pub fn valid_contact(contact: &str) -> bool {
    let contact = contact.trim();
    !contact.contains(char::is_whitespace)
        && (contact.starts_with("https://")
            || contact.starts_with("http://")
            || contact
                .split_once('@')
                .is_some_and(|(name, host)| !name.is_empty() && host.contains('.')))
}

impl Wiktionary {
    pub fn new(web: Arc<dyn Web>, syllables: &[u8], contact: Option<String>) -> Self {
        let flags = |wiki: Wiki, lang: Lang| {
            tone::categories(wiki, lang)
                .into_iter()
                .map(|(category, _)| category)
        };
        let filter = |lang: Lang| {
            let wiki = lang.wiki();
            let mut categories: Vec<String> = syllables
                .iter()
                .map(|n| syllable_category(wiki, lang, *n))
                .collect();
            let tracked = topics::tracked(lang);
            categories.extend(tracked.into_iter().map(|t| topics::title(lang, t)));
            categories.extend(flags(wiki, lang));
            if wiki == Wiki::WikiWoordenboek {
                categories.push(OFFICIAL.to_string());
                // Spend the remaining filter slots on the best known words.
                let spare = MAX_CATEGORY_FILTERS.saturating_sub(categories.len());
                categories.extend(
                    (0..=100u8)
                        .rev()
                        .take(spare)
                        .map(|p| format!("Categorie:Prevalentie Nederland {p} %")),
                );
            }
            categories.join("|")
        };
        // Every syllable count is asked about: a name that is known to be
        // too long is left out, one without a count is not.
        let name_filter = |lang: Lang| {
            (1..=9)
                .map(|n| syllable_category(Wiki::Wiktionary, lang, n))
                .chain(flags(Wiki::Wiktionary, lang))
                .collect::<Vec<_>>()
                .join("|")
        };
        Self {
            web,
            gate: tokio::sync::Mutex::new(()),
            recent: Mutex::new(VecDeque::new()),
            contact: RwLock::new(contact),
            filters: Lang::ALL.into_iter().map(|l| (l, filter(l))).collect(),
            name_filters: Lang::ALL.into_iter().map(|l| (l, name_filter(l))).collect(),
            syllables: syllables.to_vec(),
        }
    }

    /// The same dictionary, asked about other syllable counts. It keeps
    /// count of the requests already made this minute.
    pub fn with_syllables(&self, syllables: &[u8]) -> Self {
        let fresh = Self::new(self.web.clone(), syllables, self.contact());
        let recent = self.recent.lock().unwrap_or_else(|e| e.into_inner());
        *fresh.recent.lock().unwrap_or_else(|e| e.into_inner()) = recent.clone();
        fresh
    }

    fn filter(&self, lang: Lang) -> &str {
        &self.filters[&lang]
    }

    /// The syllable counts words are limited to.
    pub fn syllables(&self) -> &[u8] {
        &self.syllables
    }

    pub fn set_contact(&self, contact: Option<String>) {
        *self.contact.write().unwrap_or_else(|e| e.into_inner()) = contact;
    }

    pub fn contact(&self) -> Option<String> {
        self.contact
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    fn requests_per_minute(&self) -> usize {
        if self.contact().is_some() {
            IDENTIFIED_PER_MINUTE
        } else {
            ANONYMOUS_PER_MINUTE
        }
    }

    /// The limit Wikimedia publishes for a client like us.
    pub fn allowed_per_minute(&self) -> usize {
        if self.contact().is_some() { 200 } else { 10 }
    }

    /// How long until the next request fits in the rate limit.
    pub fn wait_time(&self) -> Duration {
        let mut recent = self.recent.lock().unwrap_or_else(|e| e.into_inner());
        while recent.front().is_some_and(|sent| sent.elapsed() >= MINUTE) {
            recent.pop_front();
        }
        match recent.len().checked_sub(self.requests_per_minute()) {
            // The oldest request that still counts has to leave the window first.
            Some(over) => MINUTE.saturating_sub(recent[over].elapsed()),
            None => recent.back().map_or(Duration::ZERO, |last| {
                MIN_GAP.saturating_sub(last.elapsed())
            }),
        }
    }

    /// Requests that still fit in the current minute.
    pub fn spare_requests(&self) -> usize {
        let mut recent = self.recent.lock().unwrap_or_else(|e| e.into_inner());
        while recent.front().is_some_and(|sent| sent.elapsed() >= MINUTE) {
            recent.pop_front();
        }
        self.requests_per_minute().saturating_sub(recent.len())
    }

    fn user_agent(&self) -> String {
        let version = env!("CARGO_PKG_VERSION");
        // Wikimedia asks every client to say who it is and where to reach
        // someone about it: the project, and whoever runs this copy.
        match self.contact() {
            Some(contact) => format!("domainjaeger/{version} (+{HOMEPAGE}; {contact})"),
            None => format!("domainjaeger/{version} (+{HOMEPAGE})"),
        }
    }

    /// Words ending in `suffix`, limited to the configured syllable counts.
    pub async fn words_ending_in(
        &self,
        lang: Lang,
        suffix: &str,
        cursor: Option<&str>,
    ) -> Result<Page> {
        let source: Vec<(&str, String)> = match lang.wiki() {
            Wiki::WikiWoordenboek => {
                let start: String = suffix.chars().rev().collect();
                let mut range = vec![
                    ("generator", "categorymembers".into()),
                    ("gcmnamespace", "0".into()),
                    ("gcmtitle", RETROGRADE.into()),
                    ("gcmlimit", "500".into()),
                    ("gcmstartsortkeyprefix", start.clone()),
                ];
                if let Some(end) = prefix_successor(&start) {
                    range.push(("gcmendsortkeyprefix", end));
                }
                range
            }
            Wiki::Wiktionary => vec![
                ("generator", "search".into()),
                ("gsrnamespace", "0".into()),
                ("gsrlimit", SEARCH_SLICE.to_string()),
                (
                    "gsrsearch",
                    format!("{} intitle:/^[a-z]+{suffix}$/", self.wanted(lang)),
                ),
            ],
        };
        let mut page = self.slice(Kind::Words, lang, source, cursor).await?;
        page.words.retain(|w| w.text.ends_with(suffix));
        Ok(page)
    }

    /// The next slice of a list of words, limited to the configured syllable
    /// counts.
    pub async fn words_of(
        &self,
        lang: Lang,
        list: &WordList,
        cursor: Option<&str>,
    ) -> Result<Page> {
        let members = |title: String| -> Vec<(&str, String)> {
            vec![
                ("generator", "categorymembers".into()),
                ("gcmnamespace", "0".into()),
                ("gcmtitle", title),
                ("gcmlimit", TOPIC_SLICE.to_string()),
            ]
        };
        // A search instead of a plain list of members puts the best known
        // words first, which is all the English Wiktionary has to go by.
        let search = |query: String| -> Vec<(&str, String)> {
            vec![
                ("generator", "search".into()),
                ("gsrnamespace", "0".into()),
                ("gsrlimit", TOPIC_SLICE.to_string()),
                ("gsrsearch", query),
            ]
        };
        let words =
            |filter: String| search(format!("{}{filter} intitle:/^[a-z]+$/", self.wanted(lang)));
        let source = match (lang.wiki(), list) {
            // The English Wiktionary has the first names of every language,
            // spelled with a capital.
            (_, WordList::Names) => {
                return self
                    .slice(Kind::Names, lang, search(names_query(lang)), cursor)
                    .await;
            }
            (Wiki::WikiWoordenboek, WordList::About(topic)) => members(topics::title(lang, topic)),
            // With the topics under it: Animals files hardly an animal
            // itself, they are under Mammals, Birds and Fish. The search goes
            // five levels down, into 256 topics at most.
            (Wiki::Wiktionary, WordList::About(topic)) => {
                words(format!(" deepcat:\"{}:{topic}\"", lang.code()))
            }
            (Wiki::WikiWoordenboek, WordList::KnownBy(share)) => {
                let mut source = members(format!("Categorie:Prevalentie Nederland {share} %"));
                // In the order the words were added: the everyday ones came
                // first, and it is not all words on a- to begin with.
                source.push(("gcmsort", "timestamp".into()));
                source
            }
            (Wiki::WikiWoordenboek, WordList::Popular)
            | (Wiki::Wiktionary, WordList::KnownBy(_)) => {
                bail!("{} has no such list of words", lang.dictionary())
            }
            (Wiki::Wiktionary, WordList::Popular) => search(format!(
                "{} {} intitle:/^[a-z]{{3,}}$/",
                self.wanted(lang),
                everyday(lang)
            )),
        };
        let mut page = self.slice(Kind::Words, lang, source, cursor).await?;
        for word in &mut page.words {
            match list {
                // Every word carries at least the topic it was found under.
                WordList::About(topic) if !word.topics.contains(topic) => {
                    word.topics.push(topic.clone());
                    word.topics.sort();
                }
                WordList::KnownBy(share) => word.prevalence = Some(*share),
                _ => {}
            }
        }
        Ok(page)
    }

    /// The search filter for the words of a language in the English
    /// Wiktionary: the wanted numbers of syllables and, where a language is
    /// hunted without its inflected forms, dictionary forms only.
    fn wanted(&self, lang: Lang) -> String {
        let name = lang.name();
        let categories: Vec<String> = self
            .syllables
            .iter()
            .map(|n| format!("{name} {n}-syllable words"))
            .collect();
        let mut filter = format!("incategory:\"{}\"", categories.join("|"));
        if lang.lemmas_only() {
            filter.push_str(&format!(" incategory:\"{name} lemmas\""));
        }
        filter
    }

    /// One slice of a list of words (the members of a category, or the
    /// results of a search), with what we know about each word. Words of
    /// `lang` are read from its own dictionary, its first names from the
    /// English Wiktionary.
    async fn slice(
        &self,
        kind: Kind,
        lang: Lang,
        source: Vec<(&str, String)>,
        cursor: Option<&str>,
    ) -> Result<Page> {
        let wiki = match kind {
            Kind::Names => Wiki::Wiktionary,
            Kind::Words => lang.wiki(),
        };
        // Only the list of names goes to a dictionary that is not the
        // language's own.
        let asked = match kind {
            Kind::Names => self.name_filters[&lang].clone(),
            Kind::Words => self.filter(lang).to_string(),
        };
        let mut params: Vec<(&str, String)> = vec![
            ("action", "query".into()),
            ("prop", "categories".into()),
            ("cllimit", "max".into()),
            ("clcategories", asked),
        ];
        params.extend(source);

        // A stored cursor is the API's own "continue" object.
        let mut cont: BTreeMap<String, String> = match cursor {
            Some(c) => serde_json::from_str(c).context("corrupt dictionary cursor")?,
            None => BTreeMap::new(),
        };
        let mut pages: HashMap<String, Found> = HashMap::new();
        let next = loop {
            let body = self.call(wiki, &params, &cont).await?;
            // Half a search result would look like the end of the list, and
            // so would none at all when the topics under a topic could not be
            // looked up. Too many of them is fine: the search uses a part.
            let warning = body.pointer("/warnings/search/warnings");
            if let Some(warning) = warning.and_then(Value::as_str) {
                if warning.contains("timed out") {
                    bail!("the search of {} timed out", wiki.name());
                }
                if warning.to_lowercase().contains("deep category search") {
                    bail!("the search of {} failed: {warning}", wiki.name());
                }
            }
            collect_pages(&body, &mut pages);
            let next_cont = continue_map(&body);
            // `batchcomplete` means every category of this slice has arrived;
            // until then the API hands out the rest in follow-up requests.
            if body.get("batchcomplete").is_some() || next_cont.is_none() {
                break next_cont;
            }
            cont = next_cont.unwrap_or_default();
        };

        let mut words: Vec<(Word, Option<u64>)> = pages
            .into_iter()
            .map(|(title, found)| {
                let mut word = to_word(wiki, lang, title, &found.categories);
                if kind == Kind::Names {
                    word.text = word.text.to_lowercase();
                    word.name = true;
                    word.topics = vec![topics::NAMES.to_string()];
                }
                (word, found.rank)
            })
            .filter(|(w, _)| is_plain(&w.text))
            // The dictionary counted the syllables of few names: one
            // without a count is welcome, as it may well be short enough.
            .filter(|(w, _)| match w.syllables {
                Some(n) => self.syllables.contains(&n),
                None => kind == Kind::Names,
            })
            .collect();
        // Best known words first, so the nicest names surface early. Where
        // nobody measured that, the order of the search results is a fair
        // stand-in.
        words.sort_by(|(a, a_rank), (b, b_rank)| {
            b.prevalence
                .cmp(&a.prevalence)
                .then(a_rank.cmp(b_rank))
                .then(a.text.len().cmp(&b.text.len()))
                .then(a.text.cmp(&b.text))
        });
        Ok(Page {
            words: words.into_iter().map(|(word, _)| word).collect(),
            next: next.map(|c| serde_json::to_string(&c)).transpose()?,
        })
    }

    /// The topics words of a language are filed under, with the number of
    /// words in each, biggest first.
    pub async fn topics(&self, lang: Lang) -> Result<Vec<(String, u64)>> {
        let wiki = lang.wiki();
        let params: Vec<(&str, String)> = match wiki {
            // Every subcategory of one category.
            Wiki::WikiWoordenboek => vec![
                ("action", "query".into()),
                ("generator", "categorymembers".into()),
                ("gcmtitle", TOPICS.into()),
                ("gcmtype", "subcat".into()),
                ("gcmlimit", "500".into()),
                ("prop", "categoryinfo".into()),
            ],
            // Thousands of categories called "en:<topic>": only the bigger ones.
            Wiki::Wiktionary => vec![
                ("action", "query".into()),
                ("list", "allcategories".into()),
                ("acprefix", format!("{}:", lang.code())),
                ("acmin", topic_min_size(lang).to_string()),
                ("aclimit", "500".into()),
                ("acprop", "size".into()),
            ],
        };
        let mut cont = BTreeMap::new();
        let mut sizes: HashMap<String, u64> = HashMap::new();
        loop {
            let body = self.call(wiki, &params, &cont).await?;
            let listed = match wiki {
                Wiki::WikiWoordenboek => body.pointer("/query/pages"),
                Wiki::Wiktionary => body.pointer("/query/allcategories"),
            };
            for entry in listed.and_then(Value::as_array).into_iter().flatten() {
                let (topic, size) = match wiki {
                    Wiki::WikiWoordenboek => (
                        entry
                            .get("title")
                            .and_then(Value::as_str)
                            .and_then(|title| topics::topic_of(lang, title)),
                        entry.pointer("/categoryinfo/pages"),
                    ),
                    Wiki::Wiktionary => (
                        entry
                            .get("category")
                            .and_then(Value::as_str)
                            .and_then(|name| name.strip_prefix(lang.code())?.strip_prefix(':')),
                        entry.get("pages"),
                    ),
                };
                if let (Some(topic), Some(size)) = (topic, size.and_then(Value::as_u64)) {
                    sizes.insert(topic.to_string(), size);
                }
            }
            match continue_map(&body) {
                Some(next) => cont = next,
                None => break,
            }
        }
        let mut all: Vec<(String, u64)> = sizes
            .into_iter()
            .filter(|(_, n)| *n > 0)
            // The English dictionary has hundreds of lists of place names
            // ("Villages in Wales", "Places in Texas, USA").
            .filter(|(topic, _)| !topic.contains(" in ") && !topic.contains(','))
            .collect();
        all.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        Ok(all)
    }

    /// Look up a single word. `None` when the dictionary does not know it.
    pub async fn lookup(&self, lang: Lang, word: &str) -> Result<Option<Word>> {
        let params: Vec<(&str, String)> = vec![
            ("action", "query".into()),
            ("titles", word.into()),
            ("prop", "categories".into()),
            ("cllimit", "max".into()),
            ("clcategories", self.filter(lang).to_string()),
        ];
        let body = self.call(lang.wiki(), &params, &BTreeMap::new()).await?;
        let Some(page) = body.pointer("/query/pages/0") else {
            return Ok(None);
        };
        if page.get("missing").is_some() || page.get("invalid").is_some() {
            return Ok(None);
        }
        let mut pages = HashMap::new();
        collect_pages(&body, &mut pages);
        Ok(pages
            .into_iter()
            .next()
            .map(|(title, found)| to_word(lang.wiki(), lang, title, &found.categories)))
    }

    /// What a word means, in a few words of English: the first sense the
    /// English Wiktionary gives for it as a word of `lang`. `None` when it
    /// does not describe the word. One request per word.
    pub async fn meaning(&self, lang: Lang, word: &str) -> Result<Option<String>> {
        // A word of the hunt, or a first name with its capital.
        if word.is_empty() || !word.chars().all(|c| c.is_ascii_alphabetic()) {
            return Ok(None);
        }
        let is_name = word.starts_with(|c: char| c.is_ascii_uppercase());
        let _one_at_a_time = self.gate.lock().await;
        let mut problem = String::new();
        for attempt in 1..=MEANING_ATTEMPTS {
            self.pace().await;
            let request = Request::get(format!("{DEFINITIONS}/{word}"))
                .header("user-agent", self.user_agent());
            let reply = match self.web.get(request).await {
                Ok(reply) => reply,
                Err(reason) => {
                    problem = format!("Wiktionary is unreachable ({reason})");
                    tokio::time::sleep(Duration::from_secs(attempt)).await;
                    continue;
                }
            };
            if reply.status == 404 {
                return Ok(None);
            }
            if !reply.is_success() {
                problem = format!("Wiktionary answered HTTP {}", reply.status);
                tokio::time::sleep(patience(reply.retry_after)).await;
                continue;
            }
            match reply.json() {
                Some(body) => return Ok(first_sense(&body, lang, is_name)),
                None => problem = "Wiktionary answered with something unreadable".to_string(),
            }
        }
        bail!("{problem}, gave up after {MEANING_ATTEMPTS} attempts")
    }

    /// Wait until the next request fits in the rate limit, and count it.
    async fn pace(&self) {
        loop {
            let wait = self.wait_time();
            if wait.is_zero() {
                break;
            }
            tokio::time::sleep(wait).await;
        }
        self.recent
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push_back(Instant::now());
    }

    /// One request to a dictionary. Both share one request budget, as they
    /// are both Wikimedia's.
    async fn call(
        &self,
        wiki: Wiki,
        params: &[(&str, String)],
        cont: &BTreeMap<String, String>,
    ) -> Result<Value> {
        let dictionary = wiki.name();
        let _one_at_a_time = self.gate.lock().await;
        let mut problem = String::new();
        for attempt in 1..=ATTEMPTS {
            self.pace().await;
            let mut request = Request::get(wiki.api())
                .header("user-agent", self.user_agent())
                .query("format", "json")
                .query("formatversion", "2")
                .query("maxlag", "5");
            for (name, value) in params {
                request = request.query(name, value.as_str());
            }
            for (name, value) in cont {
                request = request.query(name, value.as_str());
            }
            let reply = match self.web.get(request).await {
                Ok(reply) => reply,
                Err(reason) => {
                    problem = format!("{dictionary} is unreachable ({reason})");
                    tokio::time::sleep(Duration::from_secs(attempt)).await;
                    continue;
                }
            };
            let status = reply.status;
            // Too many requests or a busy server: wait as long as we are told to.
            if status == 429 || reply.is_server_error() {
                problem = format!("{dictionary} answered HTTP {status}");
                tokio::time::sleep(patience(reply.retry_after)).await;
                continue;
            }
            let Some(body) = reply.json() else {
                problem = format!("{dictionary} answered HTTP {status} with something unreadable");
                tokio::time::sleep(Duration::from_secs(attempt)).await;
                continue;
            };
            if let Some(error) = body.get("error") {
                let code = error
                    .get("code")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown");
                // "maxlag": the servers are busy and ask us to come back later.
                if code == "maxlag" || code == "ratelimited" {
                    problem = format!("{dictionary} is busy ({code})");
                    tokio::time::sleep(patience(reply.retry_after)).await;
                    continue;
                }
                let info = error.get("info").and_then(Value::as_str).unwrap_or("");
                bail!("{dictionary} API error {code}: {info}");
            }
            return Ok(body);
        }
        bail!("{problem}, gave up after {ATTEMPTS} attempts")
    }
}

/// How long to leave a server alone that asked for it: as long as it says,
/// within reason.
fn patience(retry_after: Option<u64>) -> Duration {
    Duration::from_secs(retry_after.unwrap_or(5).clamp(1, 60))
}

/// The filter that keeps the best known words everyday ones. The best known
/// pages of a dictionary are the ones it links to all the time itself: the
/// words of its own trade (genitive, plural, surname). Those are left out,
/// and so are words of one or two letters, which are mostly other
/// languages' (la, de, os).
fn everyday(lang: Lang) -> String {
    let trade = [
        "Grammar",
        "Linguistics",
        "Grammatical cases",
        "Grammatical moods",
        "Parts of speech",
        "Lexicography",
        "Phonetics",
        "Phonology",
        "Syntax",
        "Semantics",
        "Orthography",
        "Onomastics",
    ]
    .map(|topic| format!("{}:{topic}", lang.code()));
    format!("-incategory:\"{}\"", trade.join("|"))
}

/// The search for the first names of a language: every list the English
/// Wiktionary keeps of them, short forms like Mike included. Asking for the
/// words "given name" as well puts the pages first that are about little
/// else (Emma, Hans), ahead of well known pages that happen to be a name
/// too (China, Texas).
fn names_query(lang: Lang) -> String {
    let lists = [
        "given names",
        "male given names",
        "female given names",
        "unisex given names",
        "diminutives of male given names",
        "diminutives of female given names",
    ]
    .map(|list| format!("{} {list}", lang.name()));
    format!(
        "\"given name\" incategory:\"{}\" intitle:/^[A-Z][a-z]+$/",
        lists.join("|")
    )
}

fn syllable_category(wiki: Wiki, lang: Lang, n: u8) -> String {
    match wiki {
        Wiki::WikiWoordenboek => {
            let noun = if n == 1 {
                "lettergreep"
            } else {
                "lettergrepen"
            };
            format!("Categorie:Woorden met {n} {noun} in het Nederlands")
        }
        Wiki::Wiktionary => format!("Category:{} {n}-syllable words", lang.name()),
    }
}

/// Topics have to be at least this big to be listed. English has thousands
/// of them; the other languages have fewer words filed under each.
fn topic_min_size(lang: Lang) -> usize {
    match lang {
        Lang::English => 100,
        _ => 25,
    }
}

/// The number in "Categorie:Woorden met 2 lettergrepen in het Nederlands" or
/// "Category:English 2-syllable words".
fn syllables_of(wiki: Wiki, lang: Lang, category: &str) -> Option<u8> {
    let count = match wiki {
        Wiki::WikiWoordenboek => category
            .strip_prefix("Categorie:Woorden met ")
            .filter(|rest| rest.contains("lettergre"))?
            .split(' ')
            .next(),
        // A page lists every language that spells a word this way: "more" is
        // one syllable in English and two in Latin.
        Wiki::Wiktionary => category
            .strip_prefix("Category:")?
            .strip_prefix(lang.name())?
            .strip_prefix(' ')?
            .strip_suffix("-syllable words"),
    };
    count?.parse().ok()
}

/// Smallest string that sorts after everything starting with `prefix`.
fn prefix_successor(prefix: &str) -> Option<String> {
    let mut chars: Vec<char> = prefix.chars().collect();
    while let Some(last) = chars.pop() {
        if last < 'z' {
            chars.push(char::from_u32(last as u32 + 1)?);
            return Some(chars.into_iter().collect());
        }
    }
    None
}

fn continue_map(body: &Value) -> Option<BTreeMap<String, String>> {
    let object = body.get("continue")?.as_object()?;
    Some(
        object
            .iter()
            .map(|(k, v)| {
                (
                    k.clone(),
                    v.as_str()
                        .map(str::to_string)
                        .unwrap_or_else(|| v.to_string()),
                )
            })
            .collect(),
    )
}

fn collect_pages(body: &Value, pages: &mut HashMap<String, Found>) {
    let Some(list) = body.pointer("/query/pages").and_then(Value::as_array) else {
        return;
    };
    for page in list {
        let Some(title) = page.get("title").and_then(Value::as_str) else {
            continue;
        };
        let entry = pages.entry(title.to_string()).or_default();
        if let Some(rank) = page.get("index").and_then(Value::as_u64) {
            entry.rank = Some(rank);
        }
        if let Some(categories) = page.get("categories").and_then(Value::as_array) {
            entry.categories.extend(
                categories
                    .iter()
                    .filter_map(|c| c.get("title").and_then(Value::as_str))
                    .map(str::to_string),
            );
        }
    }
}

/// The first sense of a word in the answer of the definition service:
/// `{"la": [{"partOfSpeech": "Noun", "definitions": [{"definition": "<html>"}]}]}`.
/// A sense of its own ("truth") beats one that only points at another word
/// ("genitive singular of veritas"). And for a first name, the sense that
/// says so beats the others: Georgia is a country too.
fn first_sense(body: &Value, lang: Lang, is_name: bool) -> Option<String> {
    let senses: Vec<&str> = body
        .get(lang.code())
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.get("definitions")?.as_array())
        .flatten()
        .filter_map(|sense| sense.get("definition")?.as_str())
        .collect();
    let (refers, says): (Vec<&str>, Vec<&str>) = senses
        .into_iter()
        .partition(|sense| sense.contains("form-of-definition"));
    let mut senses: Vec<String> = says
        .into_iter()
        .chain(refers)
        .map(|sense| concise(&plain_text(sense)))
        .filter(|text| !text.is_empty())
        .collect();
    let pick = senses
        .iter()
        .position(|sense| is_name && sense.contains("given name"))
        .unwrap_or(0);
    (pick < senses.len()).then(|| senses.swap_remove(pick))
}

/// The text of a piece of HTML.
fn plain_text(html: &str) -> String {
    let mut text = String::new();
    let mut rest = html;
    while let Some(start) = rest.find('<') {
        text.push_str(&rest[..start]);
        rest = &rest[start..];
        // What is inside these is not for reading.
        let hidden = ["style", "script", "sup"].into_iter().find(|tag| {
            rest[1..].starts_with(tag) && rest[1 + tag.len()..].starts_with([' ', '>'])
        });
        let end = match hidden.and_then(|tag| rest.find(&format!("</{tag}>"))) {
            Some(close) => rest[close..].find('>').map(|end| close + end),
            None => rest.find('>'),
        };
        match end {
            Some(end) => rest = &rest[end + 1..],
            None => rest = "",
        }
    }
    text.push_str(rest);
    let text = text
        .replace("&nbsp;", " ")
        .replace("&#160;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&amp;", "&");
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A sense cut down to a few words: "parrot, member of the order …".
fn concise(sense: &str) -> String {
    let sense = sense.split(';').next().unwrap_or_default();
    // "A male bird, especially:" introduces a list we do not show.
    let sense = sense.trim().trim_end_matches(['.', ':', ',']).trim_end();
    if sense.chars().count() <= MEANING_LENGTH {
        return sense.to_string();
    }
    let cut: String = sense.chars().take(MEANING_LENGTH).collect();
    // Not in the middle of a word.
    let cut = cut.rsplit_once(' ').map_or(cut.as_str(), |(kept, _)| kept);
    format!("{}…", cut.trim_end_matches([',', ':', ' ', '(']))
}

fn to_word(wiki: Wiki, lang: Lang, text: String, categories: &[String]) -> Word {
    let mut word = Word {
        text,
        ..Word::default()
    };
    let tracked = topics::tracked(lang);
    for category in categories {
        if category == OFFICIAL {
            word.official = true;
        } else if let Some(flag) = tone::flag_of(wiki, lang, category) {
            word.tone.push(flag.to_string());
        } else if let Some(syllables) = syllables_of(wiki, lang, category) {
            // A word can be pronounced in more than one way: count the
            // shortest.
            word.syllables = Some(word.syllables.map_or(syllables, |n| n.min(syllables)));
        } else if let Some(rest) = category.strip_prefix("Categorie:Prevalentie Nederland ") {
            word.prevalence = rest.trim_end_matches('%').trim().parse().ok();
        } else if let Some(topic) = topics::topic_of(lang, category).filter(|_| wiki == lang.wiki())
        {
            // "Woorden in het Nederlands" looks like a topic but is not one.
            if tracked.contains(&topic) {
                word.topics.push(topic.to_string());
            }
        }
    }
    word.topics.sort();
    word.topics.dedup();
    word.tone.sort();
    word.tone.dedup();
    word
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::fake;

    /// A word as its own dictionary describes it.
    /// A page of a dictionary answer: a title, its place in a search and
    /// the categories it is in.
    fn page(title: &str, rank: u64, categories: &[&str]) -> Value {
        let categories: Vec<Value> = categories
            .iter()
            .map(|title| serde_json::json!({ "title": title }))
            .collect();
        serde_json::json!({ "title": title, "index": rank, "categories": categories })
    }

    fn answer(pages: Vec<Value>) -> String {
        serde_json::json!({ "batchcomplete": true, "query": { "pages": pages } }).to_string()
    }

    fn dictionary(syllables: &[u8], contact: Option<&str>) -> (Wiktionary, Arc<fake::Web>) {
        let web = Arc::new(fake::Web::new());
        let wiki = Wiktionary::new(web.clone(), syllables, contact.map(str::to_string));
        (wiki, web)
    }

    const TWO: &str = "Category:English 2-syllable words";
    const THREE: &str = "Category:English 3-syllable words";

    #[tokio::test(start_paused = true)]
    async fn english_words_come_from_a_search_best_known_first() {
        let (wiki, web) = dictionary(&[1, 2], None);
        web.on(
            "en.wiktionary.org",
            200,
            &answer(vec![
                page("baking", 2, &[TWO, "Category:en:Banking"]),
                page("cooking", 1, &[TWO]),
                // Too long, not plain letters, and not on -ing after all.
                page("offering", 3, &[THREE]),
                page("check-ing", 4, &[TWO]),
                page("kingdom", 5, &[TWO]),
            ]),
        );
        let found = wiki
            .words_ending_in(Lang::English, "ing", None)
            .await
            .unwrap();
        let words: Vec<&str> = found.words.iter().map(|w| w.text.as_str()).collect();
        assert_eq!(words, vec!["cooking", "baking"]);
        assert_eq!(found.words[1].topics, vec!["Banking"]);
        assert_eq!(found.next, None);

        let request = &web.requests()[0];
        let asked = |name: &str| {
            let hit = request.query.iter().find(|(n, _)| n == name);
            hit.map(|(_, value)| value.as_str()).unwrap_or_default()
        };
        assert_eq!(request.url, "https://en.wiktionary.org/w/api.php");
        assert_eq!(asked("generator"), "search");
        assert!(
            asked("gsrsearch").ends_with("intitle:/^[a-z]+ing$/"),
            "{}",
            asked("gsrsearch")
        );
        assert!(asked("gsrsearch").contains("English 1-syllable words|English 2-syllable words"));
        // Polite to a busy server, and clear about who is asking.
        assert_eq!(asked("maxlag"), "5");
        let agent = &request
            .headers
            .iter()
            .find(|(name, _)| *name == "user-agent")
            .unwrap()
            .1;
        assert!(
            agent.starts_with("domainjaeger/") && agent.contains(HOMEPAGE),
            "{agent}"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_page_ends_with_where_the_next_one_starts() {
        let (wiki, web) = dictionary(&[2], None);
        let first = serde_json::json!({
            "batchcomplete": true,
            "continue": { "gsroffset": 300, "continue": "gsroffset||" },
            "query": { "pages": [page("cooking", 1, &[TWO])] },
        });
        web.on(
            "gsroffset=300",
            200,
            &answer(vec![page("baking", 1, &[TWO])]),
        )
        .on("en.wiktionary.org", 200, &first.to_string());
        let one = wiki
            .words_ending_in(Lang::English, "ing", None)
            .await
            .unwrap();
        assert_eq!(one.words[0].text, "cooking");
        let cursor = one.next.expect("there is more");
        let two = wiki
            .words_ending_in(Lang::English, "ing", Some(&cursor))
            .await
            .unwrap();
        assert_eq!(two.words[0].text, "baking");
        assert_eq!(two.next, None);
        // A cursor that is not ours is refused before anything is asked.
        let asked = web.asked().len();
        assert!(
            wiki.words_ending_in(Lang::English, "ing", Some("nonsense"))
                .await
                .is_err()
        );
        assert_eq!(web.asked().len(), asked);
    }

    #[tokio::test(start_paused = true)]
    async fn an_answer_that_comes_in_parts_is_put_together() {
        let (wiki, web) = dictionary(&[2], None);
        // The categories of a page can arrive in a second helping.
        let part = serde_json::json!({
            "continue": { "clcontinue": "42|x", "continue": "||" },
            "query": { "pages": [page("baking", 1, &[])] },
        });
        web.on(
            "clcontinue=42|x",
            200,
            &answer(vec![page("baking", 1, &[TWO, "Category:en:Banking"])]),
        )
        .on("en.wiktionary.org", 200, &part.to_string());
        let found = wiki
            .words_ending_in(Lang::English, "ing", None)
            .await
            .unwrap();
        assert_eq!(web.asked().len(), 2);
        assert_eq!(found.words.len(), 1);
        assert_eq!(found.words[0].syllables, Some(2));
        assert_eq!(found.words[0].topics, vec!["Banking"]);
    }

    #[tokio::test(start_paused = true)]
    async fn dutch_words_come_from_the_index_of_endings() {
        let (wiki, web) = dictionary(&[1], None);
        let one = "Categorie:Woorden met 1 lettergreep in het Nederlands";
        web.on(
            "nl.wiktionary.org",
            200,
            &answer(vec![
                page("knap", 0, &[one, "Categorie:Prevalentie Nederland 99 %"]),
                page(
                    "kap",
                    0,
                    &[one, "Categorie:Prevalentie Nederland 100 %", OFFICIAL],
                ),
            ]),
        );
        let found = wiki.words_ending_in(Lang::Dutch, "ap", None).await.unwrap();
        // The word most people know comes first.
        let words: Vec<(&str, Option<u8>)> = found
            .words
            .iter()
            .map(|w| (w.text.as_str(), w.prevalence))
            .collect();
        assert_eq!(words, vec![("kap", Some(100)), ("knap", Some(99))]);
        assert!(found.words[0].official && !found.words[1].official);
        // The index is sorted by the word spelled backwards.
        let address = &web.asked()[0];
        assert!(
            address.contains("gcmstartsortkeyprefix=pa&gcmendsortkeyprefix=pb"),
            "{address}"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn first_names_are_words_too() {
        let (wiki, web) = dictionary(&[1, 2], None);
        web.on(
            "en.wiktionary.org",
            200,
            &answer(vec![
                page("Mike", 1, &["Category:English male given names"]),
                page("Emma", 2, &[TWO]),
            ]),
        );
        let found = wiki
            .words_of(Lang::Dutch, &WordList::Names, None)
            .await
            .unwrap();
        let names: Vec<(&str, bool)> = found
            .words
            .iter()
            .map(|w| (w.text.as_str(), w.name))
            .collect();
        // Lower case to make names of, and welcome without a syllable count.
        assert_eq!(names, vec![("mike", true), ("emma", true)]);
        assert_eq!(found.words[0].shown(), "Mike");
        assert_eq!(found.words[0].topics, vec![topics::NAMES]);
        // Dutch names are listed by the English dictionary as well.
        assert!(web.asked()[0].starts_with("https://en.wiktionary.org"));
        assert!(web.asked()[0].contains("Dutch given names"));
    }

    #[tokio::test(start_paused = true)]
    async fn lists_of_words_carry_what_they_were_asked_for() {
        let (wiki, web) = dictionary(&[2], None);
        web.on(
            "en.wiktionary.org",
            200,
            &answer(vec![page("ledger", 1, &[TWO])]),
        );
        let about = WordList::About("Banking".into());
        let found = wiki.words_of(Lang::English, &about, None).await.unwrap();
        assert_eq!(found.words[0].topics, vec!["Banking"]);
        assert!(web.asked()[0].contains("deepcat:\"en:Banking\""));
        // Each dictionary has one way of telling which words are well known.
        assert!(
            wiki.words_of(Lang::Dutch, &WordList::Popular, None)
                .await
                .is_err()
        );
        assert!(
            wiki.words_of(Lang::English, &WordList::KnownBy(99), None)
                .await
                .is_err()
        );

        let (wiki, web) = dictionary(&[1], None);
        let one = "Categorie:Woorden met 1 lettergreep in het Nederlands";
        web.on(
            "nl.wiktionary.org",
            200,
            &answer(vec![page("fiets", 0, &[one])]),
        );
        let found = wiki
            .words_of(Lang::Dutch, &WordList::KnownBy(98), None)
            .await
            .unwrap();
        assert_eq!(found.words[0].prevalence, Some(98));
        assert!(web.asked()[0].contains("Prevalentie Nederland 98 %"));
    }

    #[tokio::test(start_paused = true)]
    async fn a_busy_dictionary_is_given_the_time_it_asks_for() {
        let (wiki, web) = dictionary(&[2], None);
        let busy = r#"{"error":{"code":"maxlag","info":"Waiting for a database server"}}"#;
        web.busy("en.wiktionary.org", 429, 7)
            .on("en.wiktionary.org", 200, busy)
            .on(
                "en.wiktionary.org",
                200,
                &answer(vec![page("cooking", 1, &[TWO])]),
            );
        let started = Instant::now();
        let found = wiki
            .words_ending_in(Lang::English, "ing", None)
            .await
            .unwrap();
        assert_eq!(found.words.len(), 1);
        assert_eq!(web.asked().len(), 3);
        // Seven seconds as told, and five more for the busy database.
        assert!(
            started.elapsed() >= Duration::from_secs(12),
            "{:?}",
            started.elapsed()
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_dictionary_that_stays_away_is_reported_in_plain_words() {
        let (wiki, web) = dictionary(&[2], None);
        web.down("en.wiktionary.org");
        let problem = wiki
            .words_ending_in(Lang::English, "ing", None)
            .await
            .unwrap_err();
        assert_eq!(
            problem.to_string(),
            "Wiktionary is unreachable (cannot connect), gave up after 6 attempts"
        );
        assert_eq!(web.asked().len(), 6);

        // An error of the API itself is not tried again.
        let (wiki, web) = dictionary(&[2], None);
        web.on(
            "nl.wiktionary.org",
            200,
            r#"{"error":{"code":"badvalue","info":"Unrecognized value"}}"#,
        );
        let problem = wiki
            .words_ending_in(Lang::Dutch, "ap", None)
            .await
            .unwrap_err();
        assert_eq!(
            problem.to_string(),
            "WikiWoordenboek API error badvalue: Unrecognized value"
        );
        assert_eq!(web.asked().len(), 1);

        // Half a search result would look like the end of the list.
        let (wiki, web) = dictionary(&[2], None);
        let slow =
            r#"{"batchcomplete":true,"warnings":{"search":{"warnings":"The search timed out"}}}"#;
        web.on("en.wiktionary.org", 200, slow);
        let problem = wiki
            .words_ending_in(Lang::English, "ing", None)
            .await
            .unwrap_err();
        assert!(problem.to_string().contains("timed out"));
    }

    #[tokio::test(start_paused = true)]
    async fn a_topic_takes_in_the_topics_under_it_or_says_it_could_not() {
        let animals = WordList::About("Animals".into());
        let warned = |warning: &str| {
            let mut body: Value = serde_json::from_str(&answer(vec![page("fox", 1, &[TWO])]))
                .expect("an answer is JSON");
            body["warnings"] = serde_json::json!({ "search": { "warnings": warning } });
            body.to_string()
        };
        // A topic with more under it than the search takes in gives a part.
        let (wiki, web) = dictionary(&[2], None);
        let many = "Deep category query returned too many categories. \
                    Only a subset of categories has been applied.";
        web.on("en.wiktionary.org", 200, &warned(many));
        let found = wiki.words_of(Lang::English, &animals, None).await.unwrap();
        assert_eq!(found.words[0].text, "fox");
        assert!(web.asked()[0].contains("deepcat:\"en:Animals\""));

        // When they could not be looked up, the list is not over.
        let (wiki, web) = dictionary(&[2], None);
        web.on(
            "en.wiktionary.org",
            200,
            &warned("Deep category search SPARQL query failed"),
        );
        let problem = wiki
            .words_of(Lang::English, &animals, None)
            .await
            .unwrap_err();
        assert!(problem.to_string().contains("SPARQL query failed"));
    }

    #[tokio::test(start_paused = true)]
    async fn a_stranger_gets_nine_questions_a_minute_and_a_known_face_many_more() {
        let (wiki, web) = dictionary(&[2], None);
        web.on("rest_v1", 404, "");
        assert_eq!(wiki.spare_requests(), ANONYMOUS_PER_MINUTE);
        let started = Instant::now();
        for _ in 0..ANONYMOUS_PER_MINUTE {
            wiki.meaning(Lang::English, "word").await.unwrap();
        }
        // A short breath between two questions, no more.
        assert!(started.elapsed() < MINUTE / 2, "{:?}", started.elapsed());
        assert_eq!(wiki.spare_requests(), 0);
        // The tenth has to wait for the minute to pass.
        wiki.meaning(Lang::English, "word").await.unwrap();
        assert!(started.elapsed() >= MINUTE, "{:?}", started.elapsed());
        assert_eq!(web.asked().len(), ANONYMOUS_PER_MINUTE + 1);

        // Saying who you are lifts that, and it is passed on.
        wiki.set_contact(Some("me@example.org".into()));
        assert!(wiki.spare_requests() > 100);
        wiki.meaning(Lang::English, "word").await.unwrap();
        let requests = web.requests();
        let agent = &requests.last().unwrap().headers[0].1;
        assert!(
            agent.contains("me@example.org") && agent.contains(HOMEPAGE),
            "{agent}"
        );
        // A dictionary for other word lengths keeps count of the same budget.
        let other = wiki.with_syllables(&[1]);
        assert_eq!(other.contact().as_deref(), Some("me@example.org"));
        assert_eq!(other.spare_requests(), wiki.spare_requests());
    }

    #[tokio::test(start_paused = true)]
    async fn the_meaning_of_a_word_is_one_question() {
        let (wiki, web) = dictionary(&[2], None);
        let senses = r#"{"en":[{"partOfSpeech":"Noun","definitions":[{"definition":"The <a>art</a> of preparing food; more"}]}]}"#;
        web.on("definition/cooking", 200, senses)
            .on("definition/odd", 200, "<html>")
            .busy("definition/busy", 503, 2);
        let meaning = wiki.meaning(Lang::English, "cooking").await.unwrap();
        assert_eq!(meaning.as_deref(), Some("The art of preparing food"));
        // A word the dictionary does not describe has no meaning: no error.
        assert_eq!(wiki.meaning(Lang::English, "qzxqzx").await.unwrap(), None);
        // Described, but not as a word of this language.
        assert_eq!(wiki.meaning(Lang::Dutch, "cooking").await.unwrap(), None);
        // Nothing is asked about what cannot be a word.
        let asked = web.asked().len();
        assert_eq!(wiki.meaning(Lang::English, "sn.app").await.unwrap(), None);
        assert_eq!(wiki.meaning(Lang::English, "").await.unwrap(), None);
        assert_eq!(web.asked().len(), asked);
        // Nonsense and a server that stays busy are given up on.
        assert!(wiki.meaning(Lang::English, "odd").await.is_err());
        let problem = wiki
            .meaning(Lang::English, "busy")
            .await
            .unwrap_err()
            .to_string();
        assert!(
            problem.contains("HTTP 503") && problem.contains("gave up"),
            "{problem}"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn one_word_can_be_looked_up_and_the_subjects_listed() {
        let (wiki, web) = dictionary(&[1, 2, 3], None);
        web.on(
            "titles=snap",
            200,
            &answer(vec![page(
                "snap",
                0,
                &["Category:English 1-syllable words"],
            )]),
        );
        let word = wiki.lookup(Lang::English, "snap").await.unwrap().unwrap();
        assert_eq!((word.text.as_str(), word.syllables), ("snap", Some(1)));
        let missing =
            r#"{"batchcomplete":true,"query":{"pages":[{"title":"qzxqzx","missing":true}]}}"#;
        web.on("titles=qzxqzx", 200, missing);
        assert!(
            wiki.lookup(Lang::English, "qzxqzx")
                .await
                .unwrap()
                .is_none()
        );

        let listed = r#"{"batchcomplete":true,"query":{"allcategories":[
            {"category":"en:Music","pages":4200},{"category":"en:Banking","pages":310},
            {"category":"English lemmas","pages":900000}]}}"#;
        web.on("list=allcategories", 200, listed);
        let topics = wiki.topics(Lang::English).await.unwrap();
        // Biggest first, and only what is filed as a subject of this language.
        assert_eq!(
            topics,
            vec![("Music".to_string(), 4200), ("Banking".to_string(), 310)]
        );
    }

    fn word(lang: Lang, text: &str, categories: &[&str]) -> Word {
        let categories: Vec<String> = categories.iter().map(|c| c.to_string()).collect();
        to_word(lang.wiki(), lang, text.to_string(), &categories)
    }

    #[test]
    fn contacts() {
        assert!(valid_contact("jan@example.nl"));
        assert!(valid_contact("https://example.nl/domainjaeger"));
        assert!(!valid_contact("jan"));
        assert!(!valid_contact("jan @example.nl"));
    }

    #[test]
    fn successor_bounds_the_range() {
        assert_eq!(prefix_successor("ia").as_deref(), Some("ib"));
        assert_eq!(prefix_successor("az").as_deref(), Some("b"));
        assert_eq!(prefix_successor("zz"), None);
    }

    #[test]
    fn categories_become_word_facts() {
        let w = word(
            Lang::Dutch,
            "papegaai",
            &[
                "Categorie:Woorden met 3 lettergrepen in het Nederlands",
                "Categorie:Prevalentie Nederland 99 %",
                OFFICIAL,
            ],
        );
        assert_eq!(
            (w.syllables, w.prevalence, w.official),
            (Some(3), Some(99), true)
        );
        assert!(w.topics.is_empty() && w.tone.is_empty());
        assert_eq!(
            syllable_category(Wiki::WikiWoordenboek, Lang::Dutch, 1),
            "Categorie:Woorden met 1 lettergreep in het Nederlands"
        );
    }

    #[test]
    fn english_categories_become_word_facts() {
        let w = word(
            Lang::English,
            "evening",
            &[
                "Category:English 3-syllable words",
                "Category:English 2-syllable words",
                "Category:en:Finance",
                // Not something we ask about.
                "Category:en:Time",
                "Category:English lemmas",
            ],
        );
        assert_eq!((w.syllables, w.prevalence), (Some(2), None));
        assert_eq!(w.topic_list(), "Finance");
        let three = syllable_category(Wiki::Wiktionary, Lang::English, 3);
        assert_eq!(
            syllables_of(Wiki::Wiktionary, Lang::English, &three),
            Some(3)
        );
        assert_eq!(
            syllables_of(Wiki::WikiWoordenboek, Lang::Dutch, &three),
            None
        );
    }

    #[test]
    fn topics_become_word_facts() {
        let w = word(
            Lang::Dutch,
            "lening",
            &[
                "Categorie:Woorden met 2 lettergrepen in het Nederlands",
                "Categorie:Financieel in het Nederlands",
                "Categorie:Economie in het Nederlands",
                // Not a topic we ask about, and not a topic at all.
                "Categorie:Muziek in het Nederlands",
                "Categorie:Woorden in het Nederlands",
            ],
        );
        assert_eq!(w.syllables, Some(2));
        assert_eq!(w.topic_list(), "Economie, Financieel");
    }

    #[test]
    fn the_filter_fits_in_one_request() {
        let wiki = Wiktionary::new(Arc::new(fake::Web::new()), &[1, 2, 3, 4, 5], None);
        let filters: Vec<&str> = wiki.filter(Lang::Dutch).split('|').collect();
        assert_eq!(filters.len(), MAX_CATEGORY_FILTERS);
        assert!(filters.contains(&"Categorie:Financieel in het Nederlands"));
        // Whatever is left goes to the best known words.
        assert!(filters.contains(&"Categorie:Prevalentie Nederland 100 %"));
        assert!(filters.contains(&"Categorie:Prevalentie Nederland 80 %"));
        // The English Wiktionary has less to ask about: syllables and topics.
        let english: Vec<&str> = wiki.filter(Lang::English).split('|').collect();
        assert!(english.len() <= MAX_CATEGORY_FILTERS);
        assert!(english.contains(&"Category:English 2-syllable words"));
        assert!(english.contains(&"Category:en:Finance"));
        let latin: Vec<&str> = wiki.filter(Lang::Latin).split('|').collect();
        assert_eq!(latin.len(), english.len());
        assert!(latin.contains(&"Category:Latin 2-syllable words"));
        assert!(latin.contains(&"Category:la:Finance"));
        // Every word is asked whether it is one to look twice at.
        assert!(filters.contains(&"Categorie:Scheldwoord in het Nederlands"));
        assert!(english.contains(&"Category:English vulgarities"));
        assert!(latin.contains(&"Category:la:Sex"));
        // Dutch first names are read from the English Wiktionary.
        let names: Vec<&str> = wiki.name_filters[&Lang::Dutch].split('|').collect();
        assert!(names.contains(&"Category:Dutch vulgarities"));
        assert!(names.len() <= MAX_CATEGORY_FILTERS);
    }

    #[test]
    fn searches_ask_for_the_right_words() {
        let wiki = Wiktionary::new(Arc::new(fake::Web::new()), &[1, 2], None);
        // English is hunted with its forms (cooking), the others without.
        assert_eq!(
            wiki.wanted(Lang::English),
            "incategory:\"English 1-syllable words|English 2-syllable words\""
        );
        assert!(everyday(Lang::Spanish).starts_with("-incategory:\"es:Grammar|es:Linguistics|"));
        assert_eq!(
            wiki.wanted(Lang::Spanish),
            "incategory:\"Spanish 1-syllable words|Spanish 2-syllable words\" \
             incategory:\"Spanish lemmas\""
        );
        // A page describes every language that spells a word this way.
        let more = word(
            Lang::Latin,
            "more",
            &[
                "Category:English 1-syllable words",
                "Category:Latin 2-syllable words",
                "Category:en:Finance",
                "Category:la:Money",
                "Category:English vulgarities",
            ],
        );
        assert_eq!(
            (more.syllables, more.topic_list().as_str()),
            (Some(2), "Money")
        );
        assert!(more.tone.is_empty());
    }

    #[test]
    fn tone_and_names_become_word_facts() {
        let rude = word(
            Lang::English,
            "cock",
            &[
                "Category:English 1-syllable words",
                "Category:en:Genitalia",
                "Category:en:Sex",
                "Category:English vulgarities",
            ],
        );
        assert_eq!(rude.tone_list(), "vulgar, sexual");
        assert_eq!(rude.shown(), "cock");
        let dutch = word(
            Lang::Dutch,
            "kut",
            &["Categorie:Scheldwoord in het Nederlands"],
        );
        assert_eq!(dutch.tone_list(), "vulgar");
        // A first name is spelled the way a domain wants it, and shown the
        // way the dictionary spells it.
        let mike = Word {
            text: "mike".into(),
            name: true,
            ..Word::default()
        };
        assert_eq!(mike.shown(), "Mike");
        assert_eq!(
            names_query(Lang::Dutch),
            "\"given name\" incategory:\"Dutch given names|Dutch male given names|Dutch female given names|\
             Dutch unisex given names|Dutch diminutives of male given names|\
             Dutch diminutives of female given names\" intitle:/^[A-Z][a-z]+$/"
        );
        // Shelves from before words had a tone can still be read.
        let old: Word = serde_json::from_str(
            r#"{"text":"x","syllables":1,"prevalence":null,"official":false}"#,
        )
        .unwrap();
        assert!(old.tone.is_empty() && !old.name);
    }

    #[test]
    fn meanings_are_a_few_words() {
        let body: Value = serde_json::from_str(
            r##"{
              "en": [{"partOfSpeech": "Noun", "definitions": [{"definition": "An English sense."}]}],
              "la": [
                {"partOfSpeech": "Participle", "definitions": [
                  {"definition": "<span class=\"form-of-definition use-with-mention\">accusative feminine plural of <a href=\"/wiki/veritus\">veritus</a></span>"}]},
                {"partOfSpeech": "Noun", "definitions": [
                  {"definition": ""},
                  {"definition": "<a rel=\"mw:WikiLink\" href=\"/wiki/truth\">truth</a>, truthfulness<sup class=\"reference\">[1]</sup>, verity; the true nature"},
                  {"definition": "reality"}]}
              ]
            }"##,
        )
        .unwrap();
        // The first sense that says something, in the language of the word.
        assert_eq!(
            first_sense(&body, Lang::Latin, false).as_deref(),
            Some("truth, truthfulness, verity")
        );
        assert_eq!(
            first_sense(&body, Lang::English, false).as_deref(),
            Some("An English sense")
        );
        assert_eq!(first_sense(&body, Lang::Spanish, false), None);
        // A first name is told as a name, whatever else it is.
        let georgia: Value = serde_json::from_str(
            r##"{"en": [{"definitions": [
                {"definition": "A country in the Caucasus."},
                {"definition": "A female <a href=\"/wiki/given_name\">given name</a> from Ancient Greek."}]}]}"##,
        )
        .unwrap();
        assert_eq!(
            first_sense(&georgia, Lang::English, true).as_deref(),
            Some("A female given name from Ancient Greek")
        );
        assert_eq!(
            first_sense(&georgia, Lang::English, false).as_deref(),
            Some("A country in the Caucasus")
        );
        assert_eq!(
            concise("A male bird, especially:"),
            "A male bird, especially"
        );
        // A word that is only a form of another one says so.
        let form: Value = serde_json::from_str(
            r##"{"la": [{"definitions": [{"definition":
                "<span class=\"form-of-definition\">genitive singular of <i>vīta</i></span>"}]}]}"##,
        )
        .unwrap();
        assert_eq!(
            first_sense(&form, Lang::Latin, false).as_deref(),
            Some("genitive singular of vīta")
        );

        assert_eq!(
            plain_text("<style data-x=\"1\">.a{b:c}</style>salt &amp; pepper&nbsp;<b>mix</b>"),
            "salt & pepper mix"
        );
        assert_eq!(plain_text("a <b broken"), "a");
        let long = "parrot, member of the order Psittaciformes, especially of the families \
                    Psittacidae and Psittaculidae";
        assert_eq!(
            concise(long),
            "parrot, member of the order Psittaciformes, especially of…"
        );
        assert!(concise(long).chars().count() <= MEANING_LENGTH + 1);
    }
}
