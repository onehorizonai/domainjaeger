//! What a word may bring along besides its meaning: the dictionaries label
//! vulgar and offensive words, and file words under subjects like sex and
//! violence. A word is flagged when one of its senses is, so a flag says
//! "look twice", not "this word is rude": a cock is also a rooster.

use crate::lang::{Lang, Wiki};

pub const VULGAR: &str = "vulgar";
pub const OFFENSIVE: &str = "offensive";
pub const SEXUAL: &str = "sexual";
pub const VIOLENT: &str = "violent";
/// Every flag, in the order they are shown.
pub const FLAGS: [&str; 4] = [VULGAR, OFFENSIVE, SEXUAL, VIOLENT];

/// The category pages of a dictionary that flag a word of `lang`.
pub fn categories(wiki: Wiki, lang: Lang) -> Vec<(String, &'static str)> {
    match wiki {
        // Swear words, expletives and whatever is labelled vulgar, plus one
        // subject. WikiWoordenboek has no subject for violence.
        Wiki::WikiWoordenboek => [
            ("Scheldwoord", VULGAR),
            ("Vulgair", VULGAR),
            ("Krachtterm", VULGAR),
            ("Seksualiteit", SEXUAL),
        ]
        .into_iter()
        .map(|(topic, flag)| (format!("Categorie:{topic} in het Nederlands"), flag))
        .collect(),
        Wiki::Wiktionary => {
            let labels = [
                ("vulgarities", VULGAR),
                ("swear words", VULGAR),
                ("offensive terms", OFFENSIVE),
                ("ethnic slurs", OFFENSIVE),
            ];
            let subjects = [
                ("Sex", SEXUAL),
                ("Genitalia", SEXUAL),
                ("Prostitution", SEXUAL),
                ("Pornography", SEXUAL),
                ("Violence", VIOLENT),
                ("Murder", VIOLENT),
            ];
            let (name, code) = (lang.name(), lang.code());
            labels
                .into_iter()
                .map(|(label, flag)| (format!("Category:{name} {label}"), flag))
                .chain(
                    subjects
                        .into_iter()
                        .map(|(subject, flag)| (format!("Category:{code}:{subject}"), flag)),
                )
                .collect()
        }
    }
}

/// The flag a category page gives a word of `lang`, if any.
pub fn flag_of(wiki: Wiki, lang: Lang, category: &str) -> Option<&'static str> {
    categories(wiki, lang)
        .into_iter()
        .find(|(title, _)| title == category)
        .map(|(_, flag)| flag)
}

/// "vulgar, sexual": the flags of a word in the order they are shown.
pub fn list(flags: &[&str]) -> String {
    FLAGS
        .into_iter()
        .filter(|flag| flags.contains(flag))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn categories_become_flags() {
        let english = |category| flag_of(Wiki::Wiktionary, Lang::English, category);
        assert_eq!(english("Category:English vulgarities"), Some(VULGAR));
        assert_eq!(english("Category:English ethnic slurs"), Some(OFFENSIVE));
        assert_eq!(english("Category:en:Sex"), Some(SEXUAL));
        assert_eq!(english("Category:en:Murder"), Some(VIOLENT));
        // Not ours: another language, and a subject that is not a flag.
        assert_eq!(english("Category:Spanish vulgarities"), None);
        assert_eq!(english("Category:en:Music"), None);
        // Dutch names are read from the English Wiktionary as well.
        assert_eq!(
            flag_of(Wiki::Wiktionary, Lang::Dutch, "Category:nl:Sex"),
            Some(SEXUAL)
        );
        assert_eq!(
            flag_of(
                Wiki::WikiWoordenboek,
                Lang::Dutch,
                "Categorie:Scheldwoord in het Nederlands"
            ),
            Some(VULGAR)
        );
        assert_eq!(list(&[SEXUAL, VULGAR, SEXUAL]), "vulgar, sexual");
        assert_eq!(list(&[]), "");
    }
}
