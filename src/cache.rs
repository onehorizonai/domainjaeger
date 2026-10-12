//! Local SQLite cache: every verified domain, the position of each dictionary
//! stream and a small key/value store for downloaded registry metadata.

use std::path::Path;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, params};

use crate::check::Status;
use crate::net::Clock;

/// A verdict is trusted for four weeks, after that the domain is checked again.
pub const DOMAIN_TTL_SECS: i64 = 28 * 24 * 3600;
/// "Unknown" verdicts (rate limits, timeouts) are retried much sooner.
pub const UNKNOWN_TTL_SECS: i64 = 6 * 3600;

#[derive(Debug, Clone)]
pub struct Record {
    pub domain: String,
    pub word: String,
    pub label: String,
    pub tld: String,
    /// "exact" or the typo flavour, see `hacks::Kind`.
    pub kind: String,
    /// What the domain reads as without the dot, e.g. "knapp" for kn.app.
    pub spelled: String,
    pub syllables: Option<u8>,
    /// Percentage of Dutch speakers that know the word (WikiWoordenboek).
    pub prevalence: Option<u8>,
    /// Listed in the official Woordenlijst Nederlandse Taal.
    pub official: bool,
    /// The dictionary's topics of the word ("Economie, Financieel"), see
    /// `topics`.
    pub topics: Option<String>,
    /// The language of the word: "en", "nl", "la" or "es", see `lang`.
    pub lang: String,
    /// The name is the whole word (lening.nl) instead of a hack (leni.ng).
    pub whole: bool,
    /// What the word means, in a few words of English. `None` when nobody
    /// asked the dictionary yet, empty when it had nothing to say.
    pub meaning: Option<String>,
    /// Why the word may not be for everyone ("vulgar, sexual"), see `tone`.
    /// Empty for a word the dictionary has no such label for.
    pub tone: Option<String>,
    pub status: Status,
    pub source: String,
    pub detail: String,
    pub checked_at: i64,
}

impl Record {
    pub fn ttl(&self) -> i64 {
        match self.status {
            Status::Unknown => UNKNOWN_TTL_SECS,
            _ => DOMAIN_TTL_SECS,
        }
    }

    pub fn valid_until(&self) -> i64 {
        self.checked_at + self.ttl()
    }

    /// Is the verdict still to be trusted at `now`?
    pub fn is_fresh(&self, now: i64) -> bool {
        self.valid_until() > now
    }
}

#[derive(Debug, Default, Clone)]
pub struct Stats {
    pub total: u64,
    pub available: u64,
    pub taken: u64,
    pub unknown: u64,
    pub stale: u64,
    pub words: u64,
    pub tlds: u64,
}

pub struct Cache {
    conn: Mutex<Connection>,
    clock: Arc<dyn Clock>,
}

/// The layout of the tables. A later version that changes them raises
/// this, and upgrades what an older one left behind.
const SCHEMA_VERSION: i64 = 1;

impl Cache {
    pub fn open(path: &Path, clock: Arc<dyn Clock>) -> Result<Self> {
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir)
                .with_context(|| format!("cannot create cache directory {}", dir.display()))?;
        }
        let conn = Connection::open(path)
            .with_context(|| format!("cannot open cache {}", path.display()))?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             CREATE TABLE IF NOT EXISTS domains (
                 domain     TEXT PRIMARY KEY,
                 word       TEXT NOT NULL,
                 label      TEXT NOT NULL,
                 tld        TEXT NOT NULL,
                 kind       TEXT NOT NULL,
                 spelled    TEXT NOT NULL,
                 syllables  INTEGER,
                 prevalence INTEGER,
                 official   INTEGER NOT NULL DEFAULT 0,
                 status     TEXT NOT NULL,
                 source     TEXT NOT NULL,
                 detail     TEXT NOT NULL DEFAULT '',
                 checked_at INTEGER NOT NULL,
                 topics     TEXT,
                 lang       TEXT NOT NULL DEFAULT 'en',
                 whole      INTEGER NOT NULL DEFAULT 0,
                 meaning    TEXT,
                 tone       TEXT
             );
             CREATE TABLE IF NOT EXISTS streams (
                 tld        TEXT NOT NULL,
                 suffix     TEXT NOT NULL,
                 cursor     TEXT,
                 done       INTEGER NOT NULL DEFAULT 0,
                 updated_at INTEGER NOT NULL,
                 PRIMARY KEY (tld, suffix)
             );
             CREATE TABLE IF NOT EXISTS kv (
                 key        TEXT PRIMARY KEY,
                 value      TEXT NOT NULL,
                 updated_at INTEGER NOT NULL
             );",
        )?;
        conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        Ok(Self {
            conn: Mutex::new(conn),
            clock,
        })
    }

    /// A cache that lives in memory and is gone afterwards, for tests.
    #[cfg(test)]
    pub fn in_memory(clock: Arc<dyn Clock>) -> Self {
        Self::open(Path::new(":memory:"), clock).expect("a cache in memory")
    }

    /// What time it is, by the clock the cache was given.
    pub fn now(&self) -> i64 {
        self.clock.now()
    }

    fn conn(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn get(&self, domain: &str) -> Result<Option<Record>> {
        let conn = self.conn();
        let rec = conn
            .query_row(
                &format!("SELECT {COLUMNS} FROM domains WHERE domain = ?1"),
                params![domain],
                row_to_record,
            )
            .optional()?;
        Ok(rec)
    }

    /// Only returns the record when its verdict is still within its TTL.
    pub fn get_fresh(&self, domain: &str) -> Result<Option<Record>> {
        let now = self.now();
        Ok(self.get(domain)?.filter(|r| r.is_fresh(now)))
    }

    pub fn put(&self, r: &Record) -> Result<()> {
        self.conn().execute(
            "INSERT INTO domains (domain, word, label, tld, kind, spelled, syllables, prevalence,
                                  official, status, source, detail, checked_at, topics, lang,
                                  whole, meaning, tone)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16,
                     ?17, ?18)
             ON CONFLICT(domain) DO UPDATE SET
                 word = excluded.word, label = excluded.label, tld = excluded.tld,
                 kind = excluded.kind, spelled = excluded.spelled,
                 syllables = excluded.syllables, prevalence = excluded.prevalence,
                 official = excluded.official, status = excluded.status,
                 source = excluded.source, detail = excluded.detail,
                 checked_at = excluded.checked_at, topics = excluded.topics,
                 lang = excluded.lang, whole = excluded.whole,
                 meaning = COALESCE(excluded.meaning, domains.meaning),
                 tone = COALESCE(excluded.tone, domains.tone)",
            params![
                r.domain,
                r.word,
                r.label,
                r.tld,
                r.kind,
                r.spelled,
                r.syllables,
                r.prevalence,
                r.official,
                r.status.as_str(),
                r.source,
                r.detail,
                r.checked_at,
                r.topics,
                r.lang,
                r.whole,
                r.meaning,
                r.tone
            ],
        )?;
        Ok(())
    }

    /// Everything ever checked, available names first.
    pub fn all(&self) -> Result<Vec<Record>> {
        self.select(
            "ORDER BY CASE status WHEN 'available' THEN 0 WHEN 'unknown' THEN 1 ELSE 2 END,
                      COALESCE(prevalence, -1) DESC, domain",
        )
    }

    /// Records whose verdict is older than its TTL and needs a re-check.
    pub fn stale(&self) -> Result<Vec<Record>> {
        let now = self.now();
        Ok(self
            .select("ORDER BY checked_at")?
            .into_iter()
            .filter(|r| !r.is_fresh(now))
            .collect())
    }

    fn select(&self, tail: &str) -> Result<Vec<Record>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(&format!("SELECT {COLUMNS} FROM domains {tail}"))?;
        let rows = stmt.query_map([], row_to_record)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// What a word means, if the dictionary was asked about it before.
    /// Empty when it had nothing to say.
    pub fn meaning_of(&self, lang: &str, word: &str) -> Result<Option<String>> {
        let meaning = self
            .conn()
            .query_row(
                "SELECT meaning FROM domains
                 WHERE word = ?1 AND lang = ?2 AND meaning IS NOT NULL LIMIT 1",
                params![word, lang],
                |r| r.get(0),
            )
            .optional()?;
        Ok(meaning)
    }

    /// The words (language and spelling) with an available name that the
    /// dictionary was never asked the meaning of, latest finds first.
    pub fn unexplained(&self) -> Result<Vec<(String, String)>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT lang, word FROM domains WHERE status = 'available'
             GROUP BY lang, word HAVING MAX(meaning IS NOT NULL) = 0
             ORDER BY MAX(checked_at) DESC, word",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Record what a word means with every name made of it.
    pub fn explain(&self, lang: &str, word: &str, meaning: &str) -> Result<()> {
        self.conn().execute(
            "UPDATE domains SET meaning = ?3 WHERE word = ?1 AND lang = ?2",
            params![word, lang, meaning],
        )?;
        Ok(())
    }

    pub fn stats(&self) -> Result<Stats> {
        let records = self.all()?;
        let now = self.now();
        let mut s = Stats {
            total: records.len() as u64,
            ..Stats::default()
        };
        let mut words = std::collections::HashSet::new();
        let mut tlds = std::collections::HashSet::new();
        for r in &records {
            words.insert(r.word.as_str());
            tlds.insert(r.tld.as_str());
            if !r.is_fresh(now) {
                s.stale += 1;
            }
            match r.status {
                Status::Available => s.available += 1,
                Status::Taken => s.taken += 1,
                Status::Unknown => s.unknown += 1,
            }
        }
        s.words = words.len() as u64;
        s.tlds = tlds.len() as u64;
        Ok(s)
    }

    // --- dictionary stream positions -------------------------------------

    /// Returns `(cursor, done)` for a stream, forgetting positions older than
    /// the domain TTL so an exhausted stream starts over once its results expired.
    pub fn stream_state(&self, tld: &str, suffix: &str) -> Result<(Option<String>, bool)> {
        let conn = self.conn();
        let row: Option<(Option<String>, bool, i64)> = conn
            .query_row(
                "SELECT cursor, done, updated_at FROM streams WHERE tld = ?1 AND suffix = ?2",
                params![tld, suffix],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        Ok(match row {
            Some((cursor, done, updated_at)) if self.now() - updated_at < DOMAIN_TTL_SECS => {
                (cursor, done)
            }
            _ => (None, false),
        })
    }

    pub fn set_stream_state(
        &self,
        tld: &str,
        suffix: &str,
        cursor: Option<&str>,
        done: bool,
    ) -> Result<()> {
        self.conn().execute(
            "INSERT INTO streams (tld, suffix, cursor, done, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(tld, suffix) DO UPDATE SET
                 cursor = excluded.cursor, done = excluded.done, updated_at = excluded.updated_at",
            params![tld, suffix, cursor, done, self.now()],
        )?;
        Ok(())
    }

    pub fn reset_streams(&self) -> Result<usize> {
        Ok(self.conn().execute("DELETE FROM streams", [])?)
    }

    // --- key/value metadata ----------------------------------------------

    pub fn kv_get(&self, key: &str, max_age_secs: i64) -> Result<Option<String>> {
        let now = self.now();
        let conn = self.conn();
        let row: Option<(String, i64)> = conn
            .query_row(
                "SELECT value, updated_at FROM kv WHERE key = ?1",
                params![key],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        Ok(row
            .filter(|(_, at)| now - at < max_age_secs)
            .map(|(v, _)| v))
    }

    pub fn kv_put(&self, key: &str, value: &str) -> Result<()> {
        self.conn().execute(
            "INSERT INTO kv (key, value, updated_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
            params![key, value, self.now()],
        )?;
        Ok(())
    }
}

const COLUMNS: &str = "domain, word, label, tld, kind, spelled, syllables, prevalence, official, \
                       status, source, detail, checked_at, topics, lang, whole, meaning, \
                       tone";

fn row_to_record(r: &rusqlite::Row<'_>) -> rusqlite::Result<Record> {
    let status: String = r.get(9)?;
    Ok(Record {
        domain: r.get(0)?,
        word: r.get(1)?,
        label: r.get(2)?,
        tld: r.get(3)?,
        kind: r.get(4)?,
        spelled: r.get(5)?,
        syllables: r.get(6)?,
        prevalence: r.get(7)?,
        official: r.get(8)?,
        status: Status::parse(&status),
        source: r.get(10)?,
        detail: r.get(11)?,
        checked_at: r.get(12)?,
        topics: r.get(13)?,
        lang: r.get(14)?,
        whole: r.get(15)?,
        meaning: r.get(16)?,
        tone: r.get(17)?,
    })
}

#[cfg(test)]
impl Record {
    /// A verdict on `label.tld` for tests: an exact, available name that
    /// was checked at `checked_at`. Change what a test is about.
    pub fn sample(domain: &str, checked_at: i64) -> Self {
        let (label, tld) = domain.split_once('.').expect("a label and an extension");
        Self {
            domain: domain.to_string(),
            word: format!("{label}{tld}"),
            label: label.to_string(),
            tld: tld.to_string(),
            kind: "exact".to_string(),
            spelled: format!("{label}{tld}"),
            syllables: Some(2),
            prevalence: None,
            official: false,
            topics: Some(String::new()),
            lang: "en".to_string(),
            whole: false,
            meaning: None,
            tone: Some(String::new()),
            status: Status::Available,
            source: "rdap".to_string(),
            detail: String::new(),
            checked_at,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::fake;

    const DAY: i64 = 24 * 3600;

    fn cache_at(now: i64) -> (Cache, Arc<fake::Clock>) {
        let clock = Arc::new(fake::Clock::at(now));
        (Cache::in_memory(clock.clone()), clock)
    }

    #[test]
    fn a_record_comes_back_the_way_it_went_in() {
        let (cache, _) = cache_at(1_000);
        let record = Record {
            prevalence: Some(99),
            official: true,
            topics: Some("Finance".into()),
            whole: true,
            meaning: Some("a place to cook".into()),
            tone: Some("vulgar".into()),
            detail: "no registration found".into(),
            ..Record::sample("cook.ing", 1_000)
        };
        cache.put(&record).unwrap();
        let back = cache.get("cook.ing").unwrap().unwrap();
        assert_eq!(format!("{back:?}"), format!("{record:?}"));
        assert!(cache.get("bak.ing").unwrap().is_none());
        assert_eq!(cache.all().unwrap().len(), 1);
    }

    #[test]
    fn a_cache_on_disk_is_still_there_the_next_time() {
        let dir = std::env::temp_dir().join(format!("domainjaeger-test-{}", std::process::id()));
        let path = dir.join("deeper").join("cache.sqlite");
        let clock: Arc<dyn Clock> = Arc::new(fake::Clock::at(5));
        // The folders on the way are made.
        let cache = Cache::open(&path, clock.clone()).unwrap();
        cache.put(&Record::sample("sn.app", 5)).unwrap();
        drop(cache);
        let again = Cache::open(&path, clock).unwrap();
        assert!(again.get("sn.app").unwrap().is_some());
        let version: i64 = again
            .conn()
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);
        drop(again);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_verdict_is_trusted_for_four_weeks_and_a_shrug_for_six_hours() {
        let (cache, clock) = cache_at(0);
        cache.put(&Record::sample("cook.ing", 0)).unwrap();
        let shrug = Record {
            status: Status::Unknown,
            ..Record::sample("sn.app", 0)
        };
        cache.put(&shrug).unwrap();
        assert!(cache.get_fresh("cook.ing").unwrap().is_some());
        assert!(cache.get_fresh("sn.app").unwrap().is_some());
        assert!(cache.stale().unwrap().is_empty());

        clock.advance(UNKNOWN_TTL_SECS);
        assert!(cache.get_fresh("sn.app").unwrap().is_none());
        assert!(cache.get_fresh("cook.ing").unwrap().is_some());

        clock.advance(27 * DAY);
        assert!(cache.get_fresh("cook.ing").unwrap().is_some());
        clock.advance(DAY);
        assert!(cache.get_fresh("cook.ing").unwrap().is_none());
        // It is not forgotten, only due for another look.
        assert!(cache.get("cook.ing").unwrap().is_some());
        let stale: Vec<String> = cache
            .stale()
            .unwrap()
            .into_iter()
            .map(|r| r.domain)
            .collect();
        assert_eq!(stale, vec!["cook.ing", "sn.app"]);
    }

    #[test]
    fn a_new_verdict_keeps_the_meaning_and_the_tone_there_were() {
        let (cache, _) = cache_at(0);
        let first = Record {
            meaning: Some("profit".into()),
            tone: Some("vulgar".into()),
            ..Record::sample("win.st", 1)
        };
        cache.put(&first).unwrap();
        let again = Record {
            meaning: None,
            tone: None,
            status: Status::Taken,
            ..Record::sample("win.st", 2)
        };
        cache.put(&again).unwrap();
        let kept = cache.get("win.st").unwrap().unwrap();
        assert_eq!((kept.checked_at, kept.status), (2, Status::Taken));
        assert_eq!(kept.meaning.as_deref(), Some("profit"));
        assert_eq!(kept.tone.as_deref(), Some("vulgar"));
    }

    #[test]
    fn meanings_are_kept_per_word_and_language() {
        let (cache, _) = cache_at(0);
        let dutch = |domain: &str, at| Record {
            word: "winst".into(),
            lang: "nl".into(),
            ..Record::sample(domain, at)
        };
        cache.put(&dutch("win.st", 1)).unwrap();
        cache.put(&dutch("winst.nl", 2)).unwrap();
        let taken = Record {
            status: Status::Taken,
            ..Record::sample("kun.st", 3)
        };
        cache.put(&taken).unwrap();
        // Only available names are worth asking the meaning of, once a word.
        let word = |lang: &str, word: &str| (lang.to_string(), word.to_string());
        assert_eq!(cache.unexplained().unwrap(), vec![word("nl", "winst")]);
        assert_eq!(cache.meaning_of("nl", "winst").unwrap(), None);

        cache.explain("nl", "winst", "profit").unwrap();
        assert!(cache.unexplained().unwrap().is_empty());
        assert_eq!(
            cache.meaning_of("nl", "winst").unwrap().as_deref(),
            Some("profit")
        );
        // Every name made of the word has it now, and no other language.
        let meaning = |domain| cache.get(domain).unwrap().unwrap().meaning;
        assert_eq!(meaning("winst.nl").as_deref(), Some("profit"));
        assert_eq!(cache.meaning_of("en", "winst").unwrap(), None);
        // "Nothing to say" is an answer too.
        cache.put(&Record::sample("sn.app", 4)).unwrap();
        cache.explain("en", "snapp", "").unwrap();
        assert!(cache.unexplained().unwrap().is_empty());
    }

    #[test]
    fn the_numbers_add_up() {
        let (cache, clock) = cache_at(0);
        cache.put(&Record::sample("cook.ing", 0)).unwrap();
        cache.put(&Record::sample("bak.ing", 0)).unwrap();
        let other = |domain: &str, status| Record {
            status,
            ..Record::sample(domain, 0)
        };
        cache.put(&other("sn.app", Status::Taken)).unwrap();
        cache.put(&other("tr.app", Status::Unknown)).unwrap();
        let stats = cache.stats().unwrap();
        assert_eq!(
            (stats.total, stats.available, stats.taken, stats.unknown),
            (4, 2, 1, 1)
        );
        assert_eq!((stats.words, stats.tlds, stats.stale), (4, 2, 0));
        clock.advance(DAY);
        assert_eq!(cache.stats().unwrap().stale, 1);
    }

    #[test]
    fn the_place_in_the_dictionary_is_remembered_for_four_weeks() {
        let (cache, clock) = cache_at(0);
        assert_eq!(cache.stream_state("app", "ap").unwrap(), (None, false));
        cache
            .set_stream_state("app", "ap", Some("page 2"), false)
            .unwrap();
        assert_eq!(
            cache.stream_state("app", "ap").unwrap(),
            (Some("page 2".to_string()), false)
        );
        cache.set_stream_state("app", "ap", None, true).unwrap();
        assert_eq!(cache.stream_state("app", "ap").unwrap(), (None, true));
        // Once the verdicts have expired, the words are due again.
        clock.advance(DOMAIN_TTL_SECS);
        assert_eq!(cache.stream_state("app", "ap").unwrap(), (None, false));

        cache
            .set_stream_state("ing", "ing", Some("x"), false)
            .unwrap();
        assert_eq!(cache.reset_streams().unwrap(), 2);
        assert_eq!(cache.stream_state("ing", "ing").unwrap(), (None, false));
    }

    #[test]
    fn notes_have_an_age() {
        let (cache, clock) = cache_at(100);
        assert_eq!(cache.kv_get("contact", i64::MAX).unwrap(), None);
        cache.kv_put("contact", "me@example.org").unwrap();
        clock.advance(59);
        assert_eq!(
            cache.kv_get("contact", 60).unwrap().as_deref(),
            Some("me@example.org")
        );
        clock.advance(1);
        assert_eq!(cache.kv_get("contact", 60).unwrap(), None);
        assert!(cache.kv_get("contact", i64::MAX).unwrap().is_some());
        // Writing it again makes it young again.
        cache.kv_put("contact", "other@example.org").unwrap();
        assert_eq!(
            cache.kv_get("contact", 60).unwrap().as_deref(),
            Some("other@example.org")
        );
    }
}
