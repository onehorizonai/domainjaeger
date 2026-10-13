//! Domain hacks: turning a word into `label.tld` where the TLD is the end of
//! the word (papegaai -> papega.ai), optionally with one small typo
//! (knap -> kn.app).
//!
//! And the plain alternative for TLDs no word ends in, like .nl: the whole
//! word as the name (lening.nl), or a small typo of it (lenning.nl). Or the
//! word with a fixed part before or after it: headspeed, speedbox.

use std::collections::HashSet;

use crate::wiktionary::Word;

/// How the end of the word relates to the TLD.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    /// The word ends in the TLD: papegaai -> papega.ai
    Exact,
    /// The TLD doubles a letter of the word: knap -> kn.app
    Doubled,
    /// The domain drops one of a double letter: tobbe -> to.be
    Undoubled,
    /// A sound-alike letter: display -> displ.ai, check -> che.cc
    SoundAlike,
    /// Two neighbouring letters swapped: mooi -> mo.io
    Swapped,
    /// One letter left out: numbers -> nmbe.rs
    Missing,
}

impl Kind {
    /// The typos, in the order they are offered.
    pub const TYPOS: [Kind; 5] = [
        Kind::Doubled,
        Kind::Undoubled,
        Kind::SoundAlike,
        Kind::Swapped,
        Kind::Missing,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Exact => "exact",
            Kind::Doubled => "doubled letter",
            Kind::Undoubled => "dropped double",
            Kind::SoundAlike => "sound-alike",
            Kind::Swapped => "swapped letters",
            Kind::Missing => "missing letter",
        }
    }

    /// The word that picks this kind of typo: `typos missing`.
    pub fn word(self) -> &'static str {
        match self {
            Kind::Exact => "exact",
            Kind::Doubled => "doubled",
            Kind::Undoubled => "dropped",
            Kind::SoundAlike => "sound",
            Kind::Swapped => "swapped",
            Kind::Missing => "missing",
        }
    }

    /// What this kind makes of a word: the word, the name with the dot
    /// inside it, and what a whole name reads as.
    pub fn example(self) -> (&'static str, &'static str, &'static str) {
        match self {
            Kind::Exact => ("cooking", "cook.ing", "cooking"),
            Kind::Doubled => ("snap", "sn.app", "snapp"),
            Kind::Undoubled => ("especially", "especia.ly", "especialy"),
            Kind::SoundAlike => ("today", "tod.ai", "todai"),
            Kind::Swapped => ("delicious", "delicio.su", "deliciosu"),
            Kind::Missing => ("numbers", "nmbe.rs", "nmbers"),
        }
    }

    pub fn is_typo(self) -> bool {
        self != Kind::Exact
    }

    /// How far a hack strays from the word: a doubled or dropped letter
    /// (kn.app) reads almost like the real thing, swapped letters do not.
    pub fn rank(self) -> u8 {
        match self {
            Kind::Exact => 0,
            Kind::Doubled | Kind::Undoubled => 1,
            Kind::SoundAlike => 2,
            Kind::Swapped | Kind::Missing => 3,
        }
    }
}

/// Which small typos a hunt makes names with: all of them, none, or one
/// kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Typos {
    #[default]
    All,
    Off,
    Only(Kind),
}

impl Typos {
    /// What can be typed or passed for it.
    pub const CHOICES: &'static str = "all, off, doubled, dropped, sound, swapped or missing";

    pub fn allows(self, kind: Kind) -> bool {
        match self {
            Typos::All => kind.is_typo(),
            Typos::Off => false,
            Typos::Only(only) => only == kind,
        }
    }

    pub fn is_off(self) -> bool {
        self == Typos::Off
    }

    /// From what was typed or passed: "all", "off", or the word of one kind.
    /// Capitals, spaces and hyphens do not matter, and the full name of a
    /// kind ("missing letter") is fine too.
    pub fn parse(text: &str) -> Option<Self> {
        let key: String = text
            .to_lowercase()
            .chars()
            .filter(|c| c.is_alphanumeric())
            .collect();
        let kind = match key.as_str() {
            "all" | "on" | "yes" => return Some(Typos::All),
            "off" | "no" | "none" => return Some(Typos::Off),
            "doubled" | "doubledletter" => Kind::Doubled,
            "dropped" | "droppeddouble" => Kind::Undoubled,
            "sound" | "soundalike" => Kind::SoundAlike,
            "swapped" | "swappedletters" => Kind::Swapped,
            "missing" | "missingletter" | "left" | "leftout" => Kind::Missing,
            _ => return None,
        };
        Some(Typos::Only(kind))
    }
}

/// A word ending that maps onto a TLD.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Variant {
    /// The ending we look up in the dictionary ("ap" for .app).
    pub suffix: String,
    pub kind: Kind,
}

#[derive(Debug, Clone)]
pub struct Candidate {
    pub domain: String,
    pub word: String,
    pub label: String,
    pub tld: String,
    pub kind: Kind,
    pub syllables: Option<u8>,
    pub prevalence: Option<u8>,
    pub official: bool,
    /// The word's topics, e.g. "Economie, Financieel".
    pub topics: String,
    /// Why the word may not be for everyone, e.g. "vulgar, sexual".
    pub tone: String,
    /// The name is the whole word (lening.nl), the TLD is not part of it.
    pub whole: bool,
    /// How good a name it is among the others of its word, 0 is best. Names
    /// from 2 up wait until the better ones of every word had their turn.
    pub rank: u8,
}

impl Candidate {
    /// What the domain reads as: "knapp" for kn.app, "lenning" for
    /// lenning.nl.
    pub fn spelled(&self) -> String {
        if self.whole {
            self.label.clone()
        } else {
            format!("{}{}", self.label, self.tld)
        }
    }
}

/// Letters that sound alike enough in Dutch to pass as a small typo.
const SOUND_ALIKE: &[(char, char)] = &[('c', 'k'), ('i', 'y'), ('s', 'z')];

/// All word endings worth looking up for a TLD, best ones first. A name with
/// a letter missing has no ending of its own: see `missing_letter`.
pub fn variants(tld: &str, typos: Typos) -> Vec<Variant> {
    let chars: Vec<char> = tld.chars().collect();
    let mut out = vec![Variant {
        suffix: tld.to_string(),
        kind: Kind::Exact,
    }];
    let mut seen: HashSet<String> = HashSet::from([tld.to_string()]);
    // A kind that is not wanted makes no ending at all, so that an ending
    // two kinds share is named after the one that is.
    let mut push = |suffix: String, kind: Kind, out: &mut Vec<Variant>| {
        if typos.allows(kind) && suffix.chars().count() >= 2 && seen.insert(suffix.clone()) {
            out.push(Variant { suffix, kind });
        }
    };

    // knap -> kn.app: the TLD has a double letter, the word a single one.
    for i in 0..chars.len().saturating_sub(1) {
        if chars[i] == chars[i + 1] {
            let mut s = chars.clone();
            s.remove(i);
            push(s.into_iter().collect(), Kind::Doubled, &mut out);
        }
    }
    // tobbe -> to.be: the word has a double letter, the domain keeps one.
    for i in 0..chars.len() {
        let already_double = (i > 0 && chars[i - 1] == chars[i])
            || (i + 1 < chars.len() && chars[i + 1] == chars[i]);
        if !already_double {
            let mut s = chars.clone();
            s.insert(i, chars[i]);
            push(s.into_iter().collect(), Kind::Undoubled, &mut out);
        }
    }
    // display -> displ.ai: swap in a letter that sounds the same.
    for i in 0..chars.len() {
        for &(a, b) in SOUND_ALIKE {
            let replacement = if chars[i] == a {
                b
            } else if chars[i] == b {
                a
            } else {
                continue;
            };
            let mut s = chars.clone();
            s[i] = replacement;
            push(s.into_iter().collect(), Kind::SoundAlike, &mut out);
        }
    }
    // mooi -> mo.io: two neighbouring letters swapped.
    for i in 0..chars.len().saturating_sub(1) {
        if chars[i] != chars[i + 1] {
            let mut s = chars.clone();
            s.swap(i, i + 1);
            push(s.into_iter().collect(), Kind::Swapped, &mut out);
        }
    }
    out
}

/// Build the domain for a word that ends in `variant.suffix`.
pub fn candidate(word: &Word, tld: &str, variant: &Variant, min_label: usize) -> Option<Candidate> {
    let label = word.text.strip_suffix(variant.suffix.as_str())?;
    if label.len() < min_label || label.len() > 63 || !is_plain(label) {
        return None;
    }
    Some(Candidate {
        domain: format!("{label}.{tld}"),
        word: word.shown(),
        label: label.to_string(),
        tld: tld.to_string(),
        kind: variant.kind,
        syllables: word.syllables,
        prevalence: word.prevalence,
        official: word.official,
        topics: word.topic_list(),
        tone: word.tone_list(),
        whole: false,
        rank: variant.kind.rank(),
    })
}

/// The letters a word still reads without: nmbers.
const SPARE: &str = "aeiou";

/// A word or label with its first spare letter left out: numbers -> nmbers.
/// Never the first letter, and never one of a double: that is a dropped
/// double. What is left keeps a vowel to read it by, so knap has none.
/// `last` says whether the last letter may go: it may in a label, which an
/// extension follows, and not in a whole word.
fn without_a_letter(text: &str, last: bool) -> Option<String> {
    let mut chars: Vec<char> = text.chars().collect();
    let end = chars.len() - usize::from(!last && !chars.is_empty());
    let at = (1..end).find(|&i| {
        let letter = chars[i];
        SPARE.contains(letter) && chars[i - 1] != letter && chars.get(i + 1) != Some(&letter)
    })?;
    chars.remove(at);
    chars
        .iter()
        .copied()
        .any(is_vowel)
        .then(|| chars.into_iter().collect())
}

/// The name with a letter missing that goes with a name where the TLD ends
/// the word: numbe.rs -> nmbe.rs. The letter leaves the label, so the word
/// keeps its ending and the dictionary needs no extra question.
pub fn missing_letter(exact: &Candidate, min_label: usize) -> Option<Candidate> {
    if exact.kind != Kind::Exact || exact.whole {
        return None;
    }
    let label = without_a_letter(&exact.label, true)?;
    (label.len() >= min_label).then(|| Candidate {
        domain: format!("{label}.{}", exact.tld),
        label,
        kind: Kind::Missing,
        rank: Kind::Missing.rank(),
        ..exact.clone()
    })
}

/// A name, and behind it the same name with a letter missing when that is
/// wanted and the name has one to spare.
pub fn with_missing(
    name: Candidate,
    wanted: bool,
    min_label: usize,
) -> impl Iterator<Item = Candidate> {
    let missing = wanted.then(|| missing_letter(&name, min_label)).flatten();
    std::iter::once(name).chain(missing)
}

/// Typos of a whole word that are tried per word, at most. A missing letter
/// comes on top of these.
const MISSPELLINGS: usize = 3;

fn is_vowel(c: char) -> bool {
    "aeiouy".contains(c)
}

/// A small typo of a whole word.
pub struct Misspelling {
    pub text: String,
    pub kind: Kind,
    /// Reads like something a Dutch speaker could have written (apel,
    /// fietz, lenning), as opposed to a slip of the finger (knapp, lenign).
    pub natural: bool,
}

/// Small typos of a whole word, for names that are the word itself, of the
/// kinds that are wanted. The ones that read most like the word come first:
/// a dropped double letter (appel -> apel), a letter that sounds the same
/// (fiets -> fietz, vrijdag -> vrydag), a doubled letter (lening ->
/// lenning) and, as a last resort, the last two letters swapped. Behind
/// those comes the word with a letter missing (lening -> lning).
pub fn misspellings(word: &str, typos: Typos) -> Vec<Misspelling> {
    let chars: Vec<char> = word.chars().collect();
    let n = chars.len();
    let mut out: Vec<Misspelling> = Vec::new();
    let mut push = |letters: Vec<char>, kind: Kind, natural: bool| {
        let text: String = letters.into_iter().collect();
        if typos.allows(kind) && text != word && out.iter().all(|known| known.text != text) {
            out.push(Misspelling {
                text,
                kind,
                natural,
            });
        }
    };
    let replaced = |i: usize, letter: char| {
        let mut letters = chars.clone();
        letters[i] = letter;
        letters
    };

    for i in (1..n).rev() {
        if chars[i] == chars[i - 1] {
            let mut letters = chars.clone();
            letters.remove(i);
            push(letters, Kind::Undoubled, true);
        }
    }

    let before = |i: usize| i.checked_sub(1).map(|j| chars[j]);
    let after = |i: usize| chars.get(i + 1).copied();
    let vowel_or_end = |c: Option<char>| c.is_none_or(is_vowel);
    for i in (0..n).rev() {
        let sounds_like = match chars[i] {
            // The Dutch ij is one sound, and so is the y that replaces it.
            'i' if after(i) == Some('j') => {
                let mut letters = replaced(i, 'y');
                letters.remove(i + 1);
                letters
            }
            // A hard c: not the one in "ch", nor the soft one in "cent". Not
            // the other way around: nobody writes a c for the k of "klok".
            'c' if matches!(after(i), None | Some('a' | 'o' | 'u' | 'l' | 'r' | 't')) => {
                replaced(i, 'k')
            }
            // Only where an s sounds like a z: not in "st", "sch" or "ss".
            's' if vowel_or_end(after(i)) && before(i) != Some('s') => replaced(i, 'z'),
            'z' if after(i).is_some_and(is_vowel) && before(i) != Some('z') => replaced(i, 's'),
            'y' if i > 0 => replaced(i, 'i'),
            _ => continue,
        };
        push(sounds_like, Kind::SoundAlike, true);
    }

    // A double reads like Dutch on a consonant between two vowels
    // (lenning). On the last letter (knapp) it is a slip: no Dutch word ends
    // in a double consonant.
    let between = (1..n.saturating_sub(1))
        .rev()
        .find(|&i| !is_vowel(chars[i]) && is_vowel(chars[i - 1]) && is_vowel(chars[i + 1]));
    let last = n.checked_sub(1).filter(|&i| before(i) != Some(chars[i]));
    if let Some(i) = between.or(last) {
        let mut letters = chars.clone();
        letters.insert(i, chars[i]);
        push(letters, Kind::Doubled, between.is_some());
    }
    if n >= 2 {
        let mut letters = chars.clone();
        letters.swap(n - 2, n - 1);
        push(letters, Kind::Swapped, false);
    }
    out.truncate(MISSPELLINGS);
    let missing = without_a_letter(word, false).filter(|_| typos.allows(Kind::Missing));
    if let Some(text) = missing.filter(|text| out.iter().all(|known| known.text != *text)) {
        out.push(Misspelling {
            text,
            kind: Kind::Missing,
            natural: false,
        });
    }
    out
}

/// A fixed part to go before or after every word of a hunt for whole
/// names: "head" makes headspeed, "box" makes speedbox.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Affix {
    pub prefix: String,
    pub suffix: String,
}

impl Affix {
    pub fn is_empty(&self) -> bool {
        self.prefix.is_empty() && self.suffix.is_empty()
    }

    /// The name a word makes: "speedbox".
    pub fn around(&self, word: &str) -> String {
        format!("{}{word}{}", self.prefix, self.suffix)
    }

    /// The same with its parts apart: "head + speed", "speed + box".
    pub fn spelled_out(&self, word: &str) -> String {
        [self.prefix.as_str(), word, self.suffix.as_str()]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join(" + ")
    }

    /// The fixed parts of a name that was made of `word`, if it has any.
    pub fn of(label: &str, word: &str) -> Option<Self> {
        let (prefix, suffix) = label.split_once(&word.to_lowercase())?;
        let affix = Self {
            prefix: prefix.to_string(),
            suffix: suffix.to_string(),
        };
        (!affix.is_empty() && !word.is_empty()).then_some(affix)
    }
}

/// A whole name under `tld`, if it can be one.
fn whole_name(
    word: &Word,
    tld: &str,
    label: &str,
    (kind, rank): (Kind, u8),
    min_label: usize,
) -> Option<Candidate> {
    let fits = label.len() >= min_label && label.len() <= 63 && is_plain(label);
    fits.then(|| Candidate {
        domain: format!("{label}.{tld}"),
        word: word.shown(),
        label: label.to_string(),
        tld: tld.to_string(),
        kind,
        syllables: word.syllables,
        prevalence: word.prevalence,
        official: word.official,
        topics: word.topic_list(),
        tone: word.tone_list(),
        whole: true,
        rank,
    })
}

/// The word itself as a name under `tld` (lening.nl), followed by its small
/// typos of the kinds that are wanted.
pub fn whole_candidates(word: &Word, tld: &str, typos: Typos, min_label: usize) -> Vec<Candidate> {
    let name =
        |label: &str, kind: Kind, rank: u8| whole_name(word, tld, label, (kind, rank), min_label);
    let mut out: Vec<Candidate> = name(&word.text, Kind::Exact, 0).into_iter().collect();
    // The most natural typo goes with the word itself, as the word is
    // usually taken. The others wait until every word had its turn. A kind
    // that was picked on its own is what the hunt is after, natural or not.
    let picked = matches!(typos, Typos::Only(_));
    let mut lesser = 2;
    let mut best_given = false;
    for typo in misspellings(&word.text, typos) {
        let rank = if (typo.natural || picked) && !best_given {
            best_given = true;
            0
        } else {
            lesser += 1;
            lesser - 1
        };
        out.extend(name(&typo.text, typo.kind, rank));
    }
    out
}

/// The word with a fixed part before or after it as a name under `tld`
/// (speedbox.nl). No typos: the combination is the find.
pub fn combination(word: &Word, tld: &str, affix: &Affix, min_label: usize) -> Option<Candidate> {
    let label = affix.around(&word.text);
    whole_name(word, tld, &label, (Kind::Exact, 0), min_label)
}

/// Every hack a single word allows across the given TLDs (used by `check <word>`).
pub fn candidates_for_word(
    word: &Word,
    tlds: &[String],
    typos: Typos,
    min_label: usize,
) -> Vec<Candidate> {
    let missing = typos.allows(Kind::Missing);
    let mut seen = HashSet::new();
    let mut out: Vec<Candidate> = tlds
        .iter()
        .flat_map(|tld| {
            variants(tld, typos)
                .into_iter()
                .filter_map(move |v| candidate(word, tld, &v, min_label))
                .flat_map(move |name| with_missing(name, missing, min_label))
        })
        .filter(|c| seen.insert(c.domain.clone()))
        .collect();
    out.sort_by_key(|c| (c.kind.is_typo(), c.domain.clone()));
    out
}

/// Only a-z survives: no capitals, spaces, hyphens, dots or diacritics.
pub fn is_plain(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lang::Lang;

    #[test]
    fn the_examples_are_names_a_hunt_makes() {
        for lang in Lang::ALL {
            let examples = lang.examples();
            // The typo is one of the word it is said to be made of.
            let (_, tld) = examples.typo.split_once('.').unwrap();
            let made = candidates_for_word(&word(examples.word), &[tld.to_string()], Typos::All, 2);
            assert!(
                made.iter()
                    .any(|c| c.domain == examples.typo && c.kind.is_typo()),
                "{} is not a typo of {}",
                examples.typo,
                examples.word
            );
            // The hack is a name without one.
            let (label, tld) = examples.hack.split_once('.').unwrap();
            let whole = word(&format!("{label}{tld}"));
            let made = candidates_for_word(&whole, &[tld.to_string()], Typos::Off, 2);
            assert_eq!(made[0].domain, examples.hack);
            // A first name keeps its capital as a word, not in the domain.
            let mike = Word {
                text: "mike".into(),
                name: true,
                tone: vec!["sexual".into(), "vulgar".into()],
                ..Word::default()
            };
            let names = whole_candidates(&mike, "nl", Typos::All, 2);
            assert_eq!(names[0].domain, "mike.nl");
            assert_eq!(
                (names[0].word.as_str(), names[0].tone.as_str()),
                ("Mike", "vulgar, sexual")
            );
            assert!(names.iter().any(|c| c.domain == "mikke.nl" && c.rank == 0));
            let box_ = Affix {
                suffix: "box".into(),
                ..Affix::default()
            };
            let combo = combination(&mike, "nl", &box_, 2).unwrap();
            assert_eq!(
                (combo.domain.as_str(), combo.word.as_str()),
                ("mikebox.nl", "Mike")
            );
            assert_eq!(combo.spelled(), "mikebox");
            assert_eq!(Affix::of(&combo.label, &combo.word), Some(box_.clone()));
            assert_eq!(box_.spelled_out("mike"), "mike + box");
            let head = Affix {
                prefix: "head".into(),
                ..Affix::default()
            };
            assert_eq!(head.around("speed"), "headspeed");
            assert_eq!(head.spelled_out("speed"), "head + speed");
            assert_eq!(Affix::of("speed", "speed"), None);
            assert_eq!(Affix::of("lenning", "lening"), None);
            // And the whole word comes with the typo shown next to it. The
            // Dutch one shows a letter that sounds the same, in another word.
            let (name, tld) = examples.whole.split_once('.').unwrap();
            let source = if lang == Lang::Dutch { "fiets" } else { name };
            let made = whole_candidates(&word(source), tld, Typos::All, 2);
            assert!(
                made.iter()
                    .any(|c| c.domain == examples.whole_typo && c.rank == 0),
                "{} is not the first typo of {source}",
                examples.whole_typo
            );
        }
    }

    fn word(text: &str) -> Word {
        Word {
            text: text.into(),
            syllables: Some(1),
            official: true,
            ..Word::default()
        }
    }

    fn hack(text: &str, tld: &str) -> Vec<(String, Kind)> {
        variants(tld, Typos::All)
            .iter()
            .filter_map(|v| candidate(&word(text), tld, v, 2))
            .map(|c| (c.domain, c.kind))
            .collect()
    }

    #[test]
    fn exact_hacks() {
        assert_eq!(
            hack("portfolio", "io"),
            vec![("portfol.io".to_string(), Kind::Exact)]
        );
        assert!(hack("papegaai", "ai").contains(&("papega.ai".to_string(), Kind::Exact)));
    }

    #[test]
    fn typo_hacks() {
        assert_eq!(
            hack("knap", "app"),
            vec![("kn.app".to_string(), Kind::Doubled)]
        );
        assert!(hack("papegaai", "ai").contains(&("papeg.ai".to_string(), Kind::Undoubled)));
        assert!(hack("tobbe", "be").contains(&("to.be".to_string(), Kind::Undoubled)));
        assert_eq!(
            hack("display", "ai"),
            vec![("displ.ai".to_string(), Kind::SoundAlike)]
        );
        assert_eq!(
            hack("strooi", "io"),
            vec![("stro.io".to_string(), Kind::Swapped)]
        );
    }

    #[test]
    fn no_typos_when_disabled() {
        assert_eq!(variants("app", Typos::Off).len(), 1);
    }

    #[test]
    fn label_rules() {
        // One letter labels are skipped, as is anything that is not plain a-z.
        assert!(hack("haai", "ai").iter().all(|(d, _)| d != "h.ai"));
        assert!(candidate(&word("ca-fe"), "fe", &variants("fe", Typos::Off)[0], 2).is_none());
    }

    #[test]
    fn whole_words_and_their_typos() {
        // The typos that change a letter. The one that leaves a letter out
        // comes behind them, and has a test of its own.
        let changed = |text: &str| -> Vec<Misspelling> {
            let all = misspellings(text, Typos::All).into_iter();
            all.filter(|t| t.kind != Kind::Missing).collect()
        };
        let typos =
            |text: &str| -> Vec<String> { changed(text).into_iter().map(|t| t.text).collect() };
        let natural =
            |text: &str| -> Vec<bool> { changed(text).into_iter().map(|t| t.natural).collect() };
        // A dropped double first, then a sound-alike, then a doubled letter.
        assert_eq!(typos("appel"), vec!["apel", "appell", "apple"]);
        assert_eq!(typos("fiets"), vec!["fietz", "fietss", "fiest"]);
        assert_eq!(typos("lening"), vec!["lenning", "lenign"]);
        assert_eq!(typos("kado"), vec!["kaddo", "kaod"]);
        assert_eq!(typos("cadeau")[0], "kadeau");
        assert_eq!(typos("vrijdag")[0], "vrydag");
        // lenning could be Dutch, a double at the end and the swaps could not.
        assert_eq!(natural("lening"), vec![true, false]);
        assert_eq!(typos("knap"), vec!["knapp", "knpa"]);
        assert_eq!(natural("knap"), vec![false, false]);
        assert_eq!(typos("tand"), vec!["tandd", "tadn"]);
        assert_eq!(natural("tand"), vec![false, false]);
        assert_eq!(typos("papegaai")[0], "papegai");
        assert_eq!(typos("circus")[..2], ["circuz", "cirkus"]);
        // A k never becomes a c.
        assert!(typos("knie").iter().all(|t| !t.contains('c')));
        assert_eq!(typos("keuken")[0], "keukken");
        // No hard c in "school", no z-sound in "kassa" or "stoel".
        assert!(typos("school").iter().all(|t| !t.contains('k')));
        assert!(typos("kassa").iter().all(|t| !t.contains('z')));
        assert!(typos("stoel").iter().all(|t| !t.starts_with('z')));

        let names: Vec<(String, u8, String)> =
            whole_candidates(&word("appel"), "nl", Typos::All, 2)
                .into_iter()
                .map(|c| (c.domain.clone(), c.rank, c.spelled()))
                .collect();
        assert_eq!(
            names,
            vec![
                ("appel.nl".to_string(), 0, "appel".to_string()),
                ("apel.nl".to_string(), 0, "apel".to_string()),
                ("appell.nl".to_string(), 2, "appell".to_string()),
                ("apple.nl".to_string(), 3, "apple".to_string()),
                // The missing letter comes last of all.
                ("appl.nl".to_string(), 4, "appl".to_string()),
            ]
        );
        // A word without a natural typo stands alone in the first rank.
        let ranks: Vec<u8> = whole_candidates(&word("tand"), "nl", Typos::All, 2)
            .into_iter()
            .map(|c| c.rank)
            .collect();
        assert_eq!(ranks, vec![0, 2, 3]);
        // Without typos only the word itself is left.
        assert_eq!(
            whole_candidates(&word("appel"), "nl", Typos::Off, 2).len(),
            1
        );
        // A hack still reads as label plus TLD.
        let hack = candidate(&word("knap"), "app", &variants("app", Typos::All)[1], 2).unwrap();
        assert_eq!((hack.spelled().as_str(), hack.rank), ("knapp", 1));
    }

    #[test]
    fn a_letter_goes_missing() {
        let dotted = |text: &str, tld: &str, typos| -> Vec<(String, Kind)> {
            candidates_for_word(&word(text), &[tld.to_string()], typos, 2)
                .into_iter()
                .map(|c| (c.domain, c.kind))
                .collect()
        };
        let missing = Typos::Only(Kind::Missing);
        // The first letter the word can spare leaves the label: the word
        // keeps its ending. The real name comes first.
        assert_eq!(
            dotted("numbers", "rs", missing),
            vec![
                ("numbe.rs".to_string(), Kind::Exact),
                ("nmbe.rs".to_string(), Kind::Missing)
            ]
        );
        assert_eq!(dotted("evening", "ing", missing)[1].0, "evn.ing");
        // Not one of a double, so here it is the i that goes.
        assert_eq!(dotted("cooking", "ng", missing)[1].0, "cook.ng");
        // Nothing is left to read "mk.ing" by, and "cook" has no letter to
        // spare at all.
        assert_eq!(dotted("making", "ing", missing).len(), 1);
        assert_eq!(dotted("cooking", "ing", missing).len(), 1);
        // It is one of all the typos, and no part of another kind.
        assert!(dotted("numbers", "rs", Typos::All).contains(&("nmbe.rs".into(), Kind::Missing)));
        assert_eq!(dotted("numbers", "rs", Typos::Only(Kind::Swapped)).len(), 1);
        assert_eq!(dotted("numbers", "rs", Typos::Off).len(), 1);
        // Only a real name has one: kn.app is a typo already.
        let knap = candidate(&word("knap"), "app", &variants("app", Typos::All)[1], 2).unwrap();
        assert!(missing_letter(&knap, 2).is_none());
        // And it reads as the word without the letter, last in line.
        let name = missing_letter(&dotted_name("numbers", "rs"), 2).unwrap();
        assert_eq!((name.spelled().as_str(), name.rank), ("nmbers", 3));
        assert_eq!(name.word, "numbers");

        // A whole word loses a letter the same way, but never its last one.
        let whole = |text: &str, typos| -> Vec<(String, Kind)> {
            misspellings(text, typos)
                .into_iter()
                .map(|t| (t.text, t.kind))
                .collect()
        };
        assert_eq!(
            whole("numbers", missing),
            vec![("nmbers".to_string(), Kind::Missing)]
        );
        assert_eq!(whole("lening", missing)[0].0, "lning");
        assert_eq!(whole("kado", missing)[0].0, "kdo");
        // Not the first letter (apple), not its only vowel (knap, tand), and
        // not one of a double (boot).
        for none in ["apple", "knap", "tand", "boot"] {
            assert!(whole(none, missing).is_empty(), "{none}");
        }
        // Among all typos it comes on top of the three that change a letter.
        let all = whole("numbers", Typos::All);
        assert_eq!(all.len(), 4);
        assert_eq!(all[3], ("nmbers".to_string(), Kind::Missing));
        assert!(
            misspellings("numbers", Typos::All)
                .iter()
                .all(|t| !t.natural || t.kind != Kind::Missing)
        );
        // One kind is that kind alone, and off is nothing.
        assert_eq!(
            whole("numbers", Typos::Only(Kind::SoundAlike)),
            vec![("numberz".to_string(), Kind::SoundAlike)]
        );
        assert!(whole("numbers", Typos::Off).is_empty());
        // Among all typos the name with a letter missing waits its turn.
        // Picked on its own it goes with the word, as the best typo does.
        let ranks = |typos| -> Vec<(String, u8)> {
            whole_candidates(&word("numbers"), "nl", typos, 2)
                .into_iter()
                .map(|c| (c.domain, c.rank))
                .collect()
        };
        assert_eq!(
            ranks(Typos::All).last().unwrap(),
            &("nmbers.nl".to_string(), 4)
        );
        assert_eq!(
            ranks(missing),
            vec![("numbers.nl".to_string(), 0), ("nmbers.nl".to_string(), 0)]
        );
    }

    fn dotted_name(text: &str, tld: &str) -> Candidate {
        candidate(&word(text), tld, &variants(tld, Typos::Off)[0], 2).unwrap()
    }

    #[test]
    fn every_kind_of_typo_can_be_picked_on_its_own() {
        for kind in Kind::TYPOS {
            let only = Typos::Only(kind);
            // By its word, by its full name, and however it is typed.
            assert_eq!(Typos::parse(kind.word()), Some(only));
            assert_eq!(Typos::parse(kind.as_str()), Some(only));
            assert_eq!(
                Typos::parse(&format!(" {} ", kind.word().to_uppercase())),
                Some(only)
            );
            assert!(Typos::CHOICES.contains(kind.word()), "{}", kind.word());
            assert!(only.allows(kind) && Typos::All.allows(kind) && !Typos::Off.allows(kind));
            assert!(
                Kind::TYPOS
                    .iter()
                    .all(|other| *other == kind || !only.allows(*other))
            );

            // Its example is a name it makes, with the dot inside the word
            // and as a whole word, and no other kind makes it.
            let (text, dotted, reads) = kind.example();
            let (_, tld) = dotted.split_once('.').unwrap();
            for typos in [only, Typos::All] {
                let made = candidates_for_word(&word(text), &[tld.to_string()], typos, 2);
                assert!(
                    made.iter().any(|c| c.domain == dotted && c.kind == kind),
                    "{dotted} is not a {} of {text}",
                    kind.as_str()
                );
                let whole = misspellings(text, typos);
                assert!(
                    whole.iter().any(|t| t.text == reads && t.kind == kind),
                    "{reads} is not a {} of {text}",
                    kind.as_str()
                );
            }
            let made = candidates_for_word(&word(text), &[tld.to_string()], only, 2);
            assert!(made.iter().all(|c| c.kind == kind || c.kind == Kind::Exact));
            assert!(misspellings(text, only).iter().all(|t| t.kind == kind));
        }
        assert_eq!(Typos::parse("all"), Some(Typos::All));
        assert_eq!(Typos::parse("On"), Some(Typos::All));
        assert_eq!(Typos::parse("off"), Some(Typos::Off));
        assert_eq!(Typos::parse("left-out"), Some(Typos::Only(Kind::Missing)));
        for nonsense in ["", "maybe", "exact", "typos"] {
            assert_eq!(Typos::parse(nonsense), None, "{nonsense}");
        }
        assert!(!Typos::All.allows(Kind::Exact) && Typos::default() == Typos::All);
        assert!(Typos::Off.is_off() && !Typos::All.is_off());
    }

    #[test]
    fn variants_are_unique() {
        for tld in ["ai", "io", "app", "cc", "nl", "ooo"] {
            let v = variants(tld, Typos::All);
            let unique: HashSet<_> = v.iter().map(|v| &v.suffix).collect();
            assert_eq!(unique.len(), v.len(), "{tld}");
            assert_eq!(v[0].kind, Kind::Exact);
        }
    }
}
