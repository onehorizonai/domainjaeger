//! The setup: every setting as a line with its value, the command that
//! changes it and the choices that open on it.

use super::*;

/// The line of the home screen that opens the settings that are seldom
/// touched.
pub(super) const ADVANCED: &str = "Advanced";

/// The lines of the setup. The ones that are seldom touched are a step away,
/// behind the last line of the home screen.
pub(super) struct Setup {
    /// The home screen. Its last line opens `advanced`.
    pub(super) main: Vec<ui::Setting>,
    pub(super) advanced: Vec<ui::Setting>,
}

impl Setup {
    /// The settings of the home screen, without the line that opens the rest.
    pub(super) fn plain(&self) -> &[ui::Setting] {
        &self.main[..self.main.len() - 1]
    }

    /// How many lines the tallest thing takes that the keys can bring up.
    pub(super) fn tallest(&self) -> usize {
        let notes = |lines: &[ui::Setting]| lines.iter().filter(|s| s.note.is_some()).count();
        // Start, a title and the keys around the lines of the home screen,
        // with room for small print that shows up later.
        let home = self.main.len() + notes(&self.main) + 6 + 2;
        let advanced = self.advanced.len() + notes(&self.advanced) + 4 + 1;
        let lists = self.main.iter().chain(&self.advanced);
        let list = lists.map(|s| s.choices.len() + 3).max().unwrap_or(0);
        home.max(advanced).max(list)
    }
}

/// The parts of the category that is on, as they are typed: "finance",
/// "music". None for all words.
pub(super) fn picked(settings: &hunt::Settings) -> Vec<String> {
    let parts = settings.category.as_ref().map(|c| c.name.split('+'));
    parts.into_iter().flatten().map(str::to_string).collect()
}

/// The command that turns `part` on, or off when it is on, and leaves the
/// rest of what is `picked` as it is.
pub(super) fn with_or_without(picked: &[&str], part: &str) -> String {
    let mut parts = picked.to_vec();
    match parts.iter().position(|p| *p == part) {
        Some(at) => {
            parts.remove(at);
        }
        None => parts.push(part),
    }
    match parts.is_empty() {
        true => "cat off".to_string(),
        // A comma keeps a topic of two words together.
        false => format!("cat {}", parts.join(", ")),
    }
}

/// The subjects of the dictionary as the list of all of them has them: the
/// ones that are `picked` first, in the order they were picked, then A to Z.
pub(super) fn in_order(all: &mut [(String, u64)], picked: &[String]) {
    all.sort_by_cached_key(|(topic, _)| {
        let name = topic.to_lowercase();
        let at = picked.iter().position(|part| *part == name);
        (at.unwrap_or(usize::MAX), name)
    });
}

/// A subject of the dictionary in the list of all of them, with the number
/// of its words, and the built-in category it is part of.
pub(super) fn subject(picked: &[&str], topic: &str, size: u64) -> ui::Choice {
    let part = topic.to_lowercase();
    let words = format!("{} words", ui::thousands(size));
    ui::Choice {
        label: topic.to_string(),
        about: match topics::bundle_of(topic) {
            Some(bundle) => format!("{words} · part of {}", bundle.name),
            None => words,
        },
        does: ui::Does::Run(with_or_without(picked, &part)),
        on: picked.contains(&part.as_str()),
    }
}

impl App {
    /// How the hunt is set up, one line per thing you can change: first
    /// which words, then what names to make of them, then the search itself.
    /// In plain words, with an example wherever one says it quicker. Every
    /// line comes with the list of what there is to choose from.
    pub(super) fn settings(&self) -> Result<Setup> {
        let s = &self.hunter.settings;
        let run = |label: &str, about: &str, command: &str| ui::Choice {
            label: label.to_string(),
            about: about.to_string(),
            does: ui::Does::Run(command.to_string()),
            on: false,
        };
        let ask = |label: &str, about: &str, question, command: &str| ui::Choice {
            label: label.to_string(),
            about: about.to_string(),
            does: ui::Does::Ask {
                question,
                command: command.to_string(),
            },
            on: false,
        };
        let setting = |label, value: String, command: &str| ui::Setting {
            label,
            value,
            note: None,
            command: command.to_string(),
            nudge: false,
            choices: Vec::new(),
            current: None,
            several: false,
        };
        let examples = s.lang.examples();

        let others: Vec<&str> = s.lang.others().map(Lang::code).collect();
        let language = ui::Setting {
            choices: Lang::ALL
                .iter()
                .map(|lang| {
                    let about = format!(
                        "words from {} · like {}",
                        lang.dictionary(),
                        lang.examples().hack
                    );
                    run(lang.name(), &about, &format!("lang {}", lang.code()))
                })
                .collect(),
            current: Lang::ALL.iter().position(|lang| *lang == s.lang),
            ..setting(
                "Language",
                format!("{} · words from {}", s.lang.name(), s.lang.dictionary()),
                &format!("lang {}", others.join("|")),
            )
        };

        // A part is a syllable: what that is shows best in a word or three.
        let spoken = [
            "cat",
            "le-mon",
            "ba-na-na",
            "in-for-ma-tion",
            "u-ni-ver-si-ty",
        ];
        let said = |most: usize| match most {
            1..=3 => spoken[..most].join(", "),
            _ => format!("up to {}", spoken[most.min(5) - 1]),
        };
        let counts = self.wiki.syllables();
        let in_a_row = counts.windows(2).all(|pair| pair[1] == pair[0] + 1);
        let longest = counts
            .last()
            .map_or(1, |most| usize::from(*most).clamp(1, 5));
        let length = match counts {
            [1] => format!("1 part · {}", said(1)),
            [1, .., last] if in_a_row => format!("1 to {last} parts · {}", said(longest)),
            several => {
                let counts: Vec<String> = several.iter().map(u8::to_string).collect();
                format!("{} parts · like {}", counts.join(", "), spoken[longest - 1])
            }
        };
        // One part, up to two, up to three and so on.
        let ranges: Vec<Vec<u8>> = (1..=5).map(|most| (1..=most).collect()).collect();
        let length = ui::Setting {
            choices: ranges
                .iter()
                .map(|range| {
                    let label = match range.len() {
                        1 => "1 part".to_string(),
                        most => format!("1 to {most} parts"),
                    };
                    let counts: Vec<String> = range.iter().map(u8::to_string).collect();
                    run(
                        &label,
                        &said(range.len()),
                        &format!("syl {}", counts.join(" ")),
                    )
                })
                .collect(),
            current: ranges.iter().position(|range| range.as_slice() == counts),
            ..setting("Word length", length, "syl 1 2")
        };

        // Several categories can be on at once: every line of the list turns
        // one on or off, and runs what the whole selection is then.
        let picked = picked(s);
        let picked: Vec<&str> = picked.iter().map(String::as_str).collect();
        let mut subjects = vec![ui::Choice {
            on: picked.is_empty(),
            ..run("All words", "the whole dictionary", "cat off")
        }];
        subjects.extend(topics::BUNDLES.iter().map(|bundle| ui::Choice {
            on: picked.contains(&bundle.name),
            ..run(
                bundle.name,
                bundle.about,
                &with_or_without(&picked, bundle.name),
            )
        }));
        // The other subjects are a list of their own: the one `cat list` shows.
        let typed: Vec<&str> = picked
            .iter()
            .copied()
            .filter(|part| !topics::is_built_in(part))
            .collect();
        let about = match typed.is_empty() {
            true => format!("every subject the dictionary has, like {}", examples.topic),
            false => format!("{} · or others", ui::listed(&typed.join("+"))),
        };
        subjects.push(ui::Choice {
            label: "Another subject…".to_string(),
            about,
            does: ui::Does::Browse("cat list".to_string()),
            on: false,
        });
        let category = match &s.category {
            Some(category) => ui::Setting {
                note: Some(
                    category
                        .topics
                        .iter()
                        .map(|topic| match topic.as_str() {
                            topics::NAMES => format!("{} first names", s.lang.name()),
                            _ => topic.clone(),
                        })
                        .collect::<Vec<_>>()
                        .join(" · "),
                ),
                ..setting(
                    "Category",
                    format!("Only {}", ui::listed(&category.name)),
                    "cat off",
                )
            },
            None => setting(
                "Category",
                "All words · or pick a subject".into(),
                "cat finance",
            ),
        };
        let category = ui::Setting {
            current: subjects.iter().position(|choice| choice.on),
            choices: subjects,
            several: true,
            ..category
        };

        let rude = ui::Setting {
            choices: vec![
                run("Show them", "with a ⚠ · swearing, sex, violence", "rude on"),
                run(
                    "Leave them out",
                    "skip words that can mean something rude",
                    "rude off",
                ),
            ],
            current: Some(usize::from(!s.rude)),
            ..match s.rude {
                true => setting(
                    "Rude words",
                    "Shown with a ⚠ · swearing, sex, violence".into(),
                    "rude off",
                ),
                false => setting(
                    "Rude words",
                    "Left out · swearing, sex, violence".into(),
                    "rude on",
                ),
            }
        };

        let name_style = ui::Setting {
            choices: vec![
                run(
                    "Dot inside the word",
                    &format!("the extension finishes the word: {}", examples.hack),
                    "whole off",
                ),
                run(
                    "Whole word before the dot",
                    &format!("{} · in .nl, or the extensions you pick", examples.whole),
                    "whole",
                ),
            ],
            current: Some(usize::from(s.whole)),
            ..match s.whole {
                true => {
                    let value = format!("Whole word before the dot · {}", examples.whole);
                    setting("Name style", value, "whole off")
                }
                false => {
                    let value = format!("Dot inside the word · {}", examples.hack);
                    setting("Name style", value, "whole")
                }
            }
        };

        let misspelled = if s.whole {
            examples.whole_typo
        } else {
            examples.typo
        };
        // A kind of typo by its name, and what it makes of a word in the
        // name style of the hunt.
        let named = |kind: Kind| {
            let mut letters = kind.as_str().chars();
            let first = letters.next().map(|c| c.to_ascii_uppercase());
            first.into_iter().chain(letters).collect::<String>()
        };
        let shown = |kind: Kind| {
            let (word, dotted, reads) = kind.example();
            format!("{word} → {}", if s.whole { reads } else { dotted })
        };
        let does = |kind: Kind| match kind {
            Kind::Doubled => "a letter twice",
            Kind::Undoubled => "one of a double gone",
            Kind::SoundAlike => "a letter that sounds the same",
            Kind::Swapped => "two letters switched",
            Kind::Missing => "one letter left out",
            Kind::Exact => "the word as it is",
        };
        let which = match s.typos {
            Typos::All => "All".to_string(),
            Typos::Off => "Off".to_string(),
            Typos::Only(kind) => named(kind),
        };
        let typos = match s.typos {
            Typos::Off => "Off · only correctly spelled names".to_string(),
            _ if !s.affix.is_empty() => format!("{which} · but not with an added word"),
            Typos::All => format!("All · small misspellings too: {misspelled}"),
            Typos::Only(kind) => format!("{which} · {}", shown(kind)),
        };
        let kinds = Kind::TYPOS.map(|kind| {
            run(
                &named(kind),
                &format!("{}: {}", does(kind), shown(kind)),
                &format!("typos {}", kind.word()),
            )
        });
        let typos = ui::Setting {
            choices: [
                run(
                    "All",
                    &format!("every kind below, like {misspelled}"),
                    "typos all",
                ),
                run("Off", "only correctly spelled names", "typos off"),
            ]
            .into_iter()
            .chain(kinds)
            .collect(),
            current: Some(match s.typos {
                Typos::All => 0,
                Typos::Off => 1,
                Typos::Only(kind) => 2 + Kind::TYPOS.iter().position(|k| *k == kind).unwrap_or(0),
            }),
            ..setting(
                "Typos",
                typos,
                if s.typos.is_off() {
                    "typos all"
                } else {
                    "typos off"
                },
            )
        };

        // A word of your own before every word, and one after: two lines,
        // each with nothing or a word to type.
        let word = examples.word;
        let added =
            |label, part: &str, shown: String, about: &str, question, command| ui::Setting {
                choices: vec![
                    run("Nothing", "just the word itself", &format!("{command} off")),
                    ask(
                        "A word…",
                        &format!("type your own, like {about}"),
                        question,
                        command,
                    ),
                ],
                current: Some(usize::from(!part.is_empty())),
                ..match part.is_empty() {
                    true => setting(label, "Nothing".into(), &format!("{command} {about}")),
                    false => setting(label, shown, &format!("{command} off")),
                }
            };
        let before = added(
            "Add before",
            &s.affix.prefix,
            format!("{} · like {}", s.affix.prefix, s.affix.around(word)),
            "head",
            "Word to put before every word (like head)",
            "before",
        );
        let after = added(
            "Add after",
            &s.affix.suffix,
            format!("{} · like {}", s.affix.suffix, s.affix.around(word)),
            "box",
            "Word to put after every word (like box)",
            "after",
        );

        let active = self.hunter.tlds();
        let presets: [(&str, &str, &str); 3] = match s.whole {
            true => [
                ("Only .nl", "nl", "close to home"),
                ("Only .com", "com", "the classic: most names are taken"),
                (".nl .com .ai", "nl com ai", "three at once"),
            ],
            false => [
                (
                    "The usual mix",
                    "all",
                    "every short extension takes its turn",
                ),
                (".ai .io .app", "ai io app", "the tech favourites"),
                (".nl .be .eu", "nl be eu", "close to home"),
            ],
        };
        let mut where_to: Vec<ui::Choice> = presets
            .iter()
            .map(|(label, endings, about)| run(label, about, &format!("t {endings}")))
            .collect();
        where_to.push(ask(
            "Your own…",
            "type the extensions you want, like ai io app",
            "Extensions, like ai io app",
            "t",
        ));
        let mut sorted = active.clone();
        sorted.sort();
        let picked = presets.iter().position(|(_, endings, _)| {
            let mut preset: Vec<&str> = endings.split(' ').collect();
            preset.sort();
            match self.focused {
                true => preset == sorted,
                false => *endings == "all",
            }
        });
        let mut endings = ui::Setting {
            current: picked.or(Some(where_to.len() - 1)),
            choices: where_to,
            ..setting(
                "Extensions",
                self.dotted(&active),
                if s.whole { "t nl be eu" } else { "t ai io app" },
            )
        };
        if !self.focused {
            let stars: Vec<String> = tld::spotlight(s.lang)
                .iter()
                .filter(|star| active.iter().any(|t| t == *star))
                .map(|star| star.to_string())
                .collect();
            endings.note = Some(format!(
                "and {} other short extensions take their turn",
                active.len() - stars.len()
            ));
            endings.value = self.dotted(&stars);
        }

        let sizes = [
            (5, "a quick look"),
            (10, "the usual"),
            (20, "a good haul"),
            (50, "time for a coffee"),
        ];
        let per_round = ui::Setting {
            choices: sizes
                .iter()
                .map(|(size, about)| run(&format!("{size} names"), about, &format!("count {size}")))
                .collect(),
            current: sizes.iter().position(|(size, _)| *size == s.count),
            ..setting(
                "Per round",
                format!(
                    "Finds {} available name{}, then stops",
                    s.count,
                    if s.count == 1 { "" } else { "s" }
                ),
                // Something else than what it is set to.
                if s.count == 20 {
                    "count 10"
                } else {
                    "count 20"
                },
            )
        };

        let known = self.wiki.contact().is_some();
        let speed = ui::Setting {
            choices: vec![
                ask(
                    "Fast",
                    "say who you are (e-mail or website): 20x faster",
                    "Your e-mail or website, for the dictionary only",
                    "contact",
                ),
                run("Slow", "stay unknown to the dictionary", "contact none"),
            ],
            current: Some(usize::from(!known)),
            ..match known {
                true => setting(
                    "Speed",
                    "Fast · the dictionary knows who you are".into(),
                    "contact",
                ),
                false => ui::Setting {
                    note: Some("20x faster with your e-mail: contact you@example.com".into()),
                    nudge: true,
                    ..setting(
                        "Speed",
                        "Slow · the dictionary does not know you".into(),
                        "contact",
                    )
                },
            }
        };

        let stats = self.cache.stats()?;
        let mut found = ui::Setting {
            // Not something to set: things to do with what was found.
            choices: vec![
                run("Show the list", "the available names found so far", "l"),
                run("Save to Excel", "everything that was checked", "e"),
                run("Numbers", "how many were checked and found", "s"),
                run(
                    "Check old names again",
                    "the ones from over 4 weeks ago",
                    "r",
                ),
            ],
            ..setting(
                "Your finds",
                format!(
                    "{} available names · {} checked",
                    ui::thousands(stats.available),
                    ui::thousands(stats.total)
                ),
                "l",
            )
        };
        if stats.total == 0 {
            found.value = "Nothing yet · start hunting to fill it".into();
        }
        if stats.stale > 0 {
            found.note = Some(format!(
                "{} were checked over 4 weeks ago: r checks them again",
                ui::thousands(stats.stale)
            ));
        }
        // What is seldom touched is a step away, behind one line that says
        // how it stands.
        let more = setting(
            ADVANCED,
            format!(
                "{} · {} per round · {} find{}",
                if known { "Fast" } else { "Slow" },
                s.count,
                ui::thousands(stats.available),
                if stats.available == 1 { "" } else { "s" }
            ),
            "",
        );
        Ok(Setup {
            main: vec![
                language, length, category, rude, name_style, typos, before, after, endings, more,
            ],
            advanced: vec![speed, per_round, found],
        })
    }

    /// The home screen: the setup and what can be done.
    pub(super) fn home(&mut self) -> Result<()> {
        let setup = self.settings()?;
        ui::show(&ui::setup("Setup", setup.plain(), ui::Mark::None, true));
        ui::show(&ui::setup(ADVANCED, &setup.advanced, ui::Mark::None, true));
        ui::actions(&self.next());
        Ok(())
    }

    /// Something in the setup changed: show how things stand now. The
    /// interactive setup does that itself, when it is drawn again.
    pub(super) fn changed(&self, label: &'static str) -> Result<()> {
        self.just_changed.set(Some(label));
        if self.interactive {
            return Ok(());
        }
        let setup = self.settings()?;
        // The table the line is in will do.
        let (title, lines) = match setup.advanced.iter().any(|s| s.label == label) {
            true => (ADVANCED, &setup.advanced[..]),
            false => ("Setup", setup.plain()),
        };
        ui::show(&ui::setup(title, lines, ui::Mark::Changed(label), true));
        self.bar();
        println!();
        Ok(())
    }

    /// What the Start line says.
    pub(super) fn start(&self) -> String {
        match self.rounds {
            0 => "Start hunting".to_string(),
            _ => format!("Find the next {}", self.hunter.settings.count),
        }
    }

    /// What enter does.
    pub(super) fn next(&self) -> String {
        match self.rounds {
            0 => "start hunting".to_string(),
            _ => format!("next {}", self.hunter.settings.count),
        }
    }

    /// The reminder of the keys that matter most.
    pub(super) fn bar(&self) {
        ui::bar(&self.next());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{self, World};

    fn app(world: &World) -> App {
        let catalog = Catalog::of(&["ai", "app", "com", "ing", "nl"]);
        let hunter = world.hunter(&["ing"], testing::settings());
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

    fn labels(lines: &[ui::Setting]) -> Vec<&str> {
        lines.iter().map(|line| line.label).collect()
    }

    #[test]
    fn the_home_screen_has_the_settings_and_one_line_for_the_rest() {
        let world = World::new();
        let setup = app(&world).settings().unwrap();
        assert_eq!(
            labels(&setup.main),
            vec![
                "Language",
                "Word length",
                "Category",
                "Rude words",
                "Name style",
                "Typos",
                "Add before",
                "Add after",
                "Extensions",
                ADVANCED
            ]
        );
        assert_eq!(
            labels(&setup.advanced),
            vec!["Speed", "Per round", "Your finds"]
        );
        assert_eq!(labels(setup.plain()).len(), setup.main.len() - 1);
        // The last line says how the rest stands, and opens it.
        let door = setup.main.last().unwrap();
        assert_eq!(door.value, "Slow · 10 per round · 0 finds");
        assert!(door.choices.is_empty() && door.command.is_empty());
        // Nothing here asked anybody anything.
        assert!(world.web.asked().is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn every_line_says_how_things_stand() {
        let world = World::new();
        world
            .cache
            .put(&Record::sample("cook.ing", world.cache.now()))
            .unwrap();
        let mut app = app(&world);
        for command in [
            "lang nl",
            "syl 1 2",
            "cat finance",
            "rude off",
            "typos on",
            "count 20",
        ] {
            app.execute(command).await;
        }
        app.execute("contact me@example.org").await;
        let setup = app.settings().unwrap();
        let value = |label: &str| {
            let lines = setup.main.iter().chain(&setup.advanced);
            lines
                .clone()
                .find(|line| line.label == label)
                .unwrap()
                .value
                .clone()
        };
        assert!(
            value("Language").starts_with("Dutch"),
            "{}",
            value("Language")
        );
        assert!(
            value("Word length").starts_with("1 to 2 parts"),
            "{}",
            value("Word length")
        );
        assert_eq!(value("Category"), "Only finance");
        assert!(value("Rude words").starts_with("Left out"));
        assert!(value("Typos").starts_with("All"), "{}", value("Typos"));
        assert_eq!(value("Extensions"), ".ing");
        assert!(value("Per round").starts_with("Finds 20 available names"));
        assert!(value("Speed").starts_with("Fast"));
        assert!(
            value("Your finds").starts_with("1 available name"),
            "{}",
            value("Your finds")
        );
        assert_eq!(value(ADVANCED), "Fast · 20 per round · 1 find");
        // The choice that is in force is the one the list opens on.
        let rude = setup
            .main
            .iter()
            .find(|line| line.label == "Rude words")
            .unwrap();
        assert_eq!(rude.choices[rude.current.unwrap()].label, "Leave them out");
    }

    #[tokio::test(start_paused = true)]
    async fn categories_are_turned_on_and_off_one_at_a_time() {
        let world = World::new();
        let listed = r#"{"batchcomplete":true,"query":{"allcategories":[
            {"category":"en:Music","pages":4200},{"category":"en:Sports","pages":3100}]}}"#;
        world.web.on("list=allcategories", 200, listed);
        let mut app = app(&world);
        let list = |app: &App| {
            let setup = app.settings().unwrap();
            setup
                .main
                .into_iter()
                .find(|line| line.label == "Category")
                .unwrap()
        };
        let on = |line: &ui::Setting| -> Vec<String> {
            let on = line.choices.iter().filter(|choice| choice.on);
            on.map(|choice| choice.label.clone()).collect()
        };
        let does = |line: &ui::Setting, label: &str| {
            let choice = line.choices.iter().find(|choice| choice.label == label);
            match &choice.unwrap().does {
                ui::Does::Run(command) | ui::Does::Browse(command) => command.clone(),
                ui::Does::Ask { command, .. } => command.clone(),
            }
        };

        let all = list(&app);
        assert!(all.several);
        assert_eq!(on(&all), ["All words"]);
        assert_eq!(all.choices[all.current.unwrap()].label, "All words");
        assert_eq!(does(&all, "finance"), "cat finance");
        // The other subjects are what `cat list` shows.
        assert_eq!(does(&all, "Another subject…"), "cat list");

        // Every line turns one on or off, the others stay as they are.
        app.execute(&does(&all, "finance")).await;
        app.execute(&does(&list(&app), "tech")).await;
        let two = list(&app);
        assert_eq!(on(&two), ["finance", "tech"]);
        assert_eq!(two.value, "Only finance and tech");
        assert_eq!(does(&two, "finance"), "cat tech");
        assert_eq!(does(&two, "names"), "cat finance, tech, names");
        assert_eq!(does(&two, "All words"), "cat off");
        let topics = &app.hunter.settings.category.as_ref().unwrap().topics;
        assert!(
            topics.contains(&"Banking".to_string()) && topics.contains(&"Computing".to_string())
        );
        // None of that asked the dictionary anything.
        assert!(world.web.asked().is_empty());

        // A subject of the dictionary is turned on and off the same way, in
        // the list of all of them.
        let picked = ["finance", "tech"];
        let music = subject(&picked, "Music", 4200);
        assert!(!music.on && music.about == "4,200 words");
        app.execute(&does_of(&music)).await;
        let three = list(&app);
        assert_eq!(three.value, "Only finance, tech and music");
        let about = three.choices.iter().find(|c| c.label == "Another subject…");
        assert_eq!(about.unwrap().about, "music · or others");
        let picked = ["finance", "tech", "music"];
        assert!(subject(&picked, "Music", 4200).on);
        assert_eq!(
            does_of(&subject(&picked, "Music", 4200)),
            "cat finance, tech"
        );
        let sports = subject(&picked, "Sports", 3100);
        assert_eq!(does_of(&sports), "cat finance, tech, music, sports");
        app.execute(&does_of(&sports)).await;
        assert_eq!(list(&app).value, "Only finance, tech, music and sports");
        // A topic of a built-in category says so.
        assert_eq!(
            subject(&[], "Banking", 900).about,
            "900 words · part of finance"
        );
    }

    #[test]
    fn the_subjects_that_are_on_come_first_then_a_to_z() {
        let mut all: Vec<(String, u64)> = [
            ("People", 11461),
            ("Medicine", 10216),
            ("Sports", 3100),
            ("animal sounds", 120),
            ("Animals", 131),
            ("Music", 4200),
        ]
        .into_iter()
        .map(|(topic, size)| (topic.to_string(), size))
        .collect();
        in_order(&mut all, &["sports".into(), "animals".into()]);
        let topics: Vec<&str> = all.iter().map(|(topic, _)| topic.as_str()).collect();
        assert_eq!(
            topics,
            vec![
                "Sports",
                "Animals",
                "animal sounds",
                "Medicine",
                "Music",
                "People"
            ]
        );
    }

    fn does_of(choice: &ui::Choice) -> String {
        match &choice.does {
            ui::Does::Run(command) => command.clone(),
            _ => String::new(),
        }
    }

    #[test]
    fn every_choice_runs_a_command_the_shell_knows() {
        let world = World::new();
        let setup = app(&world).settings().unwrap();
        let known = [
            "lang", "syl", "cat", "rude", "whole", "typos", "before", "after", "t", "count",
            "contact", "l", "e", "s", "r",
        ];
        let mut choices = 0;
        for line in setup.main.iter().chain(&setup.advanced) {
            for choice in &line.choices {
                let command = match &choice.does {
                    ui::Does::Run(command) | ui::Does::Browse(command) => command.as_str(),
                    ui::Does::Ask { command, .. } => command,
                };
                let word = command.split_whitespace().next().unwrap_or_default();
                assert!(known.contains(&word), "{}: {command}", line.label);
                assert!(!choice.label.is_empty() && !choice.about.is_empty());
                choices += 1;
            }
            // What is typed instead is one of them too.
            let typed = line.command.split_whitespace().next();
            assert!(
                typed.is_none_or(|word| known.contains(&word)),
                "{}",
                line.command
            );
        }
        assert!(choices > 25, "{choices}");
    }

    #[test]
    fn the_tallest_screen_fits_what_is_drawn() {
        let world = World::new();
        let setup = app(&world).settings().unwrap();
        let tallest = setup.tallest();
        let home = ui::start_line("Start hunting", true).len()
            + ui::setup("Setup", &setup.main, ui::Mark::None, true).len()
            + ui::menu_keys("start", None).len();
        assert!(home <= tallest, "{home} > {tallest}");
        for line in setup.main.iter().chain(&setup.advanced) {
            assert!(ui::choices(line, 0).len() <= tallest, "{}", line.label);
        }
        // And all of it fits a terminal of the usual 24 lines.
        assert!(tallest < 24, "{tallest}");
    }
}
