//! The application: the state of a session and the commands that change
//! it. The pieces it is made of (cache, dictionary, checker) are handed
//! to it: see `main`.

mod finds;
mod menu;
mod rounds;
mod setup;

use std::cell::Cell;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, Result, bail};
use console::style;

use crate::cache::{Cache, Record};
use crate::check::{Checker, Status};
use crate::cli::Command;
use crate::hacks::{self, Affix, Kind, Typos};
use crate::hunt::{self, Event, Hunter, Looked, Round};
use crate::lang::Lang;
use crate::tld::{self, Catalog};
use crate::topics::{self, Category};
use crate::wiktionary::{self, Wiktionary};
use crate::{export, ui};

pub(crate) use finds::{meaning_backlog, stats_of, to_excel};
use menu::shell;
use setup::ADVANCED;

pub(crate) const CONTACT_KEY: &str = "contact";
/// Where whole words are hunted when nobody says where.
pub(crate) const WHOLE_TLDS: &str = "nl";
pub(crate) const TOPICS_KEY: &str = "wiki:topics";
pub(crate) const TOPICS_TTL_SECS: i64 = 7 * 24 * 3600;
pub(crate) const WHOLE_NEEDS_TLDS: &str =
    "whole words in every extension is a bit much: name a few, for example `nl be eu`";

pub(crate) struct App {
    cache: Arc<Cache>,
    wiki: Arc<Wiktionary>,
    checker: Arc<Checker>,
    catalog: Catalog,
    hunter: Hunter,
    /// The user picked the TLDs (as opposed to "all of them").
    focused: bool,
    /// The TLDs of the hunt for hacks, kept while whole words are hunted.
    hacks_in: Option<(Vec<String>, bool)>,
    rounds: usize,
    verbose: bool,
    /// The setup is driven with the arrow keys, not only with commands.
    interactive: bool,
    /// Where the arrow keys are on the home screen: 0 is Start, then the
    /// lines of the setup.
    selected: usize,
    /// The line the arrow keys are on behind `Advanced`, when that is open.
    advanced: Option<usize>,
    /// The line of the setup a command just changed.
    just_changed: Cell<Option<&'static str>>,
}

/// Where the shell goes after a command.
enum After {
    /// Back to the prompt.
    Prompt,
    /// The setup changed, or was asked for.
    Setup,
    Quit,
}

impl App {
    pub(crate) fn new(
        cache: Arc<Cache>,
        wiki: Arc<Wiktionary>,
        checker: Arc<Checker>,
        catalog: Catalog,
        hunter: Hunter,
        focused: bool,
        verbose: bool,
    ) -> Self {
        Self {
            cache,
            wiki,
            checker,
            catalog,
            hunter,
            focused,
            hacks_in: None,
            rounds: 0,
            verbose,
            interactive: false,
            selected: 0,
            advanced: None,
            just_changed: Cell::new(None),
        }
    }

    /// Do what was asked on the command line: the shell, or one thing.
    pub(crate) async fn run(&mut self, command: Option<Command>) -> Result<()> {
        let app = self;
        match command {
            None | Some(Command::Hunt { rounds: None, .. }) => shell(app).await,
            Some(Command::Hunt {
                rounds: Some(rounds),
                export: path,
            }) => {
                ui::banner(app.hunter.settings.lang);
                // No commands next to the settings: nobody is going to type here.
                let setup = app.settings()?;
                ui::show(&ui::setup("Setup", setup.plain(), ui::Mark::None, false));
                ui::show(&ui::setup(ADVANCED, &setup.advanced, ui::Mark::None, false));
                for _ in 0..rounds {
                    let round = app.round().await?;
                    if round.interrupted || round.exhausted || round.stuck {
                        break;
                    }
                }
                match path {
                    Some(path) => {
                        meaning_backlog(&app.cache, &app.wiki).await;
                        to_excel(&app.cache, Some(path))
                    }
                    None => Ok(()),
                }
            }
            Some(Command::Check { words }) => {
                if words.is_empty() {
                    bail!("give at least one word, e.g. `domainjaeger check snap`");
                }
                for word in words {
                    app.check_word(&word).await?;
                }
                Ok(())
            }
            Some(Command::Refresh) => app.refresh().await,
            Some(Command::Export { .. } | Command::Stats) => unreachable!("handled before"),
        }
    }
}

impl App {
    /// Run one command of the shell, the way it would be typed.
    async fn execute(&mut self, line: &str) -> After {
        let (command, argument) = match line.split_once(char::is_whitespace) {
            Some((command, argument)) => (command, argument.trim()),
            None => (line, ""),
        };
        let outcome = match command.to_lowercase().as_str() {
            "" | "n" | "next" => {
                self.round_or_report().await;
                self.bar();
                println!();
                Ok(())
            }
            "e" | "export" => {
                meaning_backlog(&self.cache, &self.wiki).await;
                to_excel(
                    &self.cache,
                    (!argument.is_empty()).then(|| PathBuf::from(argument)),
                )
            }
            "l" | "list" => self.list(argument),
            "s" | "stats" => stats_of(&self.cache),
            "c" | "check" if !argument.is_empty() => {
                let mut outcome = Ok(());
                for word in argument.split_whitespace() {
                    outcome = self.check_word(word).await;
                    if outcome.is_err() {
                        break;
                    }
                }
                outcome
            }
            "c" | "check" => {
                let word = self.hunter.settings.lang.examples().word;
                ui::info(&format!("Which word? For example: c {word}"));
                println!();
                Ok(())
            }
            "t" | "tlds" => self.tlds(argument),
            "lang" | "language" => self.language(argument).await,
            "w" | "whole" => self.whole(argument),
            "before" | "prefix" => self.fixed(true, argument),
            "after" | "suffix" => self.fixed(false, argument),
            "cat" | "category" | "categories" => self.category(argument).await,
            "topics" => self.category("list").await,
            "typos" | "typo" => self.typos(argument),
            "rude" => self.rude(argument),
            "syl" | "syllables" | "length" | "parts" => self.syllables(argument),
            "count" | "per" => self.count(argument),
            "contact" => self.set_contact(argument),
            "home" | "m" | "menu" | "setup" if self.interactive => {
                // Back where it begins: on Start.
                self.selected = 0;
                self.advanced = None;
                return After::Setup;
            }
            "home" | "m" | "menu" | "setup" => self.home(),
            "r" | "refresh" => self.refresh().await,
            "v" | "verbose" => {
                self.verbose = !self.verbose;
                ui::info(if self.verbose {
                    "Showing taken names too."
                } else {
                    "Only showing available names."
                });
                println!();
                Ok(())
            }
            "reset" => self.reset(),
            "h" | "help" | "?" => {
                ui::help(self.hunter.settings.lang);
                Ok(())
            }
            "q" | "quit" | "exit" => return After::Quit,
            other => {
                ui::warn(&format!(
                    "\"{other}\" is not a command. h shows them all, home the setup."
                ));
                println!();
                Ok(())
            }
        };
        if let Err(e) = outcome {
            ui::error(&format!("{e:#}"));
            println!();
        }
        // A changed setting brings the setup back, on the line that changed.
        let Some(changed) = self.just_changed.take().filter(|_| self.interactive) else {
            return After::Prompt;
        };
        if let Ok(setup) = self.settings() {
            let line_of = |lines: &[ui::Setting]| lines.iter().position(|s| s.label == changed);
            if let Some(line) = line_of(&setup.advanced) {
                self.advanced = Some(line);
                self.selected = setup.main.len();
            } else if let Some(line) = line_of(&setup.main) {
                self.advanced = None;
                // Start comes first.
                self.selected = line + 1;
            }
        }
        After::Setup
    }

    /// Say who you are to the dictionary (20 times more requests), or stop
    /// doing so.
    fn set_contact(&mut self, argument: &str) -> Result<()> {
        match argument {
            "" => {
                match self.wiki.contact() {
                    Some(contact) => ui::info(&format!(
                        "The dictionary knows you as {}. `contact none` forgets it.",
                        style(contact).bold()
                    )),
                    None => {
                        ui::info("The dictionary answers 20 times as many questions a minute");
                        ui::info("when it knows who is asking. Your e-mail address or website is");
                        ui::info(
                            "only sent to the dictionary (Wikimedia): `contact you@example.com`",
                        );
                    }
                }
                println!();
                return Ok(());
            }
            "none" | "off" => {
                self.cache.kv_put(CONTACT_KEY, "")?;
                self.wiki.set_contact(None);
            }
            contact if wiktionary::valid_contact(contact) => {
                self.cache.kv_put(CONTACT_KEY, contact)?;
                self.wiki.set_contact(Some(contact.to_string()));
            }
            _ => bail!("that does not look like an e-mail address or a URL"),
        }
        self.changed("Speed")
    }

    fn tlds(&mut self, argument: &str) -> Result<()> {
        if argument.is_empty() {
            let endings = self.dotted(&self.hunter.tlds());
            ui::info(&format!("Searching in these extensions: {endings}"));
            ui::info(
                &style("Change with for example `t ai io app`, or `t all` for every short one.")
                    .dim()
                    .to_string(),
            );
            println!();
            return Ok(());
        }
        let (tlds, focused) = match argument {
            "all" if self.hunter.settings.whole => bail!("{WHOLE_NEEDS_TLDS}"),
            "all" => (self.catalog.defaults(), false),
            wanted => (self.catalog.select(wanted)?, true),
        };
        self.rebuild(tlds, focused);
        self.changed("Extensions")
    }

    /// Small typos: all of them, none, or one kind. Without a word it is
    /// all or none, whichever it is not now.
    fn typos(&mut self, argument: &str) -> Result<()> {
        let typos = match argument {
            "" if self.hunter.settings.typos.is_off() => Typos::All,
            "" => Typos::Off,
            wanted => match Typos::parse(wanted) {
                Some(typos) => typos,
                None => bail!(
                    "`typos all`, `typos off`, or one kind: doubled, dropped, sound, swapped, missing"
                ),
            },
        };
        self.hunter.settings.typos = typos;
        let (tlds, focused) = (self.hunter.tlds(), self.focused);
        self.rebuild(tlds, focused);
        self.changed("Typos")
    }

    /// Words the dictionary flags as vulgar, offensive, sexual or violent:
    /// marked, or left out of the hunt.
    fn rude(&mut self, argument: &str) -> Result<()> {
        let on = match argument {
            "" => !self.hunter.settings.rude,
            "on" | "yes" => true,
            "off" | "no" => false,
            _ => bail!("`rude on` or `rude off`"),
        };
        self.hunter.settings.rude = on;
        let (tlds, focused) = (self.hunter.tlds(), self.focused);
        self.rebuild(tlds, focused);
        self.changed("Rude words")
    }

    /// How many available names a round looks for.
    fn count(&mut self, argument: &str) -> Result<()> {
        match argument.parse::<usize>() {
            Ok(count) if (1..=100).contains(&count) => self.hunter.settings.count = count,
            _ => bail!("a number from 1 to 100, for example `count 20`"),
        }
        self.changed("Per round")
    }

    /// How many syllables a word may have.
    fn syllables(&mut self, argument: &str) -> Result<()> {
        let mut syllables = Vec::new();
        for number in argument.split([',', ' ']).filter(|s| !s.is_empty()) {
            match number.parse::<u8>() {
                Ok(n) if (1..=5).contains(&n) => syllables.push(n),
                _ => bail!("numbers from 1 to 5, for example `syl 1 2` (cat, le-mon)"),
            }
        }
        if syllables.is_empty() {
            bail!(
                "in how many parts may a word be said? `syl 1 2` is cat and le-mon, \
                 `syl 1 2 3` ba-na-na as well"
            );
        }
        syllables.sort_unstable();
        syllables.dedup();
        // The dictionary is asked about these syllable counts only.
        self.wiki = Arc::new(self.wiki.with_syllables(&syllables));
        let (tlds, focused) = (self.hunter.tlds(), self.focused);
        self.rebuild(tlds, focused);
        self.changed("Word length")
    }

    /// Switch between hunting for hacks (leni.ng) and for whole words and
    /// their typos (lening.nl, lenning.nl).
    fn whole(&mut self, argument: &str) -> Result<()> {
        if self.set_whole(argument)? {
            self.changed("Name style")?;
        }
        Ok(())
    }

    /// Returns whether anything changed.
    fn set_whole(&mut self, argument: &str) -> Result<bool> {
        let on = self.hunter.settings.whole;
        let (tlds, focused) = match argument {
            "off" | "no" | "hacks" if !on => {
                ui::info("The dot is inside the word already (cook.ing). `whole` puts the whole");
                ui::info("word before the dot instead, in .nl.");
                println!();
                return Ok(false);
            }
            // Back to the hacks, in the TLDs they were hunted in before.
            "off" | "no" | "hacks" => self
                .hacks_in
                .take()
                .unwrap_or_else(|| (self.catalog.defaults(), false)),
            "all" => bail!("{WHOLE_NEEDS_TLDS}"),
            "" if on => {
                ui::info("Already hunting for whole words.");
                ui::info(
                    &style(
                        "`whole nl be eu` changes the extensions, `whole off` puts the dot inside.",
                    )
                    .dim()
                    .to_string(),
                );
                println!();
                return Ok(false);
            }
            wanted => {
                let wanted = if wanted.is_empty() {
                    WHOLE_TLDS
                } else {
                    wanted
                };
                let tlds = self.catalog.select(wanted)?;
                if !on {
                    self.hacks_in = Some((self.hunter.tlds(), self.focused));
                }
                (tlds, true)
            }
        };
        self.hunter.settings.whole = !matches!(argument, "off" | "no" | "hacks");
        // A word with a fixed part is a whole name: no hacks of those.
        if !self.hunter.settings.whole {
            self.hunter.settings.affix = Affix::default();
        }
        self.rebuild(tlds, focused);
        Ok(true)
    }

    /// `before head` adds a word before every word, `after box` after it.
    fn fixed(&mut self, before: bool, argument: &str) -> Result<()> {
        let part = match argument {
            "" => {
                let word = self.hunter.settings.lang.examples().word;
                ui::info(&format!(
                    "`after box` adds box after every word ({word}box), `before head` adds head"
                ));
                ui::info(&format!(
                    "before it (head{word}). `after off` and `before off` take them away again."
                ));
                println!();
                return Ok(());
            }
            "off" | "no" | "none" => String::new(),
            part => fixed_part(part)?,
        };
        match before {
            true => self.set_affix(Some(part), None),
            false => self.set_affix(None, Some(part)),
        }
    }

    fn set_affix(&mut self, prefix: Option<String>, suffix: Option<String>) -> Result<()> {
        let before_changed = prefix.is_some() && suffix.is_none();
        let affix = &mut self.hunter.settings.affix;
        if let Some(prefix) = prefix {
            affix.prefix = prefix;
        }
        if let Some(suffix) = suffix {
            affix.suffix = suffix;
        }
        // A word with a fixed part is a whole name.
        if self.hunter.settings.affix.is_empty() || self.hunter.settings.whole {
            let (tlds, focused) = (self.hunter.tlds(), self.focused);
            self.rebuild(tlds, focused);
        } else {
            self.set_whole("")?;
        }
        self.changed(if before_changed {
            "Add before"
        } else {
            "Add after"
        })
    }

    /// Show the language, or switch to the words of another one.
    async fn language(&mut self, argument: &str) -> Result<()> {
        if argument.is_empty() {
            ui::languages(self.hunter.settings.lang);
            return Ok(());
        }
        let Some(lang) = Lang::parse(argument) else {
            bail!(
                "\"{argument}\" is not a language I know: {}",
                Lang::choices()
            );
        };
        // The built-in categories exist in every language; a single topic
        // may be called something else for another one, or not be there.
        let category = self.hunter.settings.category.take();
        let parts: Vec<&str> = category
            .as_ref()
            .map(|category| category.name.split('+').collect())
            .unwrap_or_default();
        let (ours, theirs): (Vec<&str>, Vec<&str>) = parts
            .into_iter()
            .partition(|part| topics::is_built_in(part));
        if !theirs.is_empty() {
            let quoted: Vec<String> = theirs.iter().map(|part| format!("\"{part}\"")).collect();
            ui::info(&format!(
                "{} words have topics of their own, so {} {} off. `cat list` shows them.",
                lang.name(),
                ui::listed(&quoted.join("+")),
                if theirs.len() == 1 { "is" } else { "are" }
            ));
        }
        self.hunter.settings.category = match ours.is_empty() {
            true => None,
            false => Some(topics::resolve(lang, &ours.join("+"), &[])?),
        };
        self.hunter.settings.lang = lang;
        // The TLDs that are always in the mix differ per language.
        let (tlds, focused) = if self.focused {
            (self.hunter.tlds(), true)
        } else {
            (self.catalog.defaults(), false)
        };
        self.rebuild(tlds, focused);
        self.changed("Language")
    }

    /// Show the categories, or limit the hunt to one.
    async fn category(&mut self, argument: &str) -> Result<()> {
        let lang = self.hunter.settings.lang;
        // The words that are not a category, however they are typed.
        let category = match argument.to_lowercase().as_str() {
            "" => {
                ui::categories(self.hunter.settings.category.as_ref(), lang);
                return Ok(());
            }
            "list" | "all?" | "?" => {
                let spinner = self.asking_for_topics(true);
                let all = topic_list(&self.cache, &self.wiki, lang).await;
                if let Some(spinner) = spinner {
                    spinner.stop();
                }
                ui::topic_list(&all?, lang);
                return Ok(());
            }
            "off" | "all" | "none" => None,
            _ => {
                let spinner = self.asking_for_topics(!topics::is_built_in(argument));
                let category = category_of(&self.cache, &self.wiki, lang, argument).await;
                if let Some(spinner) = spinner {
                    spinner.stop();
                }
                Some(category?)
            }
        };
        // The same again leaves the hunt as it is.
        if category == self.hunter.settings.category {
            return self.changed("Category");
        }
        self.hunter.settings.category = category;
        let (tlds, focused) = (self.hunter.tlds(), self.focused);
        self.rebuild(tlds, focused);
        self.changed("Category")
    }

    /// A spinner while the dictionary is asked for its topics, when they
    /// are `needed` and not known yet. Without one nothing is printed, so
    /// the lists of the home screen stay where they are.
    pub(super) fn asking_for_topics(&self, needed: bool) -> Option<ui::Spinner> {
        let lang = self.hunter.settings.lang;
        if !needed || known_topics(&self.cache, lang).is_some() {
            return None;
        }
        let spinner = ui::Spinner::start();
        spinner.status(&format!("asking {} for its topics", lang.dictionary()));
        Some(spinner)
    }

    fn reset(&mut self) -> Result<()> {
        self.cache.reset_streams()?;
        let (tlds, focused) = (self.hunter.tlds(), self.focused);
        self.rebuild(tlds, focused);
        ui::info(
            "Back to the first page of the dictionary. Names checked in the last 4 weeks are still skipped.",
        );
        println!();
        Ok(())
    }

    fn rebuild(&mut self, tlds: Vec<String>, focused: bool) {
        self.focused = focused;
        self.hunter = Hunter::new(
            self.wiki.clone(),
            self.checker.clone(),
            self.cache.clone(),
            tlds,
            focused,
            self.hunter.settings.clone(),
        );
    }

    fn dotted(&self, tlds: &[String]) -> String {
        tlds.iter()
            .map(|t| format!(".{t}"))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// A fixed part of a name ("box"): a few plain letters, like the rest of
/// what goes before the dot. Empty for none.
pub(crate) fn fixed_part(text: &str) -> Result<String> {
    let part = text.trim().to_lowercase();
    if !part.is_empty() && (!hacks::is_plain(&part) || part.len() > 20) {
        bail!("\"{part}\" has to be plain a-z, 20 letters at most");
    }
    Ok(part)
}

/// Every topic the dictionary of a language has, remembered for a week.
pub(crate) async fn topic_list(
    cache: &Cache,
    wiki: &Wiktionary,
    lang: Lang,
) -> Result<Vec<(String, u64)>> {
    if let Some(all) = known_topics(cache, lang) {
        return Ok(all);
    }
    let key = format!("{TOPICS_KEY}:{}", lang.code());
    let all = wiki
        .topics(lang)
        .await
        .with_context(|| format!("could not get the topics from {}", lang.dictionary()))?;
    cache.kv_put(&key, &serde_json::to_string(&all)?)?;
    Ok(all)
}

/// The topics of a language when the cache has them, biggest first.
fn known_topics(cache: &Cache, lang: Lang) -> Option<Vec<(String, u64)>> {
    let key = format!("{TOPICS_KEY}:{}", lang.code());
    let json = cache.kv_get(&key, TOPICS_TTL_SECS).ok()??;
    serde_json::from_str(&json).ok()
}

/// "finance", "tech+muziek", ...: the built-in categories need no lookup,
/// anything else has to be a topic the dictionary knows.
pub(crate) async fn category_of(
    cache: &Cache,
    wiki: &Wiktionary,
    lang: Lang,
    wanted: &str,
) -> Result<Category> {
    if topics::is_built_in(wanted) {
        return topics::resolve(lang, wanted, &[]);
    }
    let known: Vec<String> = topic_list(cache, wiki, lang)
        .await?
        .into_iter()
        .map(|(topic, _)| topic)
        .collect();
    topics::resolve(lang, wanted, &known)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{self, TWO, World, answer, page};

    /// An application in a scripted world, hunting in these extensions.
    fn app(world: &World, tlds: &[&str]) -> App {
        let catalog = Catalog::of(&["ai", "app", "com", "ing", "io", "museum", "ng", "nl"]);
        let hunter = world.hunter(tlds, testing::settings());
        let (wiki, checker) = (world.wiki(&[1, 2, 3]), world.checker());
        App::new(
            world.cache.clone(),
            wiki,
            checker,
            catalog,
            hunter,
            true,
            false,
        )
    }

    #[tokio::test(start_paused = true)]
    async fn settings_change_with_a_plain_command() {
        let world = World::new();
        let mut app = app(&world, &["ing"]);
        let settings = |app: &App| app.hunter.settings.clone();

        app.execute("typos on").await;
        assert_eq!(settings(&app).typos, Typos::All);
        app.execute("typos").await;
        assert_eq!(settings(&app).typos, Typos::Off);
        // One kind of typo, however it is typed. From there the bare word
        // turns them off, and from off it turns all of them on.
        app.execute("typos Missing ").await;
        assert_eq!(settings(&app).typos, Typos::Only(Kind::Missing));
        app.execute("typos swapped").await;
        assert_eq!(settings(&app).typos, Typos::Only(Kind::Swapped));
        app.execute("typos").await;
        assert_eq!(settings(&app).typos, Typos::Off);
        app.execute("typos").await;
        assert_eq!(settings(&app).typos, Typos::All);
        app.execute("typos off").await;
        app.execute("rude off").await;
        assert!(!settings(&app).rude);
        // A category by its name, however it is typed, and off the same way.
        app.execute("cat  Finance ").await;
        let category = settings(&app).category.map(|category| category.name);
        assert_eq!(category.as_deref(), Some("finance"));
        app.execute("CAT Off").await;
        assert!(settings(&app).category.is_none());
        app.execute("count 20").await;
        assert_eq!(settings(&app).count, 20);
        app.execute("lang nl").await;
        assert_eq!(settings(&app).lang, Lang::Dutch);
        app.execute("LANG Spanish").await;
        assert_eq!(settings(&app).lang, Lang::Spanish);

        // What makes no sense is said so, and changes nothing.
        for nonsense in [
            "count 0",
            "count many",
            "typos maybe",
            "rude perhaps",
            "lang xx",
            "syl 9",
            "syl",
        ] {
            app.execute(nonsense).await;
        }
        let after = settings(&app);
        assert_eq!(
            (after.count, after.typos, after.rude, after.lang),
            (20, Typos::Off, false, Lang::Spanish)
        );
        // Nothing here needed the dictionary or a registry.
        assert!(world.web.asked().is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn word_length_is_what_the_dictionary_gets_asked_about() {
        let world = World::new();
        world.registry("ing");
        world.otherwise_quiet();
        let mut app = app(&world, &["ing"]);
        app.execute("syl 2, 1 2").await;
        app.execute("").await;
        let search = world
            .web
            .asked()
            .into_iter()
            .find(|a| a.contains("gsrsearch"))
            .unwrap();
        assert!(
            search.contains("\"English 1-syllable words|English 2-syllable words\""),
            "{search}"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn extensions_are_picked_by_name() {
        let world = World::new();
        let mut app = app(&world, &["ing"]);
        app.execute("t .AI, io").await;
        assert_eq!(app.hunter.tlds(), vec!["ai", "io"]);
        assert!(app.focused);
        // One that does not exist leaves things as they were.
        app.execute("t ai zzz").await;
        assert_eq!(app.hunter.tlds(), vec!["ai", "io"]);
        // Every short one: the long .museum is for those who ask.
        app.execute("t all").await;
        assert!(!app.focused);
        let all = app.hunter.tlds();
        assert!(all.contains(&"nl".to_string()) && !all.contains(&"museum".to_string()));
    }

    #[tokio::test(start_paused = true)]
    async fn the_name_style_switches_and_remembers_where_it_was() {
        let world = World::new();
        let mut app = app(&world, &["ing", "ai"]);
        app.execute("whole nl com").await;
        assert!(app.hunter.settings.whole);
        assert_eq!(app.hunter.tlds(), vec!["nl", "com"]);
        // Every extension would be too much for whole words.
        app.execute("t all").await;
        assert_eq!(app.hunter.tlds(), vec!["nl", "com"]);
        // Back to the dot inside the word, in the extensions of before.
        app.execute("whole off").await;
        assert!(!app.hunter.settings.whole);
        assert_eq!(app.hunter.tlds(), vec!["ing", "ai"]);
        // Without extensions it is .nl.
        app.execute("w").await;
        assert_eq!(app.hunter.tlds(), vec!["nl"]);
    }

    #[tokio::test(start_paused = true)]
    async fn a_word_of_your_own_makes_whole_names() {
        let world = World::new();
        let mut app = app(&world, &["ing"]);
        app.execute("after Box").await;
        let settings = app.hunter.settings.clone();
        assert_eq!(
            (
                settings.affix.prefix.as_str(),
                settings.affix.suffix.as_str()
            ),
            ("", "box")
        );
        assert!(settings.whole);
        app.execute("before head").await;
        assert_eq!(app.hunter.settings.affix.prefix, "head");
        // Only plain letters make a name.
        app.execute("after b-o-x").await;
        assert_eq!(app.hunter.settings.affix.suffix, "box");
        app.execute("after off").await;
        app.execute("before off").await;
        assert!(app.hunter.settings.affix.is_empty());
        // Going back to the dot inside the word takes the added words away.
        app.execute("after box").await;
        app.execute("whole off").await;
        assert!(app.hunter.settings.affix.is_empty());

        assert_eq!(fixed_part("  Box ").unwrap(), "box");
        assert_eq!(fixed_part("").unwrap(), "");
        assert!(fixed_part("b0x").is_err());
        assert!(fixed_part(&"a".repeat(21)).is_err());
    }

    #[tokio::test(start_paused = true)]
    async fn an_extension_that_stops_answering_stays_in_the_setup() {
        // The registry of .nl times out until it is left out of the round.
        // A new word before every word used to leave no extension at all,
        // and the hunt said the dictionary had no more words.
        let world = World::new();
        world.registry("nl");
        world.finds("intitle:/^[a-z]{3,}$/", &["lemon", "melon", "mango"]);
        world.web.down("rdap.test/nl/");
        world.otherwise_quiet();
        let mut app = app(&world, &["ing"]);
        app.execute("before head").await;
        app.execute("").await;
        let unclear = world.cache.get("headlemon.nl").unwrap().unwrap();
        assert_eq!(unclear.status, Status::Unknown);
        assert_eq!(app.hunter.tlds(), vec!["nl"]);

        // Once it answers again, another word before them finds names.
        world.web.on("rdap.test/nl/", 404, "");
        app.execute("before foot").await;
        assert_eq!(app.hunter.tlds(), vec!["nl"]);
        app.execute("").await;
        let found = world.cache.get("footlemon.nl").unwrap().unwrap();
        assert_eq!(found.status, Status::Available);
    }

    #[tokio::test(start_paused = true)]
    async fn an_extension_that_stops_answering_has_not_run_dry() {
        // Plenty of words end in .ing, its registry just does not answer.
        // That used to switch to whole words, and crash on the way.
        let world = World::new();
        world.registry("ing").pages("+ing$/", 20, "ing");
        world.web.down("rdap.test/ing/");
        world.otherwise_quiet();
        let mut app = app(&world, &["ing"]);
        let round = app.pass().await.unwrap().0;
        assert!(round.stuck && round.retry && !round.exhausted);
        assert!(!app.ran_dry(&round));
        app.execute("").await;
        assert!(!app.hunter.settings.whole);
        assert_eq!(app.hunter.tlds(), vec!["ing"]);
    }

    #[tokio::test(start_paused = true)]
    async fn a_hunt_that_runs_dry_goes_on_with_whole_words() {
        // Only .nl, with the dot inside the word: no word ends in "nl".
        // That used to end the hunt before it began.
        let world = World::new();
        world.registry("nl").taken(&["lemon.nl"]);
        world.finds("intitle:/^[a-z]{3,}$/", &["lemon", "melon"]);
        world.otherwise_quiet();
        let mut app = app(&world, &["nl"]);
        assert!(!app.hunter.settings.whole);

        app.execute("").await;
        assert!(app.hunter.settings.whole);
        assert_eq!(app.hunter.tlds(), vec!["nl"]);
        let found = world.cache.get("melon.nl").unwrap().unwrap();
        assert_eq!((found.status, found.whole), (Status::Available, true));
        assert_eq!(
            world.cache.get("lemon.nl").unwrap().unwrap().status,
            Status::Taken
        );
        // The setup says so too.
        let setup = app.settings().unwrap();
        let style = setup
            .main
            .iter()
            .find(|line| line.label == "Name style")
            .unwrap();
        assert!(style.value.starts_with("Whole word"), "{}", style.value);
    }

    #[tokio::test(start_paused = true)]
    async fn a_hunt_that_still_has_words_keeps_its_name_style() {
        let world = World::new();
        world.registry("ing");
        world.finds("+ing$/", &["cooking"]);
        world.otherwise_quiet();
        // Every extension in the mix: running dry there is the end of it.
        let mut everything = app(&world, &["ing"]);
        everything.focused = false;
        everything.execute("").await;
        assert!(!everything.hunter.settings.whole);
        assert_eq!(
            world.cache.get("cook.ing").unwrap().unwrap().status,
            Status::Available
        );

        let round = Round {
            exhausted: true,
            ..Round::default()
        };
        let picked = app(&world, &["ing"]);
        assert!(picked.ran_dry(&round));
        let stopped = Round {
            interrupted: true,
            exhausted: true,
            ..Round::default()
        };
        assert!(!picked.ran_dry(&stopped));
        assert!(!picked.ran_dry(&Round::default()));
    }

    #[tokio::test(start_paused = true)]
    async fn the_dictionary_can_be_told_who_is_asking() {
        let world = World::new();
        let mut app = app(&world, &["ing"]);
        app.execute("contact not an address").await;
        assert_eq!(app.wiki.contact(), None);
        app.execute("contact me@example.org").await;
        assert_eq!(app.wiki.contact().as_deref(), Some("me@example.org"));
        // Remembered for the next session.
        let stored = world.cache.kv_get(CONTACT_KEY, i64::MAX).unwrap();
        assert_eq!(stored.as_deref(), Some("me@example.org"));
        app.execute("contact none").await;
        assert_eq!(app.wiki.contact(), None);
        assert_eq!(
            world
                .cache
                .kv_get(CONTACT_KEY, i64::MAX)
                .unwrap()
                .as_deref(),
            Some("")
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_category_is_one_of_ours_or_a_subject_of_the_dictionary() {
        let world = World::new();
        let listed = r#"{"batchcomplete":true,"query":{"allcategories":[{"category":"en:Music","pages":4200}]}}"#;
        world.web.on("list=allcategories", 200, listed);
        let mut app = app(&world, &["ing"]);
        let category = |app: &App| {
            app.hunter
                .settings
                .category
                .clone()
                .map(|c| (c.name, c.topics))
        };

        // The built-in ones need no dictionary.
        app.execute("cat finance").await;
        let (name, topics) = category(&app).unwrap();
        assert_eq!(name, "finance");
        assert!(topics.contains(&"Banking".to_string()));
        assert!(world.web.asked().is_empty());

        app.execute("cat music").await;
        assert_eq!(
            category(&app),
            Some(("music".to_string(), vec!["Music".to_string()]))
        );
        // The list of subjects is kept: asked once.
        app.execute("cat knitting").await;
        assert_eq!(world.web.count("allcategories"), 1);
        assert_eq!(category(&app).unwrap().0, "music");
        // Another language has subjects of its own; ours exist in all.
        app.execute("lang nl").await;
        assert_eq!(category(&app), None);
        app.execute("lang en").await;
        app.execute("cat finance, music").await;
        app.execute("lang nl").await;
        let (name, topics) = category(&app).unwrap();
        assert_eq!(name, "finance");
        assert!(topics.contains(&"Financieel".to_string()));
        app.execute("cat names").await;
        app.execute("lang la").await;
        assert_eq!(category(&app).unwrap().0, "names");
        app.execute("cat off").await;
        assert_eq!(category(&app), None);
    }

    #[tokio::test(start_paused = true)]
    async fn the_shell_knows_when_to_stop_and_what_it_does_not_know() {
        let world = World::new();
        let mut app = app(&world, &["ing"]);
        assert!(matches!(app.execute("q").await, After::Quit));
        assert!(matches!(app.execute("exit").await, After::Quit));
        assert!(matches!(app.execute("frobnicate now").await, After::Prompt));
        assert!(matches!(app.execute("h").await, After::Prompt));
        assert!(matches!(app.execute("home").await, After::Prompt));
        // In the interactive setup a change leads back to its line.
        app.interactive = true;
        assert!(matches!(app.execute("count 5").await, After::Setup));
        assert_eq!(app.advanced, Some(1));
        assert!(matches!(app.execute("typos on").await, After::Setup));
        assert_eq!((app.advanced, app.selected), (None, 6));
        assert!(matches!(app.execute("home").await, After::Setup));
        assert_eq!(app.selected, 0);
        // A command that changed nothing leads to the prompt.
        assert!(matches!(app.execute("count zero").await, After::Prompt));
    }

    #[tokio::test(start_paused = true)]
    async fn a_word_of_your_own_and_old_names_are_checked_on_request() {
        let world = World::new();
        world.registry("ing").taken(&["bak.ing"]);
        world.web.on(
            "titles=cooking",
            200,
            &answer(vec![page("cooking", 0, &[TWO])]),
        );
        world.otherwise_quiet();
        let mut app = app(&world, &["ing"]);
        app.execute("c cooking").await;
        assert_eq!(
            world.cache.get("cook.ing").unwrap().unwrap().status,
            Status::Available
        );

        // Four weeks on, a name that was free is looked at again.
        world
            .cache
            .put(&Record::sample("bak.ing", world.cache.now()))
            .unwrap();
        app.execute("r").await;
        assert_eq!(
            world.cache.get("bak.ing").unwrap().unwrap().status,
            Status::Available
        );
        world.clock.advance(crate::cache::DOMAIN_TTL_SECS);
        app.execute("r").await;
        assert_eq!(
            world.cache.get("bak.ing").unwrap().unwrap().status,
            Status::Taken
        );

        // Starting over forgets the place in the dictionary, not the names.
        world
            .cache
            .set_stream_state("ing", "ing", None, true)
            .unwrap();
        app.execute("reset").await;
        assert_eq!(
            world.cache.stream_state("ing", "ing").unwrap(),
            (None, false)
        );
        assert!(world.cache.get("cook.ing").unwrap().is_some());
    }
}
