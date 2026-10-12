//! The languages words can come from. Dutch words are read from
//! WikiWoordenboek (nl.wiktionary.org). English, Latin and Spanish words
//! come from the English Wiktionary, which describes the words of every
//! language and is organised differently: see `wiktionary`.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Lang {
    #[default]
    English,
    Dutch,
    Latin,
    Spanish,
}

/// The two dictionaries words are read from. They are organised differently.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Wiki {
    /// nl.wiktionary.org: Dutch words, described in Dutch.
    WikiWoordenboek,
    /// en.wiktionary.org: the words of every language, described in English.
    Wiktionary,
}

impl Wiki {
    /// What the dictionary calls itself.
    pub fn name(self) -> &'static str {
        match self {
            Wiki::WikiWoordenboek => "WikiWoordenboek",
            Wiki::Wiktionary => "Wiktionary",
        }
    }

    pub fn api(self) -> &'static str {
        match self {
            Wiki::WikiWoordenboek => "https://nl.wiktionary.org/w/api.php",
            Wiki::Wiktionary => "https://en.wiktionary.org/w/api.php",
        }
    }
}

/// Names that show what a hunt in a language is after.
pub struct Examples {
    /// Three names for the title: label and TLD.
    pub names: [(&'static str, &'static str); 3],
    /// A name where the TLD ends the word.
    pub hack: &'static str,
    /// The same with a small typo, and the word it is made of.
    pub typo: &'static str,
    pub word: &'static str,
    /// A whole word as a name, and a small typo of it.
    pub whole: &'static str,
    pub whole_typo: &'static str,
    /// A topic of the dictionary.
    pub topic: &'static str,
    pub goodbye: &'static str,
}

impl Lang {
    /// English first: it is what a hunt starts in. Dutch is second.
    pub const ALL: [Lang; 4] = [Lang::English, Lang::Dutch, Lang::Latin, Lang::Spanish];

    /// The languages a hunt can switch to from this one.
    pub fn others(self) -> impl Iterator<Item = Lang> {
        Self::ALL.into_iter().filter(move |lang| *lang != self)
    }

    pub fn examples(self) -> Examples {
        match self {
            Lang::English => Examples {
                names: [("cook", "ing"), ("sn", "app"), ("tod", "ai")],
                hack: "cook.ing",
                typo: "sn.app",
                word: "snap",
                whole: "lemon.nl",
                whole_typo: "lemmon.nl",
                topic: "music",
                goodbye: "See you!",
            },
            Lang::Dutch => Examples {
                names: [("papega", "ai"), ("kn", "app"), ("leni", "ng")],
                hack: "leni.ng",
                typo: "kn.app",
                word: "knap",
                whole: "lening.nl",
                whole_typo: "fietz.nl",
                topic: "muziek",
                goodbye: "Tot ziens!",
            },
            Lang::Latin => Examples {
                names: [("lu", "men"), ("rat", "io"), ("ste", "la")],
                hack: "lu.men",
                typo: "ste.la",
                word: "stella",
                whole: "luna.nl",
                whole_typo: "lunna.nl",
                topic: "law",
                goodbye: "Vale!",
            },
            Lang::Spanish => Examples {
                names: [("verd", "ad"), ("via", "je"), ("estre", "la")],
                hack: "verd.ad",
                typo: "estre.la",
                word: "estrella",
                whole: "amigo.nl",
                whole_typo: "amiggo.nl",
                topic: "music",
                goodbye: "¡Hasta luego!",
            },
        }
    }

    /// Short code, the way the language is stored with a checked domain. It
    /// is also what the English Wiktionary files topics under ("la:Law").
    pub fn code(self) -> &'static str {
        match self {
            Lang::English => "en",
            Lang::Dutch => "nl",
            Lang::Latin => "la",
            Lang::Spanish => "es",
        }
    }

    /// The English name, which is also what the English Wiktionary calls
    /// the language ("Latin 2-syllable words").
    pub fn name(self) -> &'static str {
        match self {
            Lang::English => "English",
            Lang::Dutch => "Dutch",
            Lang::Latin => "Latin",
            Lang::Spanish => "Spanish",
        }
    }

    /// The dictionary the words of this language are read from.
    pub fn wiki(self) -> Wiki {
        match self {
            Lang::Dutch => Wiki::WikiWoordenboek,
            Lang::English | Lang::Latin | Lang::Spanish => Wiki::Wiktionary,
        }
    }

    /// What the dictionary of this language calls itself.
    pub fn dictionary(self) -> &'static str {
        self.wiki().name()
    }

    /// Only the form a dictionary lists a word under. Latin and Spanish
    /// words come in dozens of forms (amo, amas, amat) that few would
    /// recognise as a name. English is hunted with its forms: cooking is
    /// one of them.
    pub fn lemmas_only(self) -> bool {
        matches!(self, Lang::Latin | Lang::Spanish)
    }

    /// "es", "Spanish" or "Spaans": whatever people type.
    pub fn parse(text: &str) -> Option<Self> {
        match text.trim().to_lowercase().as_str() {
            "en" | "english" | "engels" => Some(Lang::English),
            "nl" | "dutch" | "nederlands" => Some(Lang::Dutch),
            "la" | "latin" | "latijn" | "latina" => Some(Lang::Latin),
            "es" | "spanish" | "spaans" | "espanol" | "español" => Some(Lang::Spanish),
            _ => None,
        }
    }

    /// "en (English), nl (Dutch), ...", for when a language is not known.
    pub fn choices() -> String {
        Self::ALL
            .map(|lang| format!("{} ({})", lang.code(), lang.name()))
            .join(", ")
    }

    /// The language a stored code stands for.
    pub fn from_code(code: &str) -> Self {
        Self::parse(code).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn languages_are_recognised() {
        assert_eq!(Lang::parse("EN"), Some(Lang::English));
        assert_eq!(Lang::parse(" engels "), Some(Lang::English));
        assert_eq!(Lang::parse("nederlands"), Some(Lang::Dutch));
        assert_eq!(Lang::parse("Latijn"), Some(Lang::Latin));
        assert_eq!(Lang::parse("español"), Some(Lang::Spanish));
        assert_eq!(Lang::parse("fr"), None);
        for lang in Lang::ALL {
            assert_eq!(Lang::from_code(lang.code()), lang);
            assert_eq!(Lang::parse(lang.name()), Some(lang));
            assert_eq!(lang.others().count(), Lang::ALL.len() - 1);
            assert!(lang.others().all(|other| other != lang));
        }
        // A hunt starts in English, and so does a code nobody knows.
        assert_eq!(Lang::default(), Lang::English);
        assert_eq!(Lang::ALL[..2], [Lang::English, Lang::Dutch]);
        assert_eq!(Lang::from_code("xx"), Lang::English);
        assert_eq!(
            Lang::choices(),
            "en (English), nl (Dutch), la (Latin), es (Spanish)"
        );
    }
}
