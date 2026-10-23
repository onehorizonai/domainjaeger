//! The hunt: walk the dictionary one TLD at a time, turn words into domain
//! hacks and verify them until enough available names are found.
//!
//! A hunt inside a category works the other way around: it walks the words
//! of the category's topics and tries every TLD on each of them. So does a
//! hunt for whole words (lening.nl), which walks the best known words.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use rand::seq::SliceRandom;
use serde::{Deserialize, Serialize};
use tokio::sync::watch;
use tokio::task::JoinSet;

use crate::cache::{Cache, DOMAIN_TTL_SECS, Record};
use crate::check::{Checker, Method, Status, Verdict};
use crate::hacks::{
    Affix, Candidate, Kind, Typos, Variant, candidate, candidates_for_word, combination, variants,
    whole_candidates, with_missing,
};
use crate::lang::Lang;
use crate::tld::spotlight;
use crate::topics::{self, Category};
use crate::wiktionary::{SLICE_REQUESTS, Wiktionary, Word, WordList};

/// Lookups in flight at the same time (they go to different registries).
const CONCURRENCY: usize = 8;
/// Dictionary requests a TLD may spend per turn before the next one is up.
const FETCHES_PER_TURN: usize = 2;
/// Unclear answers in a row before a TLD is left out of the hunt.
const UNKNOWN_STREAK_LIMIT: u32 = 3;
/// Other TLDs that get a turn between two rounds of the spotlight TLDs.
const GUESTS_PER_CYCLE: usize = 6;
/// How many turns ahead registries are looked up in the background.
const LOOK_AHEAD: usize = 16;
/// Longest wait for the dictionary's rate limit before trying another TLD.
const PATIENCE: Duration = Duration::from_secs(1);
/// Longest queue at a registry before its TLD has to skip a turn.
const REGISTRY_PATIENCE: Duration = Duration::from_secs(3);
/// A hunt for whole words starts with the words every Dutch person knows and
/// works its way down to the ones this percentage knows.
const LEAST_KNOWN: u8 = 80;
/// Among all typos, the share of the words that also get a name with a
/// letter missing. Nearly every word has such a name and nearly all of them
/// are free, so with one for every word a round would be little else.
const MISSING_ONE_IN: usize = 10;

#[derive(Debug, Clone)]
pub struct Settings {
    /// Available names to find per round.
    pub count: usize,
    /// Give up on a round after this many lookups.
    pub max_checks: usize,
    pub min_label: usize,
    /// The small typos to make names with: all, none or one kind.
    pub typos: Typos,
    /// Only hunt among the words of this category.
    pub category: Option<Category>,
    /// The language the words come from.
    pub lang: Lang,
    /// Hunt for whole words (lening.nl) and their small typos instead of
    /// names where the TLD is the end of the word.
    pub whole: bool,
    /// Also words the dictionary flags as vulgar, offensive, sexual or
    /// violent (see `tone`). They are marked; without this they are skipped.
    pub rude: bool,
    /// A fixed part before or after every whole word: headspeed, speedbox.
    pub affix: Affix,
}

impl Settings {
    /// Is this word one to make names of?
    fn welcomes(&self, word: &Word) -> bool {
        self.rude || word.tone.is_empty()
    }

    /// Does the word in this place on a page of the dictionary also get a
    /// name with a letter missing (nmbe.rs)? Every word when that is the
    /// typo that was picked, one in `MISSING_ONE_IN` among all typos.
    fn spares_a_letter(&self, place: usize) -> bool {
        match self.typos {
            Typos::All => place.is_multiple_of(MISSING_ONE_IN),
            typos => typos.allows(Kind::Missing),
        }
    }

    /// The whole names a word makes under a TLD: the word and its typos, or
    /// the word with the fixed part.
    fn whole_names(&self, word: &Word, tld: &str) -> Vec<Candidate> {
        if self.affix.is_empty() {
            whole_candidates(word, tld, self.typos, self.min_label)
        } else {
            combination(word, tld, &self.affix, self.min_label)
                .into_iter()
                .collect()
        }
    }
}

/// A name with its verdict and, when it is available and the dictionary was
/// asked, what its word means.
type Checked = (Candidate, Verdict, Option<String>);

/// What `check_word` found out about a word.
pub struct Looked {
    /// The dictionary of the language knows the word.
    pub known: bool,
    /// What it means, in a few words of English.
    pub meaning: Option<String>,
    /// Why it may not be for everyone ("vulgar, sexual"), empty for most.
    pub tone: String,
    /// The verdict on every name the word allows.
    pub records: Vec<Record>,
    /// No extension fits the end of the word, so the names are the whole
    /// word before the dot.
    pub whole_instead: bool,
}

pub enum Event {
    /// What the hunter is doing right now.
    Busy(String),
    /// A domain got its verdict.
    Checked(Box<Record>),
    /// A TLD dropped out of the hunt.
    Skipped { tld: String, reason: String },
}

#[derive(Debug, Default)]
pub struct Round {
    pub hits: Vec<Record>,
    pub checked: usize,
    pub taken: usize,
    pub unknown: usize,
    /// Stopped with ctrl-c.
    pub interrupted: bool,
    /// The dictionary has nothing left for the selected TLDs.
    pub exhausted: bool,
    /// Every TLD was left out along the way, so nothing could be checked.
    pub stuck: bool,
    /// Some of them were left out for unclear answers: the next round asks
    /// their registries again.
    pub retry: bool,
    /// Hit the lookup budget before finding enough names.
    pub capped: bool,
}

/// One word ending being paged through, e.g. "-ap" for .app.
struct Stream {
    variant: Variant,
    /// Longer endings that are covered by the same dictionary range: every
    /// word on -aai also ends in -ai, so it needs no request of its own.
    riders: Vec<Variant>,
    /// Position of the page that is currently queued (what we persist).
    cursor: Option<String>,
    /// Position after the queued page.
    after: Option<String>,
    done: bool,
    queue: VecDeque<Candidate>,
}

/// Everything we know about one TLD.
struct Lane {
    tld: String,
    streams: Vec<Stream>,
    turn: usize,
    prepared: bool,
    /// Its registry lookup is being resolved in the background.
    warming: bool,
    benched: bool,
    /// Benched for unclear answers, not for a registry that cannot be asked.
    unclear: bool,
    unknown_streak: u32,
}

impl Lane {
    fn has_queued(&self) -> bool {
        self.streams.iter().any(|s| !s.queue.is_empty())
    }

    fn alive(&self) -> bool {
        !self.benched
            && (!self.prepared || self.streams.iter().any(|s| !s.done || !s.queue.is_empty()))
    }
}

/// The words of one list fetched so far. Kept in the cache between
/// sessions, so a list is downloaded once every four weeks at most.
#[derive(Default, Serialize, Deserialize)]
struct Shelf {
    words: Vec<Word>,
    /// Position of the next slice of the list.
    next: Option<String>,
    complete: bool,
}

/// One list of words being hunted in: a topic of the category, or the words
/// a share of the Dutch know.
struct Feed {
    list: WordList,
    /// Where its shelf lives in the cache.
    key: String,
    shelf: Shelf,
}

/// The state of a hunt that starts from lists of words: inside a category,
/// or for whole words.
struct Topical {
    feeds: Vec<Feed>,
    /// Read the lists one after the other (best known words first) instead
    /// of taking turns.
    in_order: bool,
    turn: usize,
    /// The shelves have been taken out of the cache.
    loaded: bool,
    /// Every word ending that makes a hack, with the lane of its TLD.
    endings: Vec<(usize, Variant)>,
    /// Names made of the words fetched so far, best ones first.
    queue: VecDeque<(usize, Candidate)>,
}

pub struct Hunter {
    wiki: Arc<Wiktionary>,
    checker: Arc<Checker>,
    cache: Arc<Cache>,
    pub settings: Settings,
    lanes: Vec<Lane>,
    /// Order in which TLDs get their turn.
    deck: Vec<usize>,
    position: usize,
    /// Whose turn it is among the TLDs with queued names.
    queued_turn: usize,
    /// Domains already handed out this session.
    seen: HashSet<String>,
    /// Available names found beyond what a round asked for.
    carry: VecDeque<Record>,
    /// Set when the hunt is limited to a category.
    topical: Option<Topical>,
    /// The user picked the TLDs (as opposed to "all of them").
    focused: bool,
}

impl Hunter {
    /// `focused` means the user picked the TLDs, so they all take equal turns.
    pub fn new(
        wiki: Arc<Wiktionary>,
        checker: Arc<Checker>,
        cache: Arc<Cache>,
        tlds: Vec<String>,
        focused: bool,
        settings: Settings,
    ) -> Self {
        let (mut stars, mut guests): (Vec<String>, Vec<String>) = if focused {
            (tlds, Vec::new())
        } else {
            tlds.into_iter()
                .partition(|t| spotlight(settings.lang).contains(&t.as_str()))
        };
        if !focused {
            stars.sort_by_key(|t| spotlight(settings.lang).iter().position(|s| s == t));
            guests.shuffle(&mut rand::rng());
        }
        let star_count = stars.len();
        let lanes: Vec<Lane> = stars
            .into_iter()
            .chain(guests)
            .map(|tld| Lane {
                streams: streams_for(&tld, settings.typos),
                tld,
                turn: 0,
                prepared: false,
                warming: false,
                benched: false,
                unclear: false,
                unknown_streak: 0,
            })
            .collect();

        let star_turns: Vec<usize> = (0..star_count).collect();
        let guest_turns: Vec<usize> = (star_count..lanes.len()).collect();
        let deck = if guest_turns.is_empty() {
            star_turns
        } else {
            guest_turns
                .chunks(GUESTS_PER_CYCLE)
                .flat_map(|guests| star_turns.iter().chain(guests).copied())
                .collect()
        };
        // A category has its own lists of words. Without one, whole words
        // come from the best known words of the language.
        let lists: Vec<WordList> = match (&settings.category, settings.whole, settings.lang) {
            (Some(category), _, _) => category
                .topics
                .iter()
                .map(|topic| match topic.as_str() {
                    topics::NAMES => WordList::Names,
                    _ => WordList::About(topic.clone()),
                })
                .collect(),
            (None, true, Lang::Dutch) => (LEAST_KNOWN..=100).rev().map(WordList::KnownBy).collect(),
            (None, true, _) => vec![WordList::Popular],
            (None, false, _) => Vec::new(),
        };
        // Which words a list has depends on the syllables asked for.
        let syllables: Vec<String> = wiki.syllables().iter().map(u8::to_string).collect();
        let (scope, syllables) = (scope(settings.lang), syllables.join("-"));
        let topical = (!lists.is_empty()).then(|| Topical {
            feeds: lists
                .into_iter()
                .map(|list| Feed {
                    // The numbers are for shelves that know the tone of their
                    // words, that take in the topics under a topic, and for
                    // the list of everyday words.
                    key: match &list {
                        WordList::About(topic) => format!("topic3:{scope}{syllables}:{topic}"),
                        WordList::KnownBy(share) => format!("known2:{syllables}:{share}"),
                        WordList::Popular => format!("popular3:{scope}{syllables}"),
                        WordList::Names => {
                            format!("names:{}:{syllables}", settings.lang.code())
                        }
                    },
                    list,
                    shelf: Shelf::default(),
                })
                .collect(),
            in_order: settings.category.is_none(),
            turn: 0,
            loaded: false,
            endings: lanes
                .iter()
                .enumerate()
                .flat_map(|(i, lane)| {
                    variants(&lane.tld, settings.typos)
                        .into_iter()
                        .map(move |variant| (i, variant))
                })
                .collect(),
            queue: VecDeque::new(),
        });
        Self {
            wiki,
            checker,
            cache,
            settings,
            lanes,
            deck,
            position: 0,
            queued_turn: 0,
            seen: HashSet::new(),
            carry: VecDeque::new(),
            topical,
            focused,
        }
    }

    /// The TLDs of the hunt, also the ones that were left out along the
    /// way: a new hunt gives them another go.
    pub fn tlds(&self) -> Vec<String> {
        self.lanes.iter().map(|l| l.tld.clone()).collect()
    }

    /// Find `settings.count` available names (or run out of budget trying).
    pub async fn round(&mut self, on_event: &mut impl FnMut(Event)) -> Result<Round> {
        let mut round = Round::default();
        // A registry that gave unclear answers may answer again by now. When
        // no TLD is left, those get another go.
        if self.lanes.iter().all(|l| l.benched) {
            for lane in self.lanes.iter_mut().filter(|l| l.unclear) {
                (lane.benched, lane.unclear, lane.unknown_streak) = (false, false, 0);
            }
        }
        while round.hits.len() < self.settings.count {
            let Some(mut record) = self.carry.pop_front() else {
                break;
            };
            // It was found while the last round was wrapping up.
            if record.meaning.is_none() {
                let lang = Lang::from_code(&record.lang);
                record.meaning = meaning_of(&self.wiki, &self.cache, lang, &record.word).await;
                self.cache.put(&record)?;
            }
            on_event(Event::Checked(Box::new(record.clone())));
            round.checked += 1;
            round.hits.push(record);
        }

        let mut tasks: JoinSet<Checked> = JoinSet::new();
        let mut ctrl_c = Box::pin(tokio::signal::ctrl_c());
        // Once the round is over, nobody waits for what a word means.
        let (wrap_up, _) = watch::channel(false);
        let mut launched = 0;
        // A dictionary failure ends the round, but only after the lookups
        // that are already running have been recorded.
        let mut failure = None;
        loop {
            while let Some(done) = tasks.try_join_next() {
                self.settle(done?, &mut round, on_event)?;
            }
            if round.hits.len() >= self.settings.count
                || round.interrupted
                || round.exhausted
                || round.stuck
                || failure.is_some()
            {
                break;
            }
            if launched >= self.settings.max_checks {
                round.capped = true;
                break;
            }
            // Near the finish fewer lookups run at once, to not overshoot by much.
            let missing = self.settings.count - round.hits.len();
            if tasks.len() >= (2 * missing).min(CONCURRENCY) {
                tokio::select! {
                    _ = &mut ctrl_c => round.interrupted = true,
                    Some(done) = tasks.join_next() => self.settle(done?, &mut round, on_event)?,
                }
                continue;
            }
            tokio::select! {
                _ = &mut ctrl_c => round.interrupted = true,
                next = self.next_candidate(on_event) => match next {
                    Err(e) => failure = Some(e),
                    Ok(None) if self.lanes.iter().all(|l| l.benched) => {
                        round.stuck = true;
                        round.retry = self.lanes.iter().any(|l| l.unclear);
                    }
                    Ok(None) => round.exhausted = true,
                    Ok(Some(candidate)) => {
                        launched += 1;
                        on_event(Event::Busy(format!("checking {}", candidate.domain)));
                        let (checker, wiki) = (self.checker.clone(), self.wiki.clone());
                        let (cache, lang) = (self.cache.clone(), self.settings.lang);
                        let mut over = wrap_up.subscribe();
                        tasks.spawn(async move {
                            let verdict = checker.check(&candidate.domain, &candidate.tld).await;
                            let meaning = match verdict.status {
                                Status::Available => tokio::select! {
                                    known = meaning_of(&wiki, &cache, lang, &candidate.word) => known,
                                    _ = over.wait_for(|over| *over) => None,
                                },
                                _ => None,
                            };
                            (candidate, verdict, meaning)
                        });
                    }
                },
            }
        }
        // Whatever is still in flight gets its verdict, nothing is dropped.
        wrap_up.send_replace(true);
        if !tasks.is_empty() {
            on_event(Event::Busy("waiting for the last answers".into()));
        }
        while let Some(done) = tasks.join_next().await {
            self.settle(done?, &mut round, on_event)?;
        }
        match failure {
            Some(e) => Err(e),
            None => Ok(round),
        }
    }

    /// Verify records again, e.g. the ones whose four weeks are up.
    pub async fn recheck(
        &self,
        records: Vec<Record>,
        on_event: &mut impl FnMut(Event),
    ) -> Result<Round> {
        let mut round = Round::default();
        let mut tasks: JoinSet<Record> = JoinSet::new();
        let mut ctrl_c = Box::pin(tokio::signal::ctrl_c());
        let mut queue = records.into_iter();
        loop {
            while !round.interrupted && tasks.len() < CONCURRENCY {
                let Some(mut record) = queue.next() else {
                    break;
                };
                let (checker, cache) = (self.checker.clone(), self.cache.clone());
                tasks.spawn(async move {
                    let verdict = checker.check(&record.domain, &record.tld).await;
                    record.status = verdict.status;
                    record.source = verdict.source.to_string();
                    record.detail = verdict.detail;
                    record.checked_at = cache.now();
                    record
                });
            }
            tokio::select! {
                _ = &mut ctrl_c, if !round.interrupted => round.interrupted = true,
                done = tasks.join_next() => {
                    let Some(done) = done else { break };
                    let record = done?;
                    // A registry we cannot reach must not erase an earlier verdict.
                    if record.source != "none" {
                        self.cache.put(&record)?;
                    }
                    tally(&mut round, &record);
                    if record.status == Status::Available {
                        round.hits.push(record.clone());
                    }
                    on_event(Event::Checked(Box::new(record)));
                }
            }
        }
        Ok(round)
    }

    /// Every hack a single word allows in the active TLDs, verified, with
    /// what the dictionary knows about the word.
    pub async fn check_word(&self, text: &str, on_event: &mut impl FnMut(Event)) -> Result<Looked> {
        let lang = self.settings.lang;
        on_event(Event::Busy(format!(
            "looking up \"{text}\" in {}",
            lang.dictionary()
        )));
        let known = self.wiki.lookup(lang, text).await?;
        let in_dictionary = known.is_some();
        let meaning = meaning_of(&self.wiki, &self.cache, lang, text).await;
        let tone = known.as_ref().map(Word::tone_list).unwrap_or_default();
        let word = known.unwrap_or(Word {
            text: text.to_string(),
            ..Word::default()
        });
        let (typos, min_label) = (self.settings.typos, self.settings.min_label);
        let whole_names = || -> Vec<Candidate> {
            self.tlds()
                .iter()
                .flat_map(|tld| self.settings.whole_names(&word, tld))
                .collect()
        };
        let mut candidates = if self.settings.whole {
            whole_names()
        } else {
            candidates_for_word(&word, &self.tlds(), typos, min_label)
        };
        // Hardly any word ends in com or nl. Rather than check nothing, the
        // whole word goes before the dot, as in a hunt that runs dry. Not in
        // every extension at once, though: that is hundreds of names.
        let whole_instead = candidates.is_empty() && !self.settings.whole && self.focused;
        if whole_instead {
            candidates = whole_names();
        }

        let mut records = Vec::new();
        let mut tasks: JoinSet<(Candidate, Verdict)> = JoinSet::new();
        for candidate in candidates {
            if let Some(mut record) = self.cache.get_fresh(&candidate.domain)? {
                record.meaning = record.meaning.or_else(|| meaning.clone());
                on_event(Event::Checked(Box::new(record.clone())));
                records.push(record);
                continue;
            }
            let checker = self.checker.clone();
            tasks.spawn(async move {
                let verdict = checker.check(&candidate.domain, &candidate.tld).await;
                (candidate, verdict)
            });
        }
        while let Some(done) = tasks.join_next().await {
            let (candidate, verdict) = done?;
            // TLDs we cannot verify are simply left out.
            if verdict.source == "none" {
                continue;
            }
            let record = to_record(
                &candidate,
                &verdict,
                lang,
                meaning.clone(),
                self.cache.now(),
            );
            self.cache.put(&record)?;
            on_event(Event::Checked(Box::new(record.clone())));
            records.push(record);
        }
        // Names of this word that were verified earlier get the meaning too.
        if let Some(meaning) = &meaning {
            self.cache.explain(lang.code(), text, meaning)?;
        }
        Ok(Looked {
            known: in_dictionary,
            meaning: meaning.filter(|m| !m.is_empty()),
            tone,
            whole_instead: whole_instead && !records.is_empty(),
            records,
        })
    }

    fn settle(
        &mut self,
        (candidate, verdict, meaning): Checked,
        round: &mut Round,
        on_event: &mut impl FnMut(Event),
    ) -> Result<()> {
        let record = to_record(
            &candidate,
            &verdict,
            self.settings.lang,
            meaning,
            self.cache.now(),
        );
        self.cache.put(&record)?;
        tally(round, &record);

        if let Some(lane) = self.lanes.iter_mut().find(|l| l.tld == candidate.tld) {
            if record.status == Status::Unknown {
                lane.unknown_streak += 1;
                if lane.unknown_streak >= UNKNOWN_STREAK_LIMIT && !lane.benched {
                    (lane.benched, lane.unclear) = (true, true);
                    on_event(Event::Skipped {
                        tld: lane.tld.clone(),
                        reason: format!("left out for now, {}", record.detail),
                    });
                }
            } else {
                lane.unknown_streak = 0;
            }
        }

        if record.status == Status::Available {
            if round.hits.len() >= self.settings.count {
                // Found more than asked for: keep it for the next round.
                self.carry.push_back(record);
                return Ok(());
            }
            round.hits.push(record.clone());
        }
        on_event(Event::Checked(Box::new(record)));
        Ok(())
    }

    /// Resolve the registry lookups of the TLDs that are up next in the
    /// background, so the hunt rarely has to wait for one.
    fn warm_up(&mut self) {
        for offset in 0..self.deck.len().min(LOOK_AHEAD) {
            let lane = &mut self.lanes[self.deck[(self.position + offset) % self.deck.len()]];
            if lane.prepared || lane.warming {
                continue;
            }
            lane.warming = true;
            let (checker, tld) = (self.checker.clone(), lane.tld.clone());
            tokio::spawn(async move {
                checker.method(&tld).await;
            });
        }
    }

    async fn next_candidate(
        &mut self,
        on_event: &mut impl FnMut(Event),
    ) -> Result<Option<Candidate>> {
        if self.topical.is_some() {
            return self.next_in_category(on_event).await;
        }
        // TLDs whose registry has a queue are passed over, for at most one
        // lap: when everybody is busy we simply wait our turn.
        let mut passed = 0;
        // Every turn of a live lane either yields a name or moves a stream
        // forward, so this ends once the dictionary is exhausted.
        while self.lanes.iter().any(Lane::alive) {
            // Dictionary requests are rationed. While we are out of them, live
            // off the names that are already queued instead of standing still.
            if self.wiki.wait_time() > PATIENCE {
                if let Some(lane) = self.next_queued_lane() {
                    if let Some(candidate) = self.pull(lane, false, on_event).await? {
                        return Ok(Some(candidate));
                    }
                    continue;
                }
                // Names are waiting, but their registries all have a queue.
                if self.lanes.iter().any(|l| !l.benched && l.has_queued()) {
                    on_event(Event::Busy("pausing a moment, not to ask too fast".into()));
                    tokio::time::sleep(Duration::from_millis(500)).await;
                    continue;
                }
            }
            self.warm_up();
            let lane = self.deck[self.position % self.deck.len()];
            self.position += 1;
            if passed < self.deck.len() && self.registry_is_busy(lane) {
                passed += 1;
                continue;
            }
            if let Some(candidate) = self.pull(lane, true, on_event).await? {
                return Ok(Some(candidate));
            }
        }
        Ok(None)
    }

    /// The next name of a hunt that starts from lists of words (a category,
    /// whole words). The words are matched against every TLD at once, and
    /// the best name whose registry has time for us goes first.
    async fn next_in_category(
        &mut self,
        on_event: &mut impl FnMut(Event),
    ) -> Result<Option<Candidate>> {
        // All state stays where it is, so that dropping this future (ctrl-c)
        // loses nothing.
        let Self {
            wiki,
            checker,
            cache,
            settings,
            lanes,
            seen,
            topical,
            ..
        } = self;
        let Some(topical) = topical.as_mut() else {
            return Ok(None);
        };
        if !topical.loaded {
            for feed in &mut topical.feeds {
                let shelf = cache.kv_get(&feed.key, DOMAIN_TTL_SECS)?;
                if let Some(shelf) = shelf.and_then(|json| serde_json::from_str(&json).ok()) {
                    feed.shelf = shelf;
                }
                let names = cross(&feed.shelf.words, &topical.endings, lanes, settings);
                topical.queue.extend(names);
            }
            topical
                .queue
                .make_contiguous()
                .sort_by(|a, b| best_first(&a.1, &b.1));
            topical.loaded = true;
        }

        loop {
            // Without a TLD to check in, more words would be fetched for nothing.
            if lanes.iter().all(|l| l.benched) {
                return Ok(None);
            }
            // Registry lookups are resolved in the background, a few TLDs at
            // a time, in the order their names are waiting.
            let mut resolving = 0;
            for lane in lanes.iter_mut().filter(|l| l.warming && !l.prepared) {
                match checker.known_method(&lane.tld) {
                    Some(method) => adopt(lane, &method, on_event),
                    None => resolving += 1,
                }
            }
            for (lane, _) in &topical.queue {
                if resolving == LOOK_AHEAD {
                    break;
                }
                let lane = &mut lanes[*lane];
                if lane.prepared || lane.warming {
                    continue;
                }
                lane.warming = true;
                resolving += 1;
                let (checker, tld) = (checker.clone(), lane.tld.clone());
                tokio::spawn(async move {
                    checker.method(&tld).await;
                });
            }

            let feeds = topical.feeds.len();
            let first = if topical.in_order { 0 } else { topical.turn };
            let unread = (0..feeds)
                .map(|k| (first + k) % feeds)
                .find(|&f| !topical.feeds[f].shelf.complete);
            // Fetching ahead is fine as long as it cannot get stuck halfway
            // on the dictionary's rate limit while names are waiting.
            let more_words = unread.is_some() && wiki.spare_requests() >= SLICE_REQUESTS;

            // TLDs whose registry is not known yet, or has a queue, have to wait.
            let waiting: Vec<bool> = lanes
                .iter()
                .map(|l| !l.prepared || checker.registry_wait(&l.tld) > REGISTRY_PATIENCE)
                .collect();
            let mut blocked = false;
            let mut i = 0;
            while let Some((lane, name)) = topical.queue.get(i) {
                if lanes[*lane].benched {
                    topical.queue.remove(i);
                } else if waiting[*lane] {
                    blocked = true;
                    i += 1;
                } else if name.rank > 1 && more_words {
                    // Only the lesser typos are left: new words come first.
                    blocked = true;
                    break;
                } else if let Some((_, candidate)) = topical.queue.remove(i) {
                    let is_new = seen.insert(candidate.domain.clone())
                        && cache.get_fresh(&candidate.domain)?.is_none();
                    if is_new {
                        return Ok(Some(candidate));
                    }
                }
            }

            // Nothing can go out right now. Fetch more words when there are
            // any (and the dictionary has time for us), otherwise wait.
            let Some(f) = unread.filter(|_| !blocked || more_words) else {
                if !blocked {
                    return Ok(None);
                }
                on_event(Event::Busy(if resolving > 0 {
                    "finding out how to check the next extensions".into()
                } else {
                    "pausing a moment, not to ask too fast".into()
                }));
                tokio::time::sleep(Duration::from_millis(150)).await;
                continue;
            };
            let feed = &mut topical.feeds[f];
            let wait = wiki.wait_time();
            let dictionary = settings.lang.dictionary();
            let wanted = feed.list.describe();
            on_event(Event::Busy(if wait > PATIENCE {
                format!(
                    "{dictionary} answers {} questions a minute, next one in {}s ({wanted})",
                    wiki.allowed_per_minute(),
                    wait.as_secs() + 1
                )
            } else {
                format!("asking {dictionary} for {wanted}")
            }));
            let page = wiki
                .words_of(settings.lang, &feed.list, feed.shelf.next.as_deref())
                .await?;
            topical.turn = f + 1;
            topical
                .queue
                .extend(cross(&page.words, &topical.endings, lanes, settings));
            topical
                .queue
                .make_contiguous()
                .sort_by(|a, b| best_first(&a.1, &b.1));
            feed.shelf.words.extend(page.words);
            feed.shelf.complete = page.next.is_none();
            feed.shelf.next = page.next;
            cache.kv_put(&feed.key, &serde_json::to_string(&feed.shelf)?)?;
        }
    }

    fn registry_is_busy(&self, lane: usize) -> bool {
        let lane = &self.lanes[lane];
        lane.prepared && self.checker.registry_wait(&lane.tld) > REGISTRY_PATIENCE
    }

    /// Round-robin over the TLDs that have names waiting and a registry with
    /// time for us.
    fn next_queued_lane(&mut self) -> Option<usize> {
        let lanes = self.lanes.len();
        let lane = (0..lanes)
            .map(|k| (self.queued_turn + k) % lanes)
            .find(|&i| {
                !self.lanes[i].benched && self.lanes[i].has_queued() && !self.registry_is_busy(i)
            })?;
        self.queued_turn = lane + 1;
        Some(lane)
    }

    /// One turn for a TLD: hand out a queued name, or fetch more words.
    async fn pull(
        &mut self,
        lane: usize,
        may_fetch: bool,
        on_event: &mut impl FnMut(Event),
    ) -> Result<Option<Candidate>> {
        if self.lanes[lane].benched {
            return Ok(None);
        }
        if !self.lanes[lane].prepared {
            self.prepare(lane, on_event).await?;
            if self.lanes[lane].benched {
                return Ok(None);
            }
        }
        let mut fetches = 0;
        loop {
            while let Some(candidate) = self.pop_queued(lane)? {
                let is_new = self.seen.insert(candidate.domain.clone())
                    && self.cache.get_fresh(&candidate.domain)?.is_none();
                if is_new {
                    return Ok(Some(candidate));
                }
            }
            let streams = self.lanes[lane].streams.len();
            let turn = self.lanes[lane].turn;
            let next = (0..streams)
                .map(|k| (turn + k) % streams)
                .find(|&s| !self.lanes[lane].streams[s].done);
            let Some(s) = next else { return Ok(None) };
            if !may_fetch || fetches == FETCHES_PER_TURN {
                return Ok(None);
            }
            fetches += 1;

            let suffix = self.lanes[lane].streams[s].variant.suffix.clone();
            let cursor = self.lanes[lane].streams[s].cursor.clone();
            let wait = self.wiki.wait_time();
            let lang = self.settings.lang;
            let dictionary = lang.dictionary();
            on_event(Event::Busy(if wait > PATIENCE {
                format!(
                    "{dictionary} answers {} questions a minute, next one in {}s (words ending in -{suffix})",
                    self.wiki.allowed_per_minute(),
                    wait.as_secs() + 1
                )
            } else {
                format!(
                    "asking {dictionary} for words ending in -{suffix} (.{})",
                    self.lanes[lane].tld
                )
            }));
            let page = self
                .wiki
                .words_ending_in(lang, &suffix, cursor.as_deref())
                .await?;

            let min_label = self.settings.min_label;
            let Lane { tld, streams, .. } = &mut self.lanes[lane];
            let (tld, stream) = (tld.as_str(), &mut streams[s]);
            let settings = &self.settings;
            stream.queue = page
                .words
                .iter()
                .filter(|word| settings.welcomes(word))
                .enumerate()
                .flat_map(|(place, word)| {
                    let missing = settings.spares_a_letter(place);
                    std::iter::once(&stream.variant)
                        .chain(&stream.riders)
                        .filter_map(move |variant| candidate(word, tld, variant, min_label))
                        .flat_map(move |name| with_missing(name, missing, min_label))
                })
                .collect();
            stream.after = page.next;
            if stream.queue.is_empty() {
                self.commit(lane, s)?;
            }
        }
    }

    /// Next queued name of a lane, rotating over its word endings.
    fn pop_queued(&mut self, lane: usize) -> Result<Option<Candidate>> {
        let streams = self.lanes[lane].streams.len();
        for k in 0..streams {
            let s = (self.lanes[lane].turn + k) % streams;
            if let Some(candidate) = self.lanes[lane].streams[s].queue.pop_front() {
                self.lanes[lane].turn = (s + 1) % streams;
                if self.lanes[lane].streams[s].queue.is_empty() {
                    self.commit(lane, s)?;
                }
                return Ok(Some(candidate));
            }
        }
        Ok(None)
    }

    /// The queued page of a stream is used up: move on and remember where we are.
    fn commit(&mut self, lane: usize, s: usize) -> Result<()> {
        let Lane { tld, streams, .. } = &mut self.lanes[lane];
        let stream = &mut streams[s];
        stream.cursor = stream.after.take();
        stream.done = stream.cursor.is_none();
        self.cache.set_stream_state(
            &format!("{}{tld}", scope(self.settings.lang)),
            &stream.variant.suffix,
            stream.cursor.as_deref(),
            stream.done,
        )
    }

    /// First visit of a TLD: make sure it can be verified, then pick up the
    /// dictionary where an earlier session left off.
    async fn prepare(&mut self, lane: usize, on_event: &mut impl FnMut(Event)) -> Result<()> {
        let tld = self.lanes[lane].tld.clone();
        on_event(Event::Busy(format!(
            "finding out how to check .{tld} names"
        )));
        let method = self.checker.method(&tld).await;
        let lane = &mut self.lanes[lane];
        adopt(lane, &method, on_event);
        if lane.benched {
            return Ok(());
        }
        // Every language has its own position in its own dictionary.
        let place = format!("{}{tld}", scope(self.settings.lang));
        for stream in &mut lane.streams {
            (stream.cursor, stream.done) =
                self.cache.stream_state(&place, &stream.variant.suffix)?;
        }
        Ok(())
    }
}

/// Prefix that keeps what the cache remembers about a dictionary apart per
/// language. Dutch came first and has none.
fn scope(lang: Lang) -> &'static str {
    match lang {
        Lang::Dutch => "",
        Lang::English => "en:",
        Lang::Latin => "la:",
        Lang::Spanish => "es:",
    }
}

/// The registry lookup of a TLD is known: it joins the hunt or drops out.
fn adopt(lane: &mut Lane, method: &Method, on_event: &mut impl FnMut(Event)) {
    lane.prepared = true;
    if !method.is_usable() {
        lane.benched = true;
        on_event(Event::Skipped {
            tld: lane.tld.clone(),
            reason: method.describe(),
        });
    }
}

/// Every name the words allow in the TLDs of the hunt: hacks, or the whole
/// words and their typos.
fn cross(
    words: &[Word],
    endings: &[(usize, Variant)],
    lanes: &[Lane],
    settings: &Settings,
) -> Vec<(usize, Candidate)> {
    let min_label = settings.min_label;
    // A name with a letter missing comes last (see `Kind::rank`), so here
    // every word can have one.
    let missing = settings.typos.allows(Kind::Missing);
    let words = words.iter().filter(|word| settings.welcomes(word));
    if settings.whole {
        return words
            .flat_map(|word| {
                lanes.iter().enumerate().flat_map(move |(i, lane)| {
                    settings
                        .whole_names(word, &lane.tld)
                        .into_iter()
                        .map(move |name| (i, name))
                })
            })
            .collect();
    }
    words
        .flat_map(|word| {
            endings
                .iter()
                .filter_map(move |(lane, variant)| {
                    let name = candidate(word, &lanes[*lane].tld, variant, min_label)?;
                    Some((*lane, name))
                })
                .flat_map(move |(lane, name)| {
                    with_missing(name, missing, min_label).map(move |name| (lane, name))
                })
        })
        .collect()
}

/// The order names from a list of words are tried in: the best name of
/// every word first (see `Candidate::rank`), well known words first. Where
/// that is all the same, the order of the dictionary stays (sorting is
/// stable), which for English is best known first as well.
fn best_first(a: &Candidate, b: &Candidate) -> Ordering {
    a.rank.cmp(&b.rank).then(b.prevalence.cmp(&a.prevalence))
}

/// The dictionary ranges to page through for a TLD.
fn streams_for(tld: &str, typos: Typos) -> Vec<Stream> {
    let mut streams: Vec<Stream> = Vec::new();
    for variant in variants(tld, typos) {
        // A longer ending rides along with the stream whose range contains it.
        match streams
            .iter_mut()
            .find(|s| variant.suffix.ends_with(&s.variant.suffix))
        {
            Some(host) => host.riders.push(variant),
            None => streams.push(Stream {
                variant,
                riders: Vec::new(),
                cursor: None,
                after: None,
                done: false,
                queue: VecDeque::new(),
            }),
        }
    }
    streams
}

fn tally(round: &mut Round, record: &Record) {
    round.checked += 1;
    match record.status {
        Status::Taken => round.taken += 1,
        Status::Unknown => round.unknown += 1,
        Status::Available => {}
    }
}

/// What a word means, for a name that turned out to be available. The
/// dictionary is asked once per word. `None` when it did not answer, empty
/// when it has nothing to say.
async fn meaning_of(wiki: &Wiktionary, cache: &Cache, lang: Lang, word: &str) -> Option<String> {
    if let Ok(Some(known)) = cache.meaning_of(lang.code(), word) {
        return Some(known);
    }
    let meaning = wiki.meaning(lang, word).await.ok()?;
    Some(meaning.unwrap_or_default())
}

fn to_record(
    candidate: &Candidate,
    verdict: &Verdict,
    lang: Lang,
    meaning: Option<String>,
    checked_at: i64,
) -> Record {
    Record {
        domain: candidate.domain.clone(),
        word: candidate.word.clone(),
        label: candidate.label.clone(),
        tld: candidate.tld.clone(),
        kind: candidate.kind.as_str().to_string(),
        spelled: candidate.spelled(),
        syllables: candidate.syllables,
        prevalence: candidate.prevalence,
        official: candidate.official,
        topics: Some(candidate.topics.clone()),
        tone: Some(candidate.tone.clone()),
        lang: lang.code().to_string(),
        whole: candidate.whole,
        meaning,
        status: verdict.status,
        source: verdict.source.to_string(),
        detail: verdict.detail.clone(),
        checked_at,
    }
}

/// Per-TLD tallies for the stats screen.
pub fn by_tld(records: &[Record]) -> Vec<(String, usize, usize)> {
    let mut map: HashMap<&str, (usize, usize)> = HashMap::new();
    for r in records {
        let entry = map.entry(r.tld.as_str()).or_default();
        entry.0 += 1;
        if r.status == Status::Available {
            entry.1 += 1;
        }
    }
    let mut rows: Vec<(String, usize, usize)> = map
        .into_iter()
        .map(|(tld, (checked, free))| (tld.to_string(), checked, free))
        .collect();
    rows.sort_by(|a, b| b.2.cmp(&a.2).then(b.1.cmp(&a.1)).then(a.0.cmp(&b.0)));
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{self, ONE, THREE, TWO, World, answer, page};

    /// Run a round and keep what it reported along the way.
    async fn round(hunter: &mut Hunter) -> (Round, Vec<String>) {
        let mut skipped = Vec::new();
        let mut on_event = |event: Event| {
            if let Event::Skipped { tld, reason } = event {
                skipped.push(format!(".{tld} {reason}"));
            }
        };
        let round = hunter.round(&mut on_event).await.unwrap();
        (round, skipped)
    }

    fn domains(records: &[Record]) -> Vec<&str> {
        records.iter().map(|r| r.domain.as_str()).collect()
    }

    #[tokio::test(start_paused = true)]
    async fn a_round_finds_the_names_that_are_free() {
        let world = World::new();
        world.registry("ing").taken(&["mak.ing"]);
        world.finds("+ing$/", &["cooking", "making", "baking"]);
        world.otherwise_quiet();
        let mut hunter = world.hunter(&["ing"], testing::settings());

        let (found, skipped) = round(&mut hunter).await;
        assert_eq!(domains(&found.hits), vec!["cook.ing", "bak.ing"]);
        assert_eq!((found.checked, found.taken, found.unknown), (3, 1, 0));
        // Fewer than asked for, and the dictionary has no more: that is it.
        assert!(found.exhausted && !found.capped && !found.interrupted);
        assert!(skipped.is_empty());

        // What was found is kept, with what the word is and means.
        let kept = world.cache.get("cook.ing").unwrap().unwrap();
        assert_eq!(
            (kept.word.as_str(), kept.label.as_str(), kept.tld.as_str()),
            ("cooking", "cook", "ing")
        );
        assert_eq!(
            (kept.kind.as_str(), kept.lang.as_str(), kept.whole),
            ("exact", "en", false)
        );
        assert_eq!(kept.syllables, Some(2));
        assert_eq!(kept.checked_at, world.cache.now());
        assert_eq!(
            world.cache.get("mak.ing").unwrap().unwrap().status,
            Status::Taken
        );
        // A taken name is not worth a question to the dictionary.
        assert_eq!(world.web.count("definition/making"), 0);

        // The next round knows all that: nothing is asked twice.
        let asked = world.lookups("");
        let (again, _) = round(&mut hunter).await;
        assert!(again.hits.is_empty() && again.exhausted);
        assert_eq!(world.lookups(""), asked);
    }

    #[tokio::test(start_paused = true)]
    async fn a_round_stops_when_it_has_enough_and_goes_on_from_there() {
        let world = World::new();
        world.registry("ing");
        world.finds("+ing$/", &["cooking", "baking", "making"]);
        world.otherwise_quiet();
        let wanted = Settings {
            count: 2,
            ..testing::settings()
        };
        let mut hunter = world.hunter(&["ing"], wanted);

        let (first, _) = round(&mut hunter).await;
        assert_eq!(domains(&first.hits), vec!["cook.ing", "bak.ing"]);
        let (second, _) = round(&mut hunter).await;
        assert_eq!(domains(&second.hits), vec!["mak.ing"]);
        // Every name was looked up once.
        assert_eq!(world.lookups("domain/"), 3);
    }

    #[tokio::test(start_paused = true)]
    async fn a_round_gives_up_where_everything_is_taken() {
        let world = World::new();
        world.registry("ing");
        let names = ["cooking", "baking", "making", "taking", "waking", "raking"];
        world.finds("+ing$/", &names);
        // The registry calls every name registered.
        world.web.on("rdap.test/ing/domain/", 200, "{}");
        world.otherwise_quiet();
        let patience = Settings {
            max_checks: 4,
            ..testing::settings()
        };
        let (found, _) = round(&mut world.hunter(&["ing"], patience)).await;
        assert!(found.hits.is_empty());
        assert!(found.capped && !found.exhausted);
        assert!(
            found.checked >= 4 && found.checked < names.len(),
            "{}",
            found.checked
        );
    }

    #[tokio::test(start_paused = true)]
    async fn small_typos_reach_extensions_no_word_ends_in() {
        let world = World::new();
        world.registry("app");
        // No word ends in "app", but plenty end in "ap".
        world
            .web
            .on("+ap$/", 200, &answer(vec![page("snap", 1, &[ONE])]));
        world.otherwise_quiet();
        let exact = world
            .hunter(&["app"], testing::settings())
            .round(&mut |_| {})
            .await
            .unwrap();
        assert!(exact.hits.is_empty() && exact.exhausted);

        let with_typos = Settings {
            typos: Typos::All,
            ..testing::settings()
        };
        let (found, _) = round(&mut world.hunter(&["app"], with_typos)).await;
        assert_eq!(domains(&found.hits), vec!["sn.app"]);
        let record = &found.hits[0];
        assert_eq!(
            (record.word.as_str(), record.spelled.as_str()),
            ("snap", "snapp")
        );
        assert_ne!(record.kind, "exact");
    }

    #[tokio::test(start_paused = true)]
    async fn a_letter_goes_missing_for_every_word_or_for_one_in_ten() {
        const WORDS: [&str; 12] = [
            "numbers", "players", "drivers", "makers", "readers", "writers", "singers", "dancers",
            "bakers", "voters", "riders", "lovers",
        ];
        // The names with a letter missing that a hunt finds, and all it finds.
        let hunt = |typos: Typos| async move {
            let world = World::new();
            world.registry("rs");
            world.finds("+rs$/", &WORDS);
            world.otherwise_quiet();
            let settings = Settings {
                typos,
                count: 50,
                ..testing::settings()
            };
            let (found, _) = round(&mut world.hunter(&["rs"], settings)).await;
            let missing: Vec<String> = found
                .hits
                .iter()
                .filter(|r| r.kind == Kind::Missing.as_str())
                .map(|r| r.domain.clone())
                .collect();
            (missing, found.hits.len())
        };

        // Picked on its own, every word has one, next to its real name.
        let (missing, all) = hunt(Typos::Only(Kind::Missing)).await;
        assert_eq!((missing.len(), all), (12, 24));
        assert_eq!(missing[0], "nmbe.rs");
        // Among all typos it is the first word of every ten on a page, so
        // that real words stay the larger part of a round.
        let (missing, all) = hunt(Typos::All).await;
        assert_eq!(missing, vec!["nmbe.rs", "rde.rs"]);
        assert_eq!(all, 14);
        // And not at all without typos, or with another kind.
        assert_eq!(hunt(Typos::Off).await, (Vec::new(), 12));
        assert_eq!(hunt(Typos::Only(Kind::Swapped)).await, (Vec::new(), 12));
    }

    #[tokio::test(start_paused = true)]
    async fn whole_words_go_before_the_dot() {
        let world = World::new();
        world.registry("nl").taken(&["lemon.nl"]);
        world.finds("intitle:/^[a-z]{3,}$/", &["lemon", "melon"]);
        world.otherwise_quiet();
        let whole = Settings {
            whole: true,
            ..testing::settings()
        };
        let (found, _) = round(&mut world.hunter(&["nl"], whole)).await;
        assert_eq!(domains(&found.hits), vec!["melon.nl"]);
        assert!(found.hits[0].whole);
        assert_eq!(found.taken, 1);

        // With a word of your own behind it, different names come up.
        let boxed = Settings {
            whole: true,
            affix: Affix {
                prefix: String::new(),
                suffix: "box".into(),
            },
            ..testing::settings()
        };
        let other = World::new();
        other.registry("nl");
        other.finds("intitle:/^[a-z]{3,}$/", &["lemon"]);
        other.otherwise_quiet();
        let (found, _) = round(&mut other.hunter(&["nl"], boxed)).await;
        assert_eq!(domains(&found.hits), vec!["lemonbox.nl"]);
        assert_eq!(found.hits[0].word, "lemon");
    }

    #[tokio::test(start_paused = true)]
    async fn a_registry_that_stops_answering_is_asked_again_next_round() {
        let world = World::new();
        world.registry("nl").pages("intitle:/^[a-z]{3,}$/", 20, "");
        world.web.down("rdap.test/nl/");
        world.otherwise_quiet();
        let whole = Settings {
            whole: true,
            ..testing::settings()
        };
        let mut hunter = world.hunter(&["nl"], whole);
        let (first, skipped) = round(&mut hunter).await;
        assert!(first.stuck && first.retry && !first.exhausted);
        assert!(first.hits.is_empty());
        assert_eq!(
            skipped,
            vec![".nl left out for now, lookup failed: cannot connect"]
        );
        // With no extension left, the dictionary is not paged through for nothing.
        assert!(world.web.count("intitle") < 20);

        // It answers again.
        world.web.on("rdap.test/nl/", 404, "");
        let (second, _) = round(&mut hunter).await;
        assert_eq!(second.hits.len(), 10);
        assert!(!second.stuck);
    }

    #[tokio::test(start_paused = true)]
    async fn hardly_any_word_ends_in_nl_so_that_hunt_is_over_at_once() {
        // What made a hunt in only .nl or only .com look broken: the
        // dictionary has nothing, and the round says so right away.
        let world = World::new();
        world.registry("nl");
        world.otherwise_quiet();
        let (found, _) = round(&mut world.hunter(&["nl"], testing::settings())).await;
        assert!(found.exhausted && found.hits.is_empty());
        assert_eq!(found.checked, 0);
    }

    #[tokio::test(start_paused = true)]
    async fn rude_words_are_marked_or_left_out() {
        let script = |world: &World| {
            world.registry("ing");
            let rude = page("bleeding", 1, &[TWO, "Category:English vulgarities"]);
            let fine = page("cooking", 2, &[TWO]);
            world.web.on("+ing$/", 200, &answer(vec![rude, fine]));
            world.otherwise_quiet();
        };
        let shown = World::new();
        script(&shown);
        let (found, _) = round(&mut shown.hunter(&["ing"], testing::settings())).await;
        assert_eq!(domains(&found.hits), vec!["bleed.ing", "cook.ing"]);
        assert_eq!(found.hits[0].tone.as_deref(), Some("vulgar"));
        assert_eq!(found.hits[1].tone.as_deref(), Some(""));

        let left_out = World::new();
        script(&left_out);
        let polite = Settings {
            rude: false,
            ..testing::settings()
        };
        let (found, _) = round(&mut left_out.hunter(&["ing"], polite)).await;
        assert_eq!(domains(&found.hits), vec!["cook.ing"]);
        assert_eq!(left_out.lookups("bleed"), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn an_extension_that_cannot_be_checked_is_left_out_with_a_reason() {
        let world = World::new();
        let closed = Method::Rejected {
            reason: "has no way to look names up".into(),
        };
        let json = serde_json::to_string(&closed).unwrap();
        world.cache.kv_put("method:ing", &json).unwrap();
        world.finds("+ing$/", &["cooking"]);
        world.otherwise_quiet();
        let (found, skipped) = round(&mut world.hunter(&["ing"], testing::settings())).await;
        assert!(found.hits.is_empty());
        assert_eq!(skipped, vec![".ing skipped: has no way to look names up"]);
        // Nothing is ever called available on a guess.
        assert!(world.cache.all().unwrap().is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn a_category_only_takes_its_own_words() {
        let world = World::new();
        world.registry("ing");
        let banking = page("banking", 1, &[TWO, "Category:en:Banking"]);
        world
            .web
            .on("deepcat:\"en:Banking\"", 200, &answer(vec![banking]));
        world.finds("+ing$/", &["cooking"]);
        world.otherwise_quiet();
        let finance = Settings {
            category: Some(Category {
                name: "finance".into(),
                topics: vec!["Banking".into()],
            }),
            ..testing::settings()
        };
        let (found, _) = round(&mut world.hunter(&["ing"], finance)).await;
        assert_eq!(domains(&found.hits), vec!["bank.ing"]);
        assert_eq!(found.hits[0].topics.as_deref(), Some("Banking"));
    }

    #[tokio::test(start_paused = true)]
    async fn a_word_of_your_own_is_tried_in_every_extension() {
        let world = World::new();
        world.registry("ing").registry("ng").taken(&["cooki.ng"]);
        world.web.on(
            "titles=cooking",
            200,
            &answer(vec![page("cooking", 0, &[TWO])]),
        );
        let sense = r#"{"en":[{"definitions":[{"definition":"The art of preparing food"}]}]}"#;
        world.web.on("definition/cooking", 200, sense);
        world.otherwise_quiet();
        let hunter = world.hunter(&["ing", "ng"], testing::settings());
        let looked = hunter.check_word("cooking", &mut |_| {}).await.unwrap();
        assert!(looked.known);
        assert_eq!(looked.tone, "");
        assert_eq!(looked.meaning.as_deref(), Some("The art of preparing food"));
        let verdicts: Vec<(&str, Status)> = looked
            .records
            .iter()
            .map(|r| (r.domain.as_str(), r.status))
            .collect();
        assert!(
            verdicts.contains(&("cook.ing", Status::Available)),
            "{verdicts:?}"
        );
        assert!(
            verdicts.contains(&("cooki.ng", Status::Taken)),
            "{verdicts:?}"
        );

        // A word the dictionary does not have can still be a name.
        let made_up = hunter.check_word("zzzing", &mut |_| {}).await.unwrap();
        assert!(!made_up.known);
        assert_eq!(domains(&made_up.records), vec!["zzz.ing", "zzzi.ng"]);
        assert!(!made_up.whole_instead);
    }

    #[tokio::test(start_paused = true)]
    async fn a_word_no_extension_fits_goes_before_the_dot() {
        let world = World::new();
        world.registry("ing").registry("ng").taken(&["lemon.ng"]);
        world.otherwise_quiet();
        let hunter = world.hunter(&["ing", "ng"], testing::settings());
        let looked = hunter.check_word("lemon", &mut |_| {}).await.unwrap();
        assert!(looked.whole_instead);
        let mut verdicts: Vec<(&str, Status)> = looked
            .records
            .iter()
            .map(|r| (r.domain.as_str(), r.status))
            .collect();
        verdicts.sort_by_key(|(domain, _)| *domain);
        assert_eq!(
            verdicts,
            vec![
                ("lemon.ing", Status::Available),
                ("lemon.ng", Status::Taken)
            ]
        );
        // The name style stays what it was.
        assert!(!hunter.settings.whole);

        // Every extension at once would be hundreds of names.
        let everywhere = Hunter::new(
            world.wiki(&[1, 2, 3]),
            world.checker(),
            world.cache.clone(),
            vec!["ing".into(), "ng".into()],
            false,
            testing::settings(),
        );
        let looked = everywhere.check_word("melon", &mut |_| {}).await.unwrap();
        assert!(!looked.whole_instead);
        assert!(looked.records.is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn old_verdicts_get_a_new_look() {
        let world = World::new();
        world.registry("ing").taken(&["cook.ing"]);
        world.otherwise_quiet();
        let was_free = Record::sample("cook.ing", 5);
        world.cache.put(&was_free).unwrap();
        let hunter = world.hunter(&["ing"], testing::settings());
        let mut seen = Vec::new();
        let mut on_event = |event: Event| {
            if let Event::Checked(record) = event {
                seen.push(record.status);
            }
        };
        let looked = hunter.recheck(vec![was_free], &mut on_event).await.unwrap();
        assert_eq!((looked.checked, looked.taken), (1, 1));
        assert_eq!(seen, vec![Status::Taken]);
        let now = world.cache.get("cook.ing").unwrap().unwrap();
        assert_eq!(
            (now.status, now.checked_at),
            (Status::Taken, world.cache.now())
        );
    }

    #[test]
    fn a_three_syllable_word_is_a_word_too() {
        // The helpers of the test world agree with the dictionary's names.
        assert!(ONE.contains("1-syllable") && THREE.contains("3-syllable"));
    }

    fn endings(tld: &str) -> Vec<(String, Vec<String>)> {
        streams_for(tld, Typos::All)
            .into_iter()
            .map(|s| {
                (
                    s.variant.suffix,
                    s.riders.into_iter().map(|r| r.suffix).collect(),
                )
            })
            .collect()
    }

    #[test]
    fn longer_endings_ride_along() {
        let ai = endings("ai");
        // -aai is part of the -ai range, the other typos need their own lookup.
        assert_eq!(ai[0], ("ai".to_string(), vec!["aai".to_string()]));
        assert!(ai.iter().any(|(suffix, _)| suffix == "ay"));
        assert!(ai.iter().all(|(suffix, _)| suffix != "aai"));
        // kn.app: words on -ap do not end in -app, so that is a range of its own.
        let app = endings("app");
        assert_eq!(app[0], ("app".to_string(), vec!["aapp".to_string()]));
        assert!(app.iter().any(|(suffix, _)| suffix == "ap"));
    }

    #[test]
    fn category_words_meet_every_tld() {
        let lane = |tld: &str| Lane {
            tld: tld.into(),
            streams: Vec::new(),
            turn: 0,
            prepared: false,
            warming: false,
            benched: false,
            unclear: false,
            unknown_streak: 0,
        };
        let lanes = vec![lane("ng"), lane("st"), lane("app")];
        let endings: Vec<(usize, Variant)> = lanes
            .iter()
            .enumerate()
            .flat_map(|(i, l)| {
                variants(&l.tld, Typos::All)
                    .into_iter()
                    .map(move |v| (i, v))
            })
            .collect();
        let word = |text: &str, prevalence| Word {
            text: text.into(),
            syllables: Some(2),
            prevalence,
            topics: vec!["Financieel".into()],
            ..Word::default()
        };
        let words = [
            word("schap", Some(100)),
            word("winst", None),
            word("lening", Some(99)),
            word("fiets", Some(100)),
            word("angst", Some(98)),
        ];
        let mut settings = Settings {
            count: 10,
            max_checks: 150,
            min_label: 2,
            typos: Typos::All,
            category: None,
            lang: Lang::Dutch,
            whole: false,
            rude: true,
            affix: Affix::default(),
        };
        let mut names = cross(&words, &endings, &lanes, &settings);
        names.sort_by(|a, b| best_first(&a.1, &b.1));
        let found: Vec<(usize, &str)> = names
            .iter()
            .map(|(lane, name)| (*lane, name.domain.as_str()))
            .collect();
        // Real spellings first (best known word first, unknown prevalence
        // last), then the doubled letter, then the swapped ones and the one
        // with a letter missing.
        assert_eq!(
            found,
            vec![
                (0, "leni.ng"),
                (1, "ang.st"),
                (1, "win.st"),
                (2, "sch.app"),
                (1, "fie.st"),
                (0, "lni.ng"),
            ]
        );
        assert_eq!(names[0].1.topics, "Financieel");

        // The same words as whole names, in the order the words came in.
        // Neither has a typo that reads like Dutch, so those all come last.
        settings.whole = true;
        let lanes = vec![lane("nl")];
        let mut names = cross(&words[..2], &[], &lanes, &settings);
        names.sort_by(|a, b| best_first(&a.1, &b.1));
        let found: Vec<&str> = names.iter().map(|(_, n)| n.domain.as_str()).collect();
        assert_eq!(
            found,
            vec![
                "schap.nl",
                "winst.nl",
                "schapp.nl",
                "winstt.nl",
                "schpa.nl",
                "wints.nl"
            ]
        );
        assert!(names.iter().all(|(_, n)| n.whole));

        // A word the dictionary flags is marked, or left out when asked.
        let mut flagged = words[1].clone();
        flagged.tone = vec!["vulgar".into()];
        let both = [words[0].clone(), flagged];
        let names = cross(&both, &[], &lanes, &settings);
        assert!(
            names
                .iter()
                .any(|(_, n)| n.word == "winst" && n.tone == "vulgar")
        );
        settings.rude = false;
        let names = cross(&both, &[], &lanes, &settings);
        assert!(!names.is_empty() && names.iter().all(|(_, n)| n.word == "schap"));

        // With a fixed part every word makes one name, and no typos.
        settings.affix.suffix = "box".into();
        let names = cross(&words[..2], &[], &lanes, &settings);
        let found: Vec<&str> = names.iter().map(|(_, n)| n.domain.as_str()).collect();
        assert_eq!(found, vec!["schapbox.nl", "winstbox.nl"]);
    }

    #[test]
    fn by_tld_counts() {
        let record = |tld: &str, status| Record {
            status,
            ..Record::sample(&format!("x.{tld}"), 0)
        };
        let rows = by_tld(&[
            record("ai", Status::Taken),
            record("ai", Status::Available),
            record("io", Status::Taken),
        ]);
        assert_eq!(
            rows,
            vec![("ai".to_string(), 2, 1), ("io".to_string(), 1, 0)]
        );
    }
}
