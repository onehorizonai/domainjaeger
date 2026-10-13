//! Categories: what a word is about.
//!
//! WikiWoordenboek files Dutch words under some 250 topics ("Financieel",
//! "Informatica", "Muziek", ...), and the English Wiktionary does the same
//! with topics of its own ("Finance", "Computing", "Music"), for the words
//! of every language it describes. A category is a handful of those topics
//! under a friendlier name; any single topic can be used as a category too.
//! The words themselves are still fetched live, only the bundles are ours.

use std::collections::HashMap;

use anyhow::{Result, bail};

use crate::cache::Record;
use crate::check::Status;
use crate::lang::{Lang, Wiki};

/// The one category that is not a subject: the first names of a language.
/// The English Wiktionary lists them, for every language: see `wiktionary`.
pub const NAMES: &str = "Given names";

pub struct Bundle {
    pub name: &'static str,
    /// A sign to know it by, and its colour (256 colours). A plain one that
    /// every font has: see `ui::figure`.
    pub mark: &'static str,
    pub color: u8,
    pub about: &'static str,
    /// WikiWoordenboek topics, as in "Categorie:<topic> in het Nederlands".
    dutch: &'static [&'static str],
    /// Wiktionary topics, as in "Category:en:<topic>". Latin and Spanish
    /// words are filed under the same names ("Category:la:<topic>").
    english: &'static [&'static str],
}

impl Bundle {
    pub fn topics(&self, lang: Lang) -> &'static [&'static str] {
        match lang.wiki() {
            Wiki::WikiWoordenboek => self.dutch,
            Wiki::Wiktionary => self.english,
        }
    }

    fn has(&self, topic: &str) -> bool {
        self.dutch.contains(&topic) || self.english.contains(&topic)
    }
}

/// The built-in categories. Every word of a hunt is labelled with these
/// topics, and for Dutch each one takes the place of a prevalence value in
/// the dictionary request (50 filters at most), so keep the total modest.
pub const BUNDLES: &[Bundle] = &[
    Bundle {
        name: "finance",
        mark: "$",
        color: 220,
        about: "money, markets and bookkeeping",
        dutch: &["Financieel", "Economie", "Boekhouding", "Valutanaam"],
        english: &[
            "Finance",
            "Banking",
            "Accounting",
            "Money",
            "Currencies",
            "Economics",
            "Taxation",
            "Insurance",
        ],
    },
    Bundle {
        name: "tech",
        mark: ">",
        color: 208,
        about: "computers, electronics and engineering",
        dutch: &[
            "Informatica",
            "Techniek",
            "Elektronica",
            "Elektrotechniek",
            "Telecommunicatie",
            "Internettaal",
        ],
        english: &[
            "Computing",
            "Internet",
            "Programming",
            "Electronics",
            "Engineering",
            "Software",
            "Telecommunications",
            "Technology",
        ],
    },
    Bundle {
        name: "product",
        mark: "#",
        color: 214,
        about: "things people make, sell and use",
        dutch: &[
            "Handel",
            "Industrie",
            "Gereedschap",
            "Huishouden",
            "Kleding",
            "Meubel",
            "Speelgoed",
        ],
        english: &[
            "Business",
            "Marketing",
            "Manufacturing",
            "Tools",
            "Containers",
            "Clothing",
            "Furniture",
            "Toys",
        ],
    },
    Bundle {
        name: "names",
        mark: "@",
        color: 226,
        about: "first names, like Mike, Emma and Lucas",
        dutch: &[NAMES],
        english: &[NAMES],
    },
];
/// Marks a topic that is not part of a built-in category.
const PLAIN_MARK: (&str, u8) = ("•", 244);

/// What a hunt is limited to: one or more topics under a display name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Category {
    pub name: String,
    pub topics: Vec<String>,
}

/// The topics every word of a language is labelled with. A first name is
/// known by the list it came from, not by a topic.
pub fn tracked(lang: Lang) -> Vec<&'static str> {
    BUNDLES
        .iter()
        .flat_map(|b| b.topics(lang).iter().copied())
        .filter(|topic| *topic != NAMES)
        .collect()
}

/// The category page of a topic in the dictionary of a language.
pub fn title(lang: Lang, topic: &str) -> String {
    match lang.wiki() {
        Wiki::WikiWoordenboek => format!("Categorie:{topic} in het Nederlands"),
        Wiki::Wiktionary => format!("Category:{}:{topic}", lang.code()),
    }
}

/// The topic of a category page.
pub fn topic_of(lang: Lang, title: &str) -> Option<&str> {
    match lang.wiki() {
        Wiki::WikiWoordenboek => title
            .strip_prefix("Categorie:")?
            .strip_suffix(" in het Nederlands"),
        Wiki::Wiktionary => title
            .strip_prefix("Category:")?
            .strip_prefix(lang.code())?
            .strip_prefix(':'),
    }
}

/// The built-in category a topic is part of, if any.
pub fn bundle_of(topic: &str) -> Option<&'static Bundle> {
    BUNDLES.iter().find(|b| b.has(topic))
}

fn bundle(name: &str) -> Option<&'static Bundle> {
    BUNDLES.iter().find(|b| b.name.eq_ignore_ascii_case(name))
}

/// Does this selection need the list of topics from the dictionary, or is
/// it made of built-in categories only?
pub fn is_built_in(input: &str) -> bool {
    let mut names = input
        .split(|c: char| c == ',' || c == '+' || c.is_whitespace())
        .filter(|s| !s.is_empty())
        .peekable();
    names.peek().is_some() && names.all(|name| bundle(name).is_some())
}

/// A name as it is compared: capitals and extra spaces do not matter, so
/// " Animal  sounds" is "animal sounds".
fn plain(name: &str) -> String {
    let words: Vec<String> = name.split_whitespace().map(str::to_lowercase).collect();
    words.join(" ")
}

/// Turn "finance", "tech+muziek" or "Chemisch element" into a category.
/// `known` are the topics the dictionary has, in its own spelling. Capitals
/// and spaces do not matter, and a topic is found with or without its last
/// s: "animal" is Animals.
pub fn resolve(lang: Lang, input: &str, known: &[String]) -> Result<Category> {
    let find = |name: &str| -> Option<(String, Vec<String>)> {
        if let Some(b) = bundle(name.trim()) {
            let topics = b.topics(lang).iter().map(|t| t.to_string()).collect();
            return Some((b.name.to_string(), topics));
        }
        let wanted = plain(name);
        let one = |name: &str| name.strip_suffix('s').unwrap_or(name).to_string();
        let exact = known.iter().find(|t| plain(t) == wanted);
        exact
            .or_else(|| known.iter().find(|t| one(&plain(t)) == one(&wanted)))
            .map(|t| (t.to_lowercase(), vec![t.clone()]))
    };

    let mut names: Vec<String> = Vec::new();
    let mut topics: Vec<String> = Vec::new();
    let mut add = |(name, found): (String, Vec<String>)| {
        if !names.contains(&name) {
            names.push(name);
        }
        for topic in found {
            if !topics.contains(&topic) {
                topics.push(topic);
            }
        }
    };
    for part in input
        .split([',', '+'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        // "chemisch element" is one topic, "finance tech" are two categories.
        if let Some(found) = find(part) {
            add(found);
            continue;
        }
        for word in part.split_whitespace() {
            match find(word) {
                Some(found) => add(found),
                None => bail!("{}", unknown(lang, word, known)),
            }
        }
    }
    if topics.is_empty() {
        bail!("no category given");
    }
    Ok(Category {
        name: names.join("+"),
        topics,
    })
}

fn unknown(lang: Lang, name: &str, known: &[String]) -> String {
    let lower = name.to_lowercase();
    let stem: String = lower.chars().take(4).collect();
    let close: Vec<&str> = known
        .iter()
        .filter(|t| {
            let t = t.to_lowercase();
            t.contains(&lower) || t.starts_with(&stem)
        })
        .take(5)
        .map(String::as_str)
        .collect();
    let hint = if close.is_empty() {
        "`cat list` shows every topic".to_string()
    } else {
        format!("did you mean {}?", close.join(", "))
    };
    format!(
        "\"{name}\" is not a category or a {} topic, {hint}",
        lang.dictionary()
    )
}

/// The categories a word belongs to, given its topics ("Economie, Muziek"
/// becomes finance and muziek). Built-in categories come first. The topics
/// of the two dictionaries are told apart by their names.
pub fn labels(topics: &str) -> Vec<String> {
    let topics: Vec<&str> = topics.split(',').map(str::trim).collect();
    let mut out: Vec<String> = BUNDLES
        .iter()
        .filter(|b| topics.iter().any(|t| b.has(t)))
        .map(|b| b.name.to_string())
        .collect();
    for topic in topics {
        let plain = !topic.is_empty() && BUNDLES.iter().all(|b| !b.has(topic));
        if plain && !out.contains(&topic.to_lowercase()) {
            out.push(topic.to_lowercase());
        }
    }
    out
}

/// The mark of a category and its colour.
pub fn mark(label: &str) -> (&'static str, u8) {
    bundle(label).map_or(PLAIN_MARK, |b| (b.mark, b.color))
}

/// Does a record belong to the category called `name` (or to that topic)?
pub fn matches(record: &Record, name: &str) -> bool {
    let topics = record.topics.as_deref().unwrap_or_default();
    labels(topics)
        .iter()
        .any(|label| label.eq_ignore_ascii_case(name))
        || topics
            .split(',')
            .any(|topic| topic.trim().eq_ignore_ascii_case(name))
}

/// Per-category tallies for the stats screen: `(label, checked, available)`.
pub fn tally(records: &[Record]) -> Vec<(String, usize, usize)> {
    let mut map: HashMap<String, (usize, usize)> = HashMap::new();
    for r in records {
        for label in labels(r.topics.as_deref().unwrap_or_default()) {
            let entry = map.entry(label).or_default();
            entry.0 += 1;
            if r.status == Status::Available {
                entry.1 += 1;
            }
        }
    }
    let mut rows: Vec<(String, usize, usize)> = map
        .into_iter()
        .map(|(label, (checked, free))| (label, checked, free))
        .collect();
    rows.sort_by(|a, b| b.2.cmp(&a.2).then(b.1.cmp(&a.1)).then(a.0.cmp(&b.0)));
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn known() -> Vec<String> {
        [
            "Muziek",
            "Muziekinstrument",
            "Chemisch element",
            "Economie",
            "Animals",
            "Animal sounds",
            "Glass",
        ]
        .map(String::from)
        .to_vec()
    }

    #[test]
    fn built_in_categories_need_no_lookup() {
        assert!(is_built_in("finance"));
        assert!(is_built_in("Finance, tech"));
        assert!(!is_built_in("finance muziek"));
        assert!(!is_built_in(""));
        let finance = resolve(Lang::Dutch, "finance", &[]).unwrap();
        assert_eq!(finance.name, "finance");
        assert!(finance.topics.contains(&"Financieel".to_string()));
        // The same category is made of other topics in English.
        let finance = resolve(Lang::English, "finance", &[]).unwrap();
        assert_eq!(finance.name, "finance");
        assert!(finance.topics.contains(&"Banking".to_string()));
        assert!(!finance.topics.contains(&"Financieel".to_string()));
    }

    #[test]
    fn topics_and_combinations() {
        let both = resolve(Lang::Dutch, "tech+muziek", &known()).unwrap();
        assert_eq!(both.name, "tech+muziek");
        assert!(both.topics.contains(&"Informatica".to_string()));
        assert!(both.topics.contains(&"Muziek".to_string()));
        // A topic of two words stays whole, two categories are split.
        let element = resolve(Lang::Dutch, "chemisch element", &known()).unwrap();
        assert_eq!(element.topics, vec!["Chemisch element".to_string()]);
        let two = resolve(Lang::Dutch, "finance tech", &[]).unwrap();
        assert_eq!(two.name, "finance+tech");
        // Economie is already part of finance and is not listed twice.
        let finance = resolve(Lang::Dutch, "finance, economie", &known()).unwrap();
        let economie = finance.topics.iter().filter(|t| *t == "Economie").count();
        assert_eq!(economie, 1);
    }

    #[test]
    fn capitals_spaces_and_a_last_s_do_not_matter() {
        let topics = |input: &str| resolve(Lang::English, input, &known()).unwrap().topics;
        for typed in [
            "Animals",
            "animals",
            " ANIMALS ",
            "animal",
            "Animal",
            "animal ",
        ] {
            assert_eq!(topics(typed), vec!["Animals".to_string()], "{typed:?}");
        }
        // A topic of two words, however far apart they were typed.
        for typed in ["animal sounds", "Animal  Sounds", "\tanimal sound "] {
            assert_eq!(
                topics(typed),
                vec!["Animal sounds".to_string()],
                "{typed:?}"
            );
        }
        // The topic that is spelled the same goes first, and a word that
        // ends in s for another reason is still found.
        assert_eq!(topics("glass"), vec!["Glass".to_string()]);
        assert_eq!(topics("muziek"), vec!["Muziek".to_string()]);
        // The categories of our own, too.
        assert_eq!(topics(" Finance ")[0], "Finance");
        assert!(is_built_in("finance\ttech") && is_built_in(" Names "));
        // What is not there is still said so.
        assert!(resolve(Lang::English, "anima", &known()).is_err());
    }

    #[test]
    fn unknown_topics_get_a_suggestion() {
        let error = resolve(Lang::Dutch, "muzieek", &known())
            .unwrap_err()
            .to_string();
        assert!(error.contains("did you mean Muziek"), "{error}");
        assert!(resolve(Lang::Dutch, "", &known()).is_err());
    }

    #[test]
    fn topics_become_labels() {
        assert_eq!(labels("Economie, Financieel"), vec!["finance"]);
        assert_eq!(
            labels("Muziek, Informatica, Handel"),
            vec!["tech", "product", "muziek"]
        );
        assert!(labels("").is_empty());
        // English topics end up in the same categories.
        assert_eq!(labels("Banking, Computing"), vec!["finance", "tech"]);
        assert_eq!(labels("Music"), vec!["music"]);
        assert_eq!(mark("finance"), ("$", 220));
        // A sign of its own for every category, and one any font can draw.
        for (i, bundle) in BUNDLES.iter().enumerate() {
            assert!(bundle.mark.is_ascii() && bundle.mark.len() == 1);
            assert!(BUNDLES[..i].iter().all(|other| other.mark != bundle.mark));
        }
        // First names are a category in every language, not a topic to ask
        // the dictionary about.
        assert_eq!(labels(NAMES), vec!["names"]);
        for lang in Lang::ALL {
            assert_eq!(resolve(lang, "names", &[]).unwrap().topics, vec![NAMES]);
            assert!(!tracked(lang).contains(&NAMES));
        }
        assert!(is_built_in("names tech"));
        assert_eq!(mark("muziek"), PLAIN_MARK);
    }

    #[test]
    fn titles_round_trip() {
        for lang in Lang::ALL {
            for topic in tracked(lang) {
                assert_eq!(topic_of(lang, &title(lang, topic)), Some(topic));
            }
        }
        assert_eq!(title(Lang::English, "Finance"), "Category:en:Finance");
        assert_eq!(title(Lang::Latin, "Law"), "Category:la:Law");
        assert_eq!(topic_of(Lang::Spanish, "Category:es:Music"), Some("Music"));
        // The topics of another language are not ours.
        assert_eq!(topic_of(Lang::Spanish, "Category:en:Music"), None);
        assert_eq!(topic_of(Lang::English, "Category:es:Music"), None);
        let word_list = "Categorie:Woordenlijst Nederlandse Taal";
        assert_eq!(topic_of(Lang::Dutch, word_list), None);
        assert_eq!(topic_of(Lang::English, "Category:English lemmas"), None);
        // Fits next to the syllable, word list and prevalence filters.
        assert!(tracked(Lang::Dutch).len() <= 20);
        // No topic is in two categories, in any language.
        for lang in Lang::ALL {
            let mut all = tracked(lang);
            all.sort();
            let total = all.len();
            all.dedup();
            assert_eq!(all.len(), total);
        }
    }
}
