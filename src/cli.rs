//! The command line: what can be asked for when the tool is started.

use std::path::PathBuf;

use clap::{Parser, Subcommand};

use crate::hacks::Typos;
use crate::lang::Lang;

#[derive(Parser)]
#[command(
    name = "domainjaeger",
    version,
    about = "Find available domain names made of real English, Dutch, Latin or Spanish words (cook.ing, sn.app, tod.ai)"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,

    /// Domain extensions to search in, e.g. "ai,io,app" [default: every
    /// short public one]
    #[arg(long, global = true)]
    pub tlds: Option<String>,

    /// Available names to find per round
    #[arg(long, short = 'n', default_value_t = 10, global = true)]
    pub count: usize,

    /// Only words about this: finance, tech, product, or any topic of the
    /// dictionary such as music. Several at once: "finance,music". Or
    /// "names" for first names (Mike, Emma)
    #[arg(long, visible_alias = "cat", global = true)]
    pub category: Option<String>,

    /// The language of the words: en (English), nl (Dutch), la (Latin) or
    /// es (Spanish)
    #[arg(long, visible_alias = "language", default_value = "en", global = true,
          value_parser = parse_lang)]
    pub lang: Lang,

    /// How long a word may be, counted in the parts you say it in
    /// (syllables): cat is 1, le-mon 2, ba-na-na 3
    #[arg(long, default_value = "1,2,3", value_delimiter = ',', global = true,
          value_parser = clap::value_parser!(u8).range(1..=5))]
    pub syllables: Vec<u8>,

    /// The small spelling mistakes to make names with: all, off, or one
    /// kind. The kinds are doubled (sn.app), dropped (especia.ly), sound
    /// (tod.ai), swapped (delicio.su) and missing (nmbe.rs)
    #[arg(long, default_value = "all", global = true, value_parser = parse_typos,
          conflicts_with = "no_typos")]
    pub typos: Typos,

    /// Only correctly spelled names (cook.ing): the same as --typos off
    #[arg(long, global = true)]
    pub no_typos: bool,

    /// The whole word before the dot (lemon.nl, and lemmon.nl with a small
    /// spelling mistake) instead of the dot inside the word (cook.ing). In
    /// .nl, unless --tlds says otherwise
    #[arg(long, global = true)]
    pub whole: bool,

    /// Skip words the dictionary flags as vulgar, offensive, sexual or
    /// violent. Otherwise they are marked
    #[arg(long, global = true)]
    pub no_rude: bool,

    /// Put this before every word: "head" makes headspeed. For whole names,
    /// so it implies --whole
    #[arg(long, global = true)]
    pub prefix: Option<String>,

    /// Put this after every word: "box" makes speedbox
    #[arg(long, global = true)]
    pub suffix: Option<String>,

    /// Shortest allowed part before the dot
    #[arg(long, default_value_t = 2, global = true)]
    pub min_label: usize,

    /// Give up on a round after this many lookups
    #[arg(long, default_value_t = 150, global = true)]
    pub max_checks: usize,

    /// Also show taken names as they are checked
    #[arg(long, short, global = true)]
    pub verbose: bool,

    /// Where checked domains are remembered (for 4 weeks)
    /// [default: ~/.domainjaeger/cache.sqlite]
    #[arg(long, env = "DOMAINJAEGER_DB", global = true)]
    pub db: Option<PathBuf>,

    /// Your e-mail address or URL, sent to the dictionary in the User-Agent.
    /// Identified clients get 200 requests a minute instead of 10. Remembered.
    #[arg(long, env = "DOMAINJAEGER_CONTACT", global = true)]
    pub contact: Option<String>,
}

#[derive(Subcommand)]
pub enum Command {
    /// Hunt for names in an interactive shell (the default)
    Hunt {
        /// Run this many rounds and exit instead of opening the shell
        #[arg(long)]
        rounds: Option<usize>,
        /// Write the Excel file when done
        #[arg(long, requires = "rounds")]
        export: Option<PathBuf>,
    },
    /// Write every checked domain to an Excel file
    Export {
        /// Defaults to domainjaeger-<date>.xlsx
        path: Option<PathBuf>,
    },
    /// Try your own words, e.g. `domainjaeger check snap today`
    Check { words: Vec<String> },
    /// Re-verify names that were checked more than 4 weeks ago
    Refresh,
    /// Show what is in the cache
    Stats,
}

fn parse_lang(text: &str) -> Result<Lang, String> {
    Lang::parse(text).ok_or_else(|| format!("pick one of {}", Lang::choices()))
}

fn parse_typos(text: &str) -> Result<Typos, String> {
    Typos::parse(text).ok_or_else(|| format!("pick one of {}", Typos::CHOICES))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(arguments: &[&str]) -> Result<Cli, clap::Error> {
        Cli::try_parse_from(std::iter::once("domainjaeger").chain(arguments.iter().copied()))
    }

    #[test]
    fn nothing_on_the_command_line_is_a_hunt_as_it_comes() {
        let cli = parse(&[]).unwrap();
        assert!(cli.command.is_none());
        assert_eq!(cli.lang, Lang::English);
        assert_eq!((cli.count, cli.syllables.as_slice()), (10, &[1, 2, 3][..]));
        assert!(!cli.whole && !cli.no_typos && !cli.no_rude && !cli.verbose);
        assert_eq!(cli.typos, Typos::All);
        assert!(cli.tlds.is_none() && cli.category.is_none() && cli.db.is_none());
    }

    #[test]
    fn every_setting_has_a_flag() {
        let cli = parse(&[
            "--lang",
            "Spaans",
            "--tlds",
            "ai,io",
            "--count",
            "20",
            "--syllables",
            "1,2",
            "--category",
            "finance",
            "--whole",
            "--no-typos",
            "--no-rude",
            "--prefix",
            "head",
            "--suffix",
            "box",
            "--db",
            "/tmp/x.sqlite",
            "--contact",
            "me@example.org",
        ])
        .unwrap();
        assert_eq!(cli.lang, Lang::Spanish);
        assert_eq!(cli.tlds.as_deref(), Some("ai,io"));
        assert_eq!((cli.count, cli.syllables), (20, vec![1, 2]));
        assert_eq!(cli.category.as_deref(), Some("finance"));
        assert!(cli.whole && cli.no_typos && cli.no_rude);
        assert_eq!(
            (cli.prefix.as_deref(), cli.suffix.as_deref()),
            (Some("head"), Some("box"))
        );
        assert_eq!(cli.db, Some(PathBuf::from("/tmp/x.sqlite")));
        assert_eq!(cli.contact.as_deref(), Some("me@example.org"));
        // A language nobody knows is refused, with the ones there are.
        let problem = parse(&["--lang", "xx"]).err().unwrap().to_string();
        assert!(problem.contains("en (English)"), "{problem}");

        // Typos are all of them, none, or one kind.
        use crate::hacks::Kind;
        let typos = |arguments: &[&str]| parse(arguments).unwrap().typos;
        assert_eq!(typos(&["--typos", "off"]), Typos::Off);
        assert_eq!(typos(&["--typos", "missing"]), Typos::Only(Kind::Missing));
        assert_eq!(
            typos(&["check", "numbers", "--typos", "Swapped"]),
            Typos::Only(Kind::Swapped)
        );
        let problem = parse(&["--typos", "maybe"]).err().unwrap().to_string();
        assert!(problem.contains("swapped or missing"), "{problem}");
        // The old way to say none still works, but not next to the new one.
        assert!(parse(&["--no-typos"]).unwrap().no_typos);
        assert!(parse(&["--no-typos", "--typos", "all"]).is_err());
    }

    #[test]
    fn one_thing_can_be_asked_for_without_the_shell() {
        let hunt = parse(&["hunt", "--rounds", "3", "--export", "names.xlsx"]).unwrap();
        match hunt.command {
            Some(Command::Hunt { rounds, export }) => {
                assert_eq!(
                    (rounds, export),
                    (Some(3), Some(PathBuf::from("names.xlsx")))
                );
            }
            _ => panic!("not a hunt"),
        }
        match parse(&["check", "snap", "today"]).unwrap().command {
            Some(Command::Check { words }) => assert_eq!(words, vec!["snap", "today"]),
            _ => panic!("not a check"),
        }
        assert!(matches!(
            parse(&["export"]).unwrap().command,
            Some(Command::Export { path: None })
        ));
        assert!(matches!(
            parse(&["refresh"]).unwrap().command,
            Some(Command::Refresh)
        ));
        assert!(matches!(
            parse(&["stats"]).unwrap().command,
            Some(Command::Stats)
        ));
        // Settings go with any of them, before or after.
        assert_eq!(
            parse(&["stats", "--db", "x.sqlite"]).unwrap().db,
            Some(PathBuf::from("x.sqlite"))
        );
        assert!(parse(&["frobnicate"]).is_err());
    }
}
