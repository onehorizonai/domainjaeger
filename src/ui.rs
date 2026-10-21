//! Everything the terminal shows: the home screen, rows, spinner and a bit
//! of cheer. One accent colour, dim for whatever is secondary, and every
//! setting next to the command that changes it.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use chrono::{Local, TimeZone};
use console::{Alignment, Style, Term, pad_str, style};
use indicatif::{ProgressBar, ProgressStyle};
use rand::seq::IndexedRandom;

use crate::cache::{Record, Stats};
use crate::check::Status;
use crate::hacks::Affix;
use crate::hunt::Round;
use crate::lang::Lang;
use crate::topics::{self, BUNDLES, Category};

const ORANGE: u8 = 208;
/// The title: orange, amber, gold and yellow, three letters each. No red.
const GRADIENT: &[u8] = &[208, 208, 208, 214, 214, 214, 220, 220, 220, 226, 226, 226];
/// A word of cheer after a good round, in the language of the hunt.
fn cheers(lang: Lang) -> &'static [&'static str] {
    match lang {
        Lang::English => &[
            "Nice one!",
            "Bingo!",
            "Jackpot!",
            "There it is.",
            "On a roll.",
            "That's a keeper.",
            "Like clockwork.",
            "Fresh out of the oven.",
        ],
        Lang::Dutch => &[
            "Lekker bezig!",
            "Kassa!",
            "Hoppa!",
            "Gefeliciteerd!",
            "Dat is 'm!",
            "Kat in het bakkie.",
            "Als een speer.",
            "Zo, die zit.",
        ],
        Lang::Latin => &[
            "Veni, vidi, vici.",
            "Carpe diem!",
            "Optime!",
            "Euge!",
            "Habemus nomen!",
            "Alea iacta est.",
            "Fortes fortuna adiuvat.",
        ],
        Lang::Spanish => &[
            "¡Olé!",
            "¡Toma ya!",
            "¡Qué bien!",
            "¡Eso es!",
            "¡Vamos!",
            "Pan comido.",
            "¡Bingo!",
        ],
    }
}

/// And a sigh after a poor one.
fn sighs(lang: Lang) -> &'static [&'static str] {
    match lang {
        Lang::English => &[
            "No luck this time.",
            "Late to the party, all gone.",
            "Missed the boat.",
            "Tough crowd.",
        ],
        Lang::Dutch => &[
            "Helaas pindakaas.",
            "Mosterd na de maaltijd, alles al weg.",
            "Achter het net gevist.",
            "Jammer joh.",
        ],
        Lang::Latin => &[
            "Nihil.",
            "O tempora, o mores!",
            "Errare humanum est.",
            "Sic transit gloria mundi.",
        ],
        Lang::Spanish => &[
            "Nada de nada.",
            "Mala suerte.",
            "Otra vez será.",
            "A buenas horas, mangas verdes.",
        ],
    }
}

pub fn orange() -> Style {
    Style::new().color256(ORANGE)
}

pub fn banner(lang: Lang) {
    let [first, second, third] = lang.examples().names;
    let title: String = "DOMAINJAEGER"
        .chars()
        .zip(GRADIENT)
        .map(|(c, color)| style(c).color256(*color).bold().to_string())
        .collect();
    let dot = style("·").dim();
    println!();
    println!(
        "  {}{} {title}  {}",
        style("▟").color256(208).bold(),
        style("▙").color256(214).bold(),
        style(concat!("v", env!("CARGO_PKG_VERSION"))).dim()
    );
    println!(
        "  {} {} {dot} {} {dot} {}",
        style("Domain names made of real words:").dim(),
        domain(first.0, first.1),
        domain(second.0, second.1),
        domain(third.0, third.1),
    );
    println!();
}

/// One line of the setup: what something is set to, and the command that
/// changes it.
pub struct Setting {
    pub label: &'static str,
    pub value: String,
    /// A second line of small print under the value.
    pub note: Option<String>,
    /// What to type to change it.
    pub command: String,
    /// Worth a second look (shown in yellow).
    pub nudge: bool,
    /// What there is to choose from: the list that opens on this line of
    /// the interactive setup.
    pub choices: Vec<Choice>,
    /// The choice that is in force, if it is one of them.
    pub current: Option<usize>,
}

/// One thing to pick in the list of a setting.
pub struct Choice {
    pub label: String,
    /// What it means, in a few plain words.
    pub about: String,
    pub does: Does,
}

/// What picking a choice does.
pub enum Does {
    /// Run a command of the shell.
    Run(String),
    /// Ask for a value and run `command <value>`.
    Ask {
        question: &'static str,
        command: &'static str,
    },
}

/// Which line of the setup stands out.
#[derive(Clone, Copy)]
pub enum Mark<'a> {
    None,
    /// The line with this label was just changed: a check mark.
    Changed(&'a str),
    /// The line the arrow keys are on.
    Selected(usize),
}

/// Where the text of every line of the setup starts. The arrow that shows
/// where the keys are stands in front of it, in the margin.
const MARGIN: usize = 2;
const LABEL_WIDTH: usize = 12;
/// Where a value starts: after the label.
const NOTE_INDENT: usize = MARGIN + LABEL_WIDTH + 2;
/// The least room a value gets.
const VALUE_MIN: usize = 34;
/// Room for the command next to a value: `lang nl|la|es` is the longest.
const COMMAND_WIDTH: usize = 14;

/// A table of settings under a title, as lines to print or to draw.
/// `commands` shows what to type to change each line.
pub fn setup(title: &str, settings: &[Setting], mark: Mark, commands: bool) -> Vec<String> {
    let columns = Term::stdout().size().1 as usize;
    // Label, value and command have to fit on one line, with a cell to
    // spare: a line that fills the terminal to the brim wraps on some.
    let room = columns.saturating_sub(NOTE_INDENT + 2 + COMMAND_WIDTH + 1);
    let value_width = room.clamp(VALUE_MIN, 58);
    // Over the commands: what that column is for.
    let title = pad(
        &style(title).bold().to_string(),
        LABEL_WIDTH + 2 + value_width + 2,
    );
    let hint = if commands { "or type" } else { "" };
    let heading = format!("{}{title}{}", " ".repeat(MARGIN), style(hint).dim());
    let mut lines = vec![heading.trim_end().to_string()];
    for (i, setting) in settings.iter().enumerate() {
        let selected = matches!(mark, Mark::Selected(at) if at == i);
        let sign = match mark {
            Mark::Changed(label) if label == setting.label => style("✓").green().bold().to_string(),
            Mark::Selected(_) if selected => orange().bold().apply_to("›").to_string(),
            _ => " ".to_string(),
        };
        let label = match selected {
            true => orange().bold().apply_to(setting.label).to_string(),
            false => setting.label.to_string(),
        };
        let value = pad_str(&setting.value, value_width, Alignment::Left, Some("…"));
        let value = match (selected, setting.nudge) {
            (true, _) => style(value).bold().to_string(),
            (false, true) => style(value).yellow().to_string(),
            (false, false) => style(value).dim().to_string(),
        };
        let command = if commands { &setting.command } else { "" };
        let line = format!(
            "{sign} {}  {value}  {}",
            pad(&label, LABEL_WIDTH),
            style(command).color256(240)
        );
        lines.push(line.trim_end().to_string());
        if let Some(note) = &setting.note {
            let note = pad_str(
                note,
                value_width + 2 + COMMAND_WIDTH,
                Alignment::Left,
                Some("…"),
            );
            lines.push(format!(
                "{}{}",
                " ".repeat(NOTE_INDENT),
                style(note.trim_end()).color256(240)
            ));
        }
    }
    lines.push(String::new());
    lines
}

/// The one thing most people come for, above the setup: put the arrow on
/// it and press enter.
pub fn start_line(what: &str, focused: bool) -> Vec<String> {
    let line = match focused {
        true => format!(
            "{} {}   {}",
            orange().bold().apply_to("›"),
            orange().bold().apply_to(what),
            style("press enter").dim()
        ),
        false => format!("{}{}", " ".repeat(MARGIN), orange().apply_to(what)),
    };
    vec![line, String::new()]
}

/// The keys of the interactive setup, under it. `enter` is what enter does
/// on the line the arrow is on, `back` where escape leads from there.
pub fn menu_keys(enter: &str, back: Option<&str>) -> Vec<String> {
    let mut keys = vec![key("enter", enter), key("arrows", "move")];
    if let Some(back) = back {
        keys.push(key("esc", back));
    }
    let commands = [
        key("l", "list"),
        key("c", "check"),
        key("e", "export"),
        key("s", "stats"),
        key("h", "help"),
        key("q", "quit"),
    ];
    vec![
        format!("  {}", keys.join("   ")),
        format!("  {} {}", style("or type:").dim(), commands.join("   ")),
    ]
}

/// The list that opens on a line of the setup: what there is to choose
/// from, each with what it means. `focused` is the line the arrow is on.
pub fn choices(setting: &Setting, focused: usize) -> Vec<String> {
    let mut lines = vec![format!(
        "{}{}",
        " ".repeat(MARGIN),
        style(setting.label).bold()
    )];
    let width = setting
        .choices
        .iter()
        .map(|choice| console::measure_text_width(&choice.label))
        .max()
        .unwrap_or(0);
    // A dot in front of what is in force now, and a hollow one in front of
    // the rest. A list of things to do has no dots.
    let dots = if setting.current.is_some() { 2 } else { 0 };
    let room = (Term::stdout().size().1 as usize).saturating_sub(MARGIN + dots + width + 3 + 1);
    for (i, choice) in setting.choices.iter().enumerate() {
        let on_it = i == focused;
        let arrow = match on_it {
            true => orange().bold().apply_to("›").to_string(),
            false => " ".to_string(),
        };
        let dot = match setting.current {
            Some(current) if current == i => format!("{} ", style("●").green()),
            Some(_) => format!("{} ", style("○").dim()),
            None => String::new(),
        };
        let label = pad(&choice.label, width);
        let label = match on_it {
            true => orange().bold().apply_to(label).to_string(),
            false => label,
        };
        let about = console::truncate_str(&choice.about, room, "…");
        let line = format!("{arrow} {dot}{label}   {}", style(about).dim());
        lines.push(line.trim_end().to_string());
    }
    lines.push(String::new());
    let keys = [
        key("enter", "choose"),
        key("arrows", "move"),
        key("esc", "back"),
    ];
    lines.push(format!("  {}", keys.join("   ")));
    lines
}

/// Print lines that were made to be drawn.
pub fn show(lines: &[String]) {
    for line in lines {
        println!("{line}");
    }
}

const HIDE_CURSOR: &str = "\x1b[?25l";
const SHOW_CURSOR: &str = "\x1b[?25h";

/// The lines of a picture that was left standing while a command runs: see
/// `Frame::park`.
static PARKED: AtomicUsize = AtomicUsize::new(0);

/// Straight to the terminal, in one piece.
fn write(text: &str) {
    use std::io::Write;
    let mut out = std::io::stdout().lock();
    let _ = out.write_all(text.as_bytes());
    let _ = out.flush();
}

/// A picture of the setup that was left standing makes room for what is
/// about to be printed. The `println!` of this crate calls this first.
pub fn make_room() {
    let lines = PARKED.swap(0, Ordering::Relaxed);
    if lines > 0 {
        write(&format!("\x1b[{lines}A\r\x1b[J{SHOW_CURSOR}"));
    }
}

/// The part of the terminal the setup and its lists are drawn on. A new
/// picture is written over the old one, line by line and in one piece, so
/// there is never a moment without a picture: nothing blinks.
#[derive(Default)]
pub struct Frame {
    /// How many lines the picture on screen takes.
    lines: usize,
}

impl Frame {
    /// Draw these lines where the last picture was.
    pub fn draw(&mut self, lines: &[String]) {
        let mut out = String::from(HIDE_CURSOR);
        if self.lines > 0 {
            out.push_str(&format!("\x1b[{}A", self.lines));
        }
        out.push('\r');
        for line in lines {
            // The line, and then whatever is left of a longer one.
            out.push_str(line);
            out.push_str("\x1b[K\n");
        }
        // And whatever is left of a taller picture.
        out.push_str("\x1b[J");
        write(&out);
        self.lines = lines.len();
    }

    /// Take the picture away: what is printed next starts where it began.
    pub fn clear(&mut self) {
        if self.lines > 0 {
            write(&format!("\x1b[{}A\r\x1b[J", self.lines));
        }
        self.leave();
    }

    /// Let the picture stand: what comes next goes under it.
    pub fn leave(&mut self) {
        self.lines = 0;
        write(SHOW_CURSOR);
    }

    /// Let the picture stand while a command runs. If the command has
    /// something to say, the picture makes room for it. If it has not, the
    /// next picture is drawn over this one, without a flicker.
    pub fn park(&self) {
        PARKED.store(self.lines, Ordering::Relaxed);
    }

    /// The command is done: is the picture still there?
    pub fn unpark(&mut self) {
        if PARKED.swap(0, Ordering::Relaxed) == 0 {
            self.lines = 0;
        }
    }
}

impl Drop for Frame {
    fn drop(&mut self) {
        PARKED.store(0, Ordering::Relaxed);
        write(SHOW_CURSOR);
    }
}

/// Wipe the last `lines` lines off the terminal, to draw over them.
pub fn wipe(lines: usize) {
    let _ = Term::stdout().clear_last_lines(lines);
}

/// The key that was pressed in the interactive setup.
#[derive(Clone, Copy)]
pub enum Pressed {
    Up,
    Down,
    /// Enter, space or →: do what the line the arrow is on is for.
    Go,
    /// Escape, ← or backspace.
    Back,
    /// The start of a typed command.
    Letter(char),
    Quit,
    Other,
}

/// Wait for a key.
pub fn press() -> Pressed {
    use console::Key;
    match Term::stdout().read_key_raw() {
        Ok(Key::ArrowUp) => Pressed::Up,
        Ok(Key::ArrowDown | Key::Tab) => Pressed::Down,
        Ok(Key::Enter | Key::ArrowRight | Key::Char(' ')) => Pressed::Go,
        Ok(Key::Escape | Key::ArrowLeft | Key::Backspace) => Pressed::Back,
        // Ctrl-D, and a terminal that went away.
        Ok(Key::CtrlC | Key::Char('\u{4}')) | Err(_) => Pressed::Quit,
        // Commands start with a letter (or a question mark, for help).
        Ok(Key::Char(letter)) if letter.is_alphabetic() || letter == '?' => Pressed::Letter(letter),
        Ok(_) => Pressed::Other,
    }
}

/// Can the setup be driven with the arrow keys here? Not when the input is
/// piped in, or when the terminal is too small to draw over it.
pub fn interactive(lines: usize) -> bool {
    use std::io::IsTerminal;
    let term = Term::stdout();
    let (rows, columns) = term.size();
    // A line that wraps or scrolls away would throw the drawing off.
    let fits = rows as usize > lines && columns as usize >= SETUP_WIDTH;
    std::io::stdin().is_terminal() && term.is_term() && fits
}

/// The narrowest terminal a line of the setup fits on, with a value at its
/// smallest.
const SETUP_WIDTH: usize = NOTE_INDENT + VALUE_MIN + 2 + COMMAND_WIDTH + 1;
// Which has to be a terminal of the usual 80 columns, or narrower.
const _: () = assert!(SETUP_WIDTH <= 80);

fn key(key: &str, what: &str) -> String {
    format!("{} {}", orange().bold().apply_to(key), style(what).dim())
}

/// What can be done from the home screen, three to a line. `next` is what
/// enter does: "start hunting", or "next 10 names" once the hunt is on.
pub fn actions(next: &str) {
    let rows = [
        [
            ("enter", next),
            ("l", "list the finds"),
            ("s", "statistics"),
        ],
        [
            ("c", "check a word"),
            ("e", "export to Excel"),
            ("r", "re-check old names"),
        ],
        [
            ("h", "all commands"),
            ("home", "this screen"),
            ("q", "quit"),
        ],
    ];
    println!("  {}", style("Actions").bold());
    for row in rows {
        let cells: Vec<String> = row
            .iter()
            .map(|(k, what)| {
                let key = pad(&orange().bold().apply_to(k).to_string(), 5);
                pad(&format!("{key} {}", style(what).dim()), 25)
            })
            .collect();
        println!("    {}", cells.join("").trim_end());
    }
    println!();
}

/// The one-line reminder under a round or a change.
pub fn bar(next: &str) {
    let keys = [
        key("enter", next),
        key("l", "list"),
        key("c", "check"),
        key("e", "export"),
        key("home", "setup"),
        key("h", "help"),
        key("q", "quit"),
    ];
    println!("  {}", keys.join("   "));
}

/// 12345 -> "12,345".
pub fn thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, digit) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

pub fn domain(label: &str, tld: &str) -> String {
    format!(
        "{}{}",
        style(label).bold(),
        orange().bold().apply_to(format!(".{tld}"))
    )
}

fn pad(text: &str, width: usize) -> String {
    pad_str(text, width, Alignment::Left, None).into_owned()
}

/// A block for the verdict on a name: filled and green when it is
/// available, hollow when it is taken, grainy when nobody could tell.
fn icon(status: Status) -> String {
    match status {
        Status::Available => style("■").green().bold().to_string(),
        Status::Taken => style("□").red().to_string(),
        Status::Unknown => style("▒").yellow().to_string(),
    }
}

/// "$ finance" for a category, "$ finance + • muziek" for a combination,
/// each mark in its own colour.
pub fn category(name: &str) -> String {
    name.split('+')
        .map(|part| {
            let (mark, color) = topics::mark(part);
            format!("{} {part}", style(mark).color256(color))
        })
        .collect::<Vec<_>>()
        .join(" + ")
}

/// The category cell of a row: the first category and how many more.
fn category_cell(r: &Record) -> String {
    let labels = topics::labels(r.topics.as_deref().unwrap_or_default());
    let Some(first) = labels.first() else {
        return style("·").dim().to_string();
    };
    let more = match labels.len() {
        1 => String::new(),
        n => format!(" +{}", n - 1),
    };
    let room = CATEGORY_WIDTH - console::measure_text_width(&more);
    format!(
        "{}{}",
        pad_str(&category(first), room, Alignment::Left, Some("…")).trim_end(),
        style(more).dim()
    )
}

const CATEGORY_WIDTH: usize = 13;

pub fn header() -> String {
    let h = |text: &str, width: usize| pad(&style(text).dim().underlined().to_string(), width);
    format!(
        "  {}  {} {} {} {}",
        h(" #", 2),
        h("name", 24),
        h("made of", 18),
        h("about", CATEGORY_WIDTH),
        h("spelling", 8)
    )
}

/// Where a domain starts in a row: under it goes what its word means.
const MEANING_INDENT: usize = 8;

/// The second line of an available name: a warning when the dictionary
/// flags the word (see `tone`), and what the word means.
fn about(r: &Record) -> Option<String> {
    let flags = r.tone.as_deref().filter(|t| !t.is_empty());
    let warning = flags.map(|flags| style(format!("⚠ {flags}")).yellow().to_string());
    // What the name reads as, when that is not quite the word.
    let typo = (r.kind != "exact").then(|| format!("reads \"{}\" ({})", r.spelled, r.kind));
    let meaning = r.meaning.clone().filter(|m| !m.is_empty());
    // Only Dutch words come with how many people know them.
    let known = r.prevalence.map(|p| format!("{p}% of the Dutch know it"));
    let rest: Vec<String> = typo.into_iter().chain(meaning).chain(known).collect();
    let rest = (!rest.is_empty()).then(|| match warning {
        Some(_) => style(format!("· {}", rest.join(" · "))).dim().to_string(),
        None => style(rest.join(" · ")).dim().to_string(),
    });
    let parts: Vec<String> = warning.into_iter().chain(rest).collect();
    (!parts.is_empty()).then(|| parts.join(" "))
}

/// One verified name. `number` is its place in the round, if it has one. An
/// available name comes with a second line of small print: what the name
/// reads as if it has a typo, what its word means, how well known it is and
/// whether it is one to look twice at, as far as the dictionary says.
pub fn row(number: Option<usize>, r: &Record) -> String {
    let number = number
        .map(|n| format!("{n:>2}"))
        .unwrap_or_else(|| "  ".into());
    let name = format!("{} {}", icon(r.status), domain(&r.label, &r.tld));
    let spelling = if r.kind == "exact" {
        // speed + box, for a word with an extra word.
        let combined = Affix::of(&r.label, &r.word).filter(|_| r.whole);
        let exact = match combined {
            Some(affix) => affix.spelled_out(&r.word.to_lowercase()),
            None if r.whole => "whole word".to_string(),
            None => "exact word".to_string(),
        };
        style(exact).green().to_string()
    } else {
        style("small typo").yellow().to_string()
    };
    let line = format!(
        "  {}  {} {} {} {}",
        style(number).dim(),
        pad(&name, 24),
        pad(&r.word, 18),
        pad(&category_cell(r), CATEGORY_WIDTH),
        spelling
    );
    // The small print never runs over the edge of the terminal.
    let room = (Term::stdout().size().1 as usize).saturating_sub(MEANING_INDENT + 1);
    let about = about(r).map(|about| console::truncate_str(&about, room, "…").into_owned());
    match (r.status, about) {
        (Status::Available, Some(about)) => {
            format!("{line}\n{}{about}", " ".repeat(MEANING_INDENT))
        }
        (Status::Available, None) => line,
        (Status::Taken, _) => style(console::strip_ansi_codes(&line)).dim().to_string(),
        (Status::Unknown, _) => format!("{line} {}", style(format!("· {}", r.detail)).dim()),
    }
}

/// The line under a round: what was found and checked, and a remark.
pub fn round_footer(number: usize, round: &Round, elapsed: Duration, remark: &str) {
    let found = round.hits.len();
    let mut parts = vec![
        style(format!("{found} available"))
            .green()
            .bold()
            .to_string(),
        format!("{} checked", round.checked),
        format!("{} taken", round.taken),
    ];
    if round.unknown > 0 {
        parts.push(format!("{} unclear", round.unknown));
    }
    parts.push(format!("{:.1}s", elapsed.as_secs_f32()));
    println!("  {}", style("─".repeat(76)).dim());
    let dot = style(" · ").dim().to_string();
    println!(
        "  {}{dot}{}",
        orange().bold().apply_to(format!("Round {number}")),
        parts.join(&dot)
    );
    println!("  {}", style(remark).italic());
    println!();
}

/// What there is to say about how a round went.
pub fn remark(round: &Round, wanted: usize, category: Option<&Category>, lang: Lang) -> String {
    let found = round.hits.len();
    let mut rng = rand::rng();
    if round.interrupted {
        "Stopped, everything checked so far is saved.".to_string()
    } else if round.stuck && round.retry && found < wanted {
        "No registry answered clearly just now. Press enter to ask again, or pick other extensions with `t`."
            .to_string()
    } else if round.stuck && found < wanted {
        "None of these extensions can be checked. Pick others with `t`.".to_string()
    } else if let Some(category) = category.filter(|_| round.exhausted && found < wanted) {
        format!(
            "That was every {} word {} has for these extensions. Try another one with `cat`, or `cat off` for the whole dictionary.",
            category.name,
            lang.dictionary()
        )
    } else if round.exhausted && found < wanted {
        "The dictionary has no more words for these extensions. Try `t all`, or `reset` to start over."
            .to_string()
    } else if round.capped && found < wanted {
        format!(
            "{} Not much free here: press enter to keep looking.",
            sighs(lang).choose(&mut rng).unwrap()
        )
    } else if found == 0 {
        sighs(lang).choose(&mut rng).unwrap().to_string()
    } else {
        cheers(lang).choose(&mut rng).unwrap().to_string()
    }
}

/// The languages words can come from.
pub fn languages(current: Lang) {
    for lang in Lang::ALL {
        println!(
            "  {} {} {}",
            pad(
                &orange()
                    .bold()
                    .apply_to(format!("lang {}", lang.code()))
                    .to_string(),
                8
            ),
            pad(
                &format!("{} words from {}", lang.name(), lang.dictionary()),
                36
            ),
            if lang == current {
                style("← hunting here").green().to_string()
            } else {
                String::new()
            }
        );
    }
    println!();
}

/// The built-in categories, and how to get at all the other topics.
pub fn categories(active: Option<&Category>, lang: Lang) {
    let dictionary = lang.dictionary();
    let example = lang.examples().topic;
    for bundle in BUNDLES {
        let on = active.is_some_and(|c| c.name.split('+').any(|part| part == bundle.name));
        println!(
            "  {} {} {}",
            pad(
                &format!(
                    "{} {}",
                    style(bundle.mark).color256(bundle.color),
                    orange().bold().apply_to(bundle.name)
                ),
                11
            ),
            pad(bundle.about, 40),
            if on {
                style("← hunting here").green().to_string()
            } else {
                String::new()
            }
        );
        // First names are not a topic: the English Wiktionary lists them.
        let made_of = match bundle.topics(lang) {
            [topics::NAMES] => format!("{} given names, as Wiktionary lists them", lang.name()),
            topics => topics.join(" · "),
        };
        println!("  {} {}", pad("", 11), style(made_of).dim());
    }
    println!();
    let tip = |command: &str, what: &str| {
        println!(
            "  {} {}",
            pad(&orange().bold().apply_to(command).to_string(), 22),
            style(what).dim()
        )
    };
    tip("cat finance", "only hunt among the words of a category");
    tip("cat finance tech", "or of several");
    tip("cat names", "first names: luc.as, or mike.ai with `w ai`");
    tip(
        &format!("cat {example}"),
        &format!("any {dictionary} topic works, `cat list` shows all of them"),
    );
    tip("cat off", "back to the whole dictionary");
    println!(
        "  {}",
        style(format!(
            "The words under the names are {dictionary}'s own topics. Its volunteers label what"
        ))
        .dim()
    );
    println!(
        "  {}",
        style("they get to, so a category is a good selection of words, never a complete one.")
            .dim()
    );
    println!();
}

/// The topics a dictionary files its words under, the biggest ones.
pub fn topic_list(all: &[(String, u64)], lang: Lang) {
    const COLUMNS: usize = 3;
    const SHOWN: usize = 240;
    let cells: Vec<String> = all
        .iter()
        .take(SHOWN)
        .map(|(topic, size)| {
            let name = match topics::bundle_of(topic) {
                // Part of a built-in category.
                Some(bundle) => format!(
                    "{} {}",
                    style(bundle.mark).color256(bundle.color),
                    orange().apply_to(topic)
                ),
                None => topic.clone(),
            };
            format!(
                "{} {}",
                pad_str(&name, 21, Alignment::Left, Some("…")),
                pad(&style(size).dim().to_string(), 6)
            )
        })
        .collect();
    for row in cells.chunks(COLUMNS) {
        println!("  {}", row.join(""));
    }
    println!();
    let more = match all.len().saturating_sub(SHOWN) {
        0 => String::new(),
        n => format!(" {n} smaller ones work too."),
    };
    info(
        &style(format!(
            "The {} biggest {} topics, with the number of words in each. Pick any with `cat <topic>`.{more}",
            all.len().min(SHOWN),
            lang.dictionary()
        ))
        .dim()
        .to_string(),
    );
    println!();
}

/// A group of commands in the help: its name, a remark, and per command
/// what it does.
type HelpGroup<'a> = (&'a str, &'a str, &'a [(&'a str, &'a str)]);

/// Every command, grouped by what it is for, in plain words and with
/// examples in the language of the hunt. And how to read a result.
pub fn help(lang: Lang) {
    let examples = lang.examples();
    let check = format!("try a word of your own: c {}", examples.word);
    let typos = format!("small spelling mistakes too, like {}", examples.typo);
    let whole = format!("the whole word before the dot ({})", examples.whole);
    let quit = examples.goodbye.to_lowercase();
    let groups: [HelpGroup; 4] = [
        (
            "Hunt",
            "",
            &[
                (
                    "enter  n  next",
                    "look for the next batch of available names",
                ),
                ("c  check <word>", &check),
            ],
        ),
        (
            "Setup",
            "or the arrow keys and enter on the home screen",
            &[
                ("lang en | nl | la | es", "the language the words come from"),
                ("syl 1 2 3", "word length in parts: cat 1, ba-na-na 3"),
                ("cat finance | list | off", "only words about one subject"),
                ("cat names", "first names: luc.as, or mike.ai with `w ai`"),
                (
                    "rude on | off",
                    "show rude words with a ⚠, or leave them out",
                ),
                ("w  whole [nl be] | off", &whole),
                ("typos all | off", &typos),
                (
                    "typos missing",
                    "one kind, or doubled dropped sound swapped",
                ),
                (
                    "before head | off",
                    "add a word before each word: headspeed",
                ),
                ("after box | off", "add a word after each word: speedbox"),
                (
                    "t  tlds ai io app | all",
                    "the domain extensions to search in: .ai .io",
                ),
                ("count 20", "how many names to find before it stops"),
                (
                    "contact you@example.com",
                    "tell the dictionary who you are: 20x faster",
                ),
            ],
        ),
        (
            "Results",
            "",
            &[
                (
                    "l  list [finance | en | .nl]",
                    "the available names found so far",
                ),
                (
                    "e  export [file.xlsx]",
                    "save everything that was checked to Excel",
                ),
                ("s  stats", "how many names were checked and found"),
                ("r  refresh", "check names again after 4 weeks"),
            ],
        ),
        (
            "More",
            "",
            &[
                ("v  verbose", "also show the names that are taken"),
                ("reset", "start again at the top of the dictionary"),
                ("home", "back to the setup screen"),
                ("q  quit", &quit),
            ],
        ),
    ];
    for (group, aside, rows) in groups {
        let title = format!("{} {}", style(group).bold(), style(aside).dim());
        println!("  {}", title.trim_end());
        for (command, what) in rows {
            println!(
                "    {} {}",
                pad(&orange().apply_to(command).to_string(), 30),
                style(what).dim()
            );
        }
        println!();
    }
    let reading = [
        ("name", "the domain name you could register"),
        ("made of", "the real word it is made of"),
        (
            "about",
            "what the word is about, when the dictionary says so",
        ),
        (
            "spelling",
            "the exact word, or the small mistake that makes it fit",
        ),
        ("⚠", "the word can also mean something rude: look twice"),
        (
            "parts",
            "the bits you say a word in (syllables): ba-na-na has 3",
        ),
        ("extension", "what comes after the dot: .nl, .ai, .app"),
    ];
    println!("  {}", style("Reading a result").bold());
    for (term, what) in reading {
        println!(
            "    {} {}",
            pad(&orange().apply_to(term).to_string(), 12),
            style(what).dim()
        );
    }
    println!();
}

pub fn stats(s: &Stats, by_tld: &[(String, usize, usize)], by_category: &[(String, usize, usize)]) {
    println!();
    println!(
        "  {} names checked, made of {} words, in {} extensions",
        style(s.total).bold(),
        style(s.words).bold(),
        style(s.tlds).bold()
    );
    println!(
        "  {}   {}   {}",
        style(format!("■ {} available", s.available)).green(),
        style(format!("□ {} taken", s.taken)).red(),
        style(format!("▒ {} unclear", s.unknown)).yellow(),
    );
    if s.stale > 0 {
        println!(
            "  {}",
            style(format!(
                "• {} were checked over 4 weeks ago, `r` checks them again",
                s.stale
            ))
            .yellow()
        );
    }
    if !by_category.is_empty() {
        let cells: Vec<String> = by_category
            .iter()
            .take(6)
            .map(|(label, checked, free)| {
                format!(
                    "{} {}",
                    category(label),
                    style(format!("{free} of {checked} free")).dim()
                )
            })
            .collect();
        println!("  {}", cells.join(&style("  ·  ").dim().to_string()));
    }
    if !by_tld.is_empty() {
        let width = by_tld
            .iter()
            .map(|(_, checked, _)| *checked)
            .max()
            .unwrap_or(1)
            .max(1);
        println!();
        for (tld, checked, free) in by_tld.iter().take(12) {
            let bar = |n: usize| (n * 30).div_ceil(width);
            println!(
                "  {} {}{} {}",
                pad(&orange().bold().apply_to(format!(".{tld}")).to_string(), 7),
                style("█".repeat(bar(*free))).green(),
                style("█".repeat(bar(*checked) - bar(*free))).color256(238),
                style(format!("{free} of {checked} free")).dim()
            );
        }
        if by_tld.len() > 12 {
            println!(
                "  {}",
                style(format!("… and {} more extensions", by_tld.len() - 12)).dim()
            );
        }
    }
    println!();
}

pub fn info(text: &str) {
    println!("  {text}");
}

pub fn warn(text: &str) {
    println!("  {} {}", style("!").yellow().bold(), style(text).yellow());
}

pub fn error(text: &str) {
    println!("  {} {}", style("✗").red().bold(), style(text).red());
}

/// The day of a moment, where the user is: 2026-10-12.
pub fn local_day(timestamp: i64) -> String {
    Local
        .timestamp_opt(timestamp, 0)
        .single()
        .map(|t| t.format("%Y-%m-%d").to_string())
        .unwrap_or_default()
}

pub fn local_time(timestamp: i64) -> String {
    Local
        .timestamp_opt(timestamp, 0)
        .single()
        .map(|t| t.format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_default()
}

/// A spinner that keeps a status line alive while rows are printed above it.
pub struct Spinner {
    bar: ProgressBar,
}

impl Spinner {
    pub fn start() -> Self {
        make_room();
        let bar = ProgressBar::new_spinner();
        bar.set_style(
            ProgressStyle::with_template("  {spinner:.208} {wide_msg}")
                .expect("valid spinner template")
                .tick_strings(&["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏", " "]),
        );
        bar.enable_steady_tick(Duration::from_millis(80));
        Self { bar }
    }

    pub fn status(&self, text: &str) {
        self.bar.set_message(text.to_string());
    }

    /// Print a line above the spinner (also works when output is piped).
    pub fn line(&self, text: &str) {
        self.bar.suspend(|| println!("{text}"));
    }

    pub fn stop(self) {
        self.bar.finish_and_clear();
    }
}

/// "■ 3/10 · □ 12" style running tally for the spinner.
pub fn tally(hits: usize, wanted: usize, taken: usize, unknown: usize, doing: &str) -> String {
    let unclear = if unknown > 0 {
        format!(" · {} {unknown}", icon(Status::Unknown))
    } else {
        String::new()
    };
    format!(
        "{} {hits}/{wanted} · {} {taken}{unclear}  {}",
        icon(Status::Available),
        icon(Status::Taken),
        style(doing).dim()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(text: &str) -> String {
        console::strip_ansi_codes(text).into_owned()
    }

    #[test]
    fn a_found_name_is_a_row_with_small_print() {
        let record = Record {
            kind: "doubled letter".into(),
            spelled: "snapp".into(),
            word: "snap".into(),
            topics: Some("Banking, Music".into()),
            meaning: Some("A sharp sound".into()),
            tone: Some("vulgar".into()),
            ..Record::sample("sn.app", 0)
        };
        let row = plain(&row(Some(7), &record));
        let (first, second) = row.split_once('\n').expect("two lines");
        assert!(first.starts_with("   7  ■ sn.app"), "{first}");
        assert!(
            first.contains("snap") && first.contains("$ finance +1"),
            "{first}"
        );
        assert!(first.trim_end().ends_with("small typo"), "{first}");
        // The small print: the warning first, then what it reads as and means.
        assert!(second.starts_with(&" ".repeat(MEANING_INDENT)));
        assert_eq!(
            second.trim(),
            "⚠ vulgar · reads \"snapp\" (doubled letter) · A sharp sound"
        );
        // Dutch words come with how many people know them.
        let dutch = Record {
            prevalence: Some(99),
            ..Record::sample("leni.ng", 0)
        };
        let row = plain(&super::row(Some(1), &dutch));
        assert_eq!(
            row.split_once('\n').unwrap().1.trim(),
            "99% of the Dutch know it"
        );
        // What does not fit the window is cut short, not wrapped.
        let wordy = Record {
            meaning: Some("A quick breaking sound, ".repeat(8)),
            ..record.clone()
        };
        let row = plain(&super::row(Some(7), &wordy));
        let second = row.split_once('\n').unwrap().1;
        assert!(
            second.ends_with('…') && console::measure_text_width(second) < 80,
            "{second}"
        );
        // The header names the columns the rows have.
        let header = plain(&header());
        for column in ["name", "made of", "about", "spelling"] {
            assert!(header.contains(column), "{header}");
        }
    }

    #[test]
    fn a_row_says_no_more_than_there_is_to_say() {
        let exact = Record::sample("cook.ing", 0);
        let row_text = plain(&row(Some(1), &exact));
        assert!(!row_text.contains('\n'), "{row_text}");
        assert!(row_text.contains("exact word"));
        // A taken name and an unclear one are one line each, without a number.
        let taken = Record {
            status: Status::Taken,
            meaning: Some("something".into()),
            ..Record::sample("bak.ing", 0)
        };
        let line = plain(&row(None, &taken));
        assert!(line.contains("□ bak.ing") && !line.contains('\n'), "{line}");
        let unclear = Record {
            status: Status::Unknown,
            detail: "asked too often, waiting a bit".into(),
            ..Record::sample("mak.ing", 0)
        };
        let line = plain(&row(None, &unclear));
        assert!(
            line.contains("▒ mak.ing") && line.ends_with("· asked too often, waiting a bit"),
            "{line}"
        );
        // A whole word, and a word with one of your own.
        let whole = Record {
            whole: true,
            word: "lemon".into(),
            ..Record::sample("lemon.nl", 0)
        };
        assert!(plain(&row(Some(1), &whole)).contains("whole word"));
        let boxed = Record {
            whole: true,
            word: "lemon".into(),
            ..Record::sample("lemonbox.nl", 0)
        };
        assert!(plain(&row(Some(1), &boxed)).contains("lemon + box"));
    }

    #[test]
    fn a_round_ends_with_a_fitting_remark() {
        let round = |hits: usize| Round {
            hits: (0..hits)
                .map(|i| Record::sample(&format!("w{i}.ing"), 0))
                .collect(),
            ..Round::default()
        };
        let say =
            |round: &Round, category: Option<&Category>| remark(round, 10, category, Lang::English);
        let stopped = Round {
            interrupted: true,
            ..round(3)
        };
        assert!(say(&stopped, None).starts_with("Stopped"));
        let dry = Round {
            exhausted: true,
            ..round(2)
        };
        assert!(say(&dry, None).contains("no more words for these extensions"));
        let finance = Category {
            name: "finance".into(),
            topics: vec!["Banking".into()],
        };
        assert!(say(&dry, Some(&finance)).contains("every finance word Wiktionary has"));
        // No extension left is not the end of the dictionary.
        let timed_out = Round {
            stuck: true,
            retry: true,
            ..round(2)
        };
        assert!(say(&timed_out, None).contains("Press enter to ask again"));
        let unusable = Round {
            stuck: true,
            ..round(0)
        };
        assert_eq!(
            say(&unusable, None),
            "None of these extensions can be checked. Pick others with `t`."
        );
        let crowded = Round {
            capped: true,
            ..round(1)
        };
        assert!(say(&crowded, None).ends_with("Not much free here: press enter to keep looking."));
        // A full round gets a cheer, an empty one a sigh: one of the language's own.
        assert!(cheers(Lang::English).contains(&say(&round(10), None).as_str()));
        assert!(sighs(Lang::English).contains(&say(&round(0), None).as_str()));
        // Enough names is enough, even when the dictionary ran out as well.
        let full = Round {
            exhausted: true,
            ..round(10)
        };
        assert!(cheers(Lang::English).contains(&say(&full, None).as_str()));
        for lang in Lang::ALL {
            assert!(!cheers(lang).is_empty() && !sighs(lang).is_empty());
        }
    }

    #[test]
    fn numbers_days_and_names_read_well() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1_000), "1,000");
        assert_eq!(thousands(12_345_678), "12,345,678");
        assert_eq!(plain(&domain("cook", "ing")), "cook.ing");
        assert_eq!(plain(&category("finance")), "$ finance");
        assert_eq!(plain(&category("finance+music")), "$ finance + • music");
        let day = local_day(1_700_000_000);
        assert_eq!(day.len(), 10);
        assert!(day.starts_with("2023-11-1"), "{day}");
        assert!(local_time(1_700_000_000).starts_with(&day));
        let tally = plain(&tally(3, 10, 12, 1, "checking sn.app"));
        assert!(
            tally.contains("3/10") && tally.contains("12") && tally.ends_with("checking sn.app"),
            "{tally}"
        );
    }

    #[test]
    fn the_keys_of_a_screen_are_spelled_out() {
        let start = start_line("Start hunting", true);
        assert_eq!(plain(&start[0]), "› Start hunting   press enter");
        assert_eq!(
            plain(&start_line("Start hunting", false)[0]),
            "  Start hunting"
        );
        let keys = menu_keys("start", None);
        assert_eq!(plain(&keys[0]), "  enter start   arrows move");
        assert!(plain(&keys[1]).starts_with("  or type: l list"));
        let keys = menu_keys("change this", Some("back"));
        assert_eq!(
            plain(&keys[0]),
            "  enter change this   arrows move   esc back"
        );
    }

    fn line(label: &'static str, value: &str, note: Option<&str>) -> Setting {
        Setting {
            label,
            value: value.to_string(),
            note: note.map(str::to_string),
            command: "lang nl|la|es".to_string(),
            nudge: false,
            choices: Vec::new(),
            current: None,
        }
    }

    #[test]
    fn the_setup_lines_up() {
        let settings = [
            line("Language", "English", None),
            line("Word length", &"x".repeat(200), Some("small print")),
        ];
        for mark in [Mark::None, Mark::Selected(1), Mark::Changed("Language")] {
            let lines = setup("Setup", &settings, mark, true);
            let plain: Vec<String> = lines
                .iter()
                .map(|line| console::strip_ansi_codes(line).into_owned())
                .collect();
            // A title, two settings, a line of small print and a blank one.
            assert_eq!(plain.len(), 5);
            let start =
                |line: &str, text: &str| line.find(text).map(|at| line[..at].chars().count());
            // Title and labels share one left edge, values and notes another.
            assert_eq!(start(&plain[0], "Setup"), Some(MARGIN));
            assert_eq!(start(&plain[1], "Language"), Some(MARGIN));
            assert_eq!(start(&plain[2], "Word length"), Some(MARGIN));
            assert_eq!(start(&plain[1], "English"), Some(NOTE_INDENT));
            assert_eq!(start(&plain[2], "xxx"), Some(NOTE_INDENT));
            assert_eq!(start(&plain[3], "small print"), Some(NOTE_INDENT));
            assert_eq!(start(&plain[0], "or type"), start(&plain[1], "lang nl"));
            // Nothing reaches the last column of the 80 a test runs in.
            assert!(
                plain
                    .iter()
                    .all(|line| console::measure_text_width(line) < 80)
            );
            assert!(plain[4].is_empty());
        }
    }

    #[test]
    fn a_list_lines_up() {
        let pick = |label: &str| Choice {
            label: label.to_string(),
            about: "what it means".to_string(),
            does: Does::Run(String::new()),
        };
        let mut setting = line("Language", "English", None);
        setting.choices = vec![pick("English"), pick("Dutch")];
        for current in [Some(0), None] {
            setting.current = current;
            let lines = choices(&setting, 1);
            let plain: Vec<String> = lines
                .iter()
                .map(|line| console::strip_ansi_codes(line).into_owned())
                .collect();
            let start =
                |line: &str, text: &str| line.find(text).map(|at| line[..at].chars().count());
            // With dots the labels come after them, without they take their place.
            let edge = MARGIN + if current.is_some() { 2 } else { 0 };
            assert_eq!(start(&plain[0], "Language"), Some(MARGIN));
            assert_eq!(start(&plain[1], "English"), Some(edge));
            assert_eq!(start(&plain[2], "Dutch"), Some(edge));
            assert!(plain[2].starts_with('›'));
            assert_eq!(start(&plain[1], "what"), start(&plain[2], "what"));
        }
    }
}
