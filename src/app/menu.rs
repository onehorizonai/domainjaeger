//! The interactive shell: the home screen with its lists, driven with the
//! arrow keys, and the prompt for typed commands.

use rustyline::DefaultEditor;
use rustyline::error::ReadlineError;

use super::*;

/// What the interactive setup was left for.
pub(super) enum Picked {
    /// Enter: go hunting.
    Hunt,
    /// The first letter of a typed command.
    Typed(char),
    Quit,
}

/// The interactive shell. It opens on the home screen: how the hunt is set
/// up and what can be done. Nothing is looked up until you say so. In a
/// terminal the setup is driven with the arrow keys and space; typing a
/// command works there too, and is all there is when the input is piped in.
pub(super) async fn shell(app: &mut App) -> Result<()> {
    ui::banner(app.hunter.settings.lang);
    let mut editor = DefaultEditor::new()?;
    let prompt = format!("  {} ", ui::orange().bold().apply_to("domainjaeger ›"));
    // Room for the tallest thing that is drawn: the setup or one of its lists.
    app.interactive = ui::interactive(app.settings()?.tallest());
    if !app.interactive {
        app.home()?;
    }
    let mut at_setup = app.interactive;
    loop {
        let mut typed = String::new();
        if at_setup {
            match app.menu(&mut editor).await? {
                Picked::Hunt => {
                    at_setup = matches!(app.execute("").await, After::Setup);
                    continue;
                }
                Picked::Typed(letter) => typed.push(letter),
                Picked::Quit => break,
            }
        }
        let read = || editor.readline_with_initial(&prompt, (&typed, ""));
        let line = match tokio::task::block_in_place(read) {
            Ok(line) => line,
            Err(ReadlineError::Interrupted | ReadlineError::Eof) => break,
            Err(e) => return Err(e.into()),
        };
        let line = line.trim();
        let _ = editor.add_history_entry(line);
        println!();
        match app.execute(line).await {
            After::Quit => break,
            After::Setup => at_setup = true,
            After::Prompt => at_setup = false,
        }
    }
    let stats = app.cache.stats()?;
    ui::info(&format!(
        "{}  {} names saved. `domainjaeger export` puts them in Excel.",
        app.hunter.settings.lang.examples().goodbye,
        style(ui::thousands(stats.available)).green().bold()
    ));
    Ok(())
}

impl App {
    /// The interactive setup: the arrows move over Start and the lines of
    /// the setup, enter does what the line is for. On Start that is going
    /// hunting, on a setting it opens its list of choices, on the last line
    /// the settings that are seldom touched. Nothing else starts a hunt, so
    /// a slip of the finger never does. Returns when there is something
    /// else to do.
    pub(super) async fn menu(&mut self, editor: &mut DefaultEditor) -> Result<Picked> {
        let mut frame = ui::Frame::default();
        loop {
            let setup = self.settings()?;
            // Either the home screen, or what is behind its last line.
            let (title, rows) = match self.advanced {
                Some(_) => (ADVANCED, &setup.advanced),
                None => ("Setup", &setup.main),
            };
            let lines = match self.advanced {
                Some(at) => {
                    let at = at.min(rows.len() - 1);
                    self.advanced = Some(at);
                    let mut lines = ui::setup(title, rows, ui::Mark::Selected(at), true);
                    lines.extend(ui::menu_keys(enter_on(&rows[at]), Some("back")));
                    lines
                }
                None => {
                    // Line 0 is Start, the settings follow.
                    self.selected = self.selected.min(rows.len());
                    let (mark, enter, back) = match self.selected.checked_sub(1) {
                        Some(line) => (
                            ui::Mark::Selected(line),
                            enter_on(&rows[line]),
                            Some("back to start"),
                        ),
                        None => (ui::Mark::None, "start", None),
                    };
                    let mut lines = ui::start_line(&self.start(), self.selected == 0);
                    lines.extend(ui::setup(title, rows, mark, true));
                    lines.extend(ui::menu_keys(enter, back));
                    lines
                }
            };
            frame.draw(&lines);
            let pressed = tokio::task::block_in_place(ui::press);
            let command = match (pressed, self.advanced) {
                (ui::Pressed::Other, _) => continue,
                (ui::Pressed::Up, Some(at)) => {
                    self.advanced = Some(at.checked_sub(1).unwrap_or(rows.len() - 1));
                    continue;
                }
                (ui::Pressed::Down, Some(at)) => {
                    self.advanced = Some((at + 1) % rows.len());
                    continue;
                }
                // Back on the home screen, on the line that led here.
                (ui::Pressed::Back | ui::Pressed::Erase, Some(_)) => {
                    self.advanced = None;
                    continue;
                }
                (ui::Pressed::Up, None) => {
                    self.selected = self.selected.checked_sub(1).unwrap_or(rows.len());
                    continue;
                }
                (ui::Pressed::Down, None) => {
                    self.selected = (self.selected + 1) % (rows.len() + 1);
                    continue;
                }
                (ui::Pressed::Back | ui::Pressed::Erase, None) => {
                    self.selected = 0;
                    continue;
                }
                (ui::Pressed::Go, None) if self.selected == rows.len() => {
                    self.advanced = Some(0);
                    continue;
                }
                (ui::Pressed::Go, Some(at)) if rows[at].several => {
                    if self.several(&mut frame, editor, rows[at].label).await? {
                        frame.clear();
                        return Ok(Picked::Quit);
                    }
                    continue;
                }
                (ui::Pressed::Go, None) if self.selected > 0 && rows[self.selected - 1].several => {
                    if self
                        .several(&mut frame, editor, rows[self.selected - 1].label)
                        .await?
                    {
                        frame.clear();
                        return Ok(Picked::Quit);
                    }
                    continue;
                }
                (ui::Pressed::Go, Some(at)) => match choose(&mut frame, editor, &rows[at]) {
                    Some(command) => command,
                    None => continue,
                },
                (ui::Pressed::Go, None) if self.selected > 0 => {
                    match choose(&mut frame, editor, &rows[self.selected - 1]) {
                        Some(command) => command,
                        None => continue,
                    }
                }
                (ui::Pressed::Go | ui::Pressed::Letter(_) | ui::Pressed::Quit, _) => {
                    // The setup stays on screen the way it was left.
                    frame.draw(&ui::setup(title, rows, ui::Mark::None, true));
                    frame.leave();
                    return Ok(match pressed {
                        ui::Pressed::Go => Picked::Hunt,
                        ui::Pressed::Letter(letter) => Picked::Typed(letter),
                        _ => Picked::Quit,
                    });
                }
            };
            // The setup stays where it is while the command runs. When the
            // command has something to say (a list, an error), the setup
            // makes room for it and is drawn again below. When it has not,
            // the new setup is drawn over the old one.
            frame.park();
            let after = self.execute(&command).await;
            frame.unpark();
            if let After::Quit = after {
                frame.clear();
                return Ok(Picked::Quit);
            }
        }
    }

    /// The list of a setting where several choices can be on at once, like
    /// the categories. Enter turns the one the arrow is on on or off, right
    /// away, and the list stays open until escape. Returns whether to quit.
    async fn several(
        &mut self,
        frame: &mut ui::Frame,
        editor: &mut DefaultEditor,
        label: &str,
    ) -> Result<bool> {
        let mut focused = None;
        loop {
            // What is on changes with every press, and so do the lines.
            let setup = self.settings()?;
            let lines = setup.main.iter().chain(&setup.advanced);
            let Some(setting) = lines.into_iter().find(|line| line.label == label) else {
                return Ok(false);
            };
            let last = setting.choices.len() - 1;
            let at = focused.or(setting.current).unwrap_or(0).min(last);
            frame.draw(&ui::choices(setting, at));
            let choice = &setting.choices[at];
            let command = match tokio::task::block_in_place(ui::press) {
                ui::Pressed::Up => {
                    focused = Some(at.checked_sub(1).unwrap_or(last));
                    continue;
                }
                ui::Pressed::Down => {
                    focused = Some((at + 1) % (last + 1));
                    continue;
                }
                ui::Pressed::Back | ui::Pressed::Erase | ui::Pressed::Quit => return Ok(false),
                ui::Pressed::Letter(_) | ui::Pressed::Other => continue,
                ui::Pressed::Go => match &choice.does {
                    ui::Does::Run(command) => command.clone(),
                    ui::Does::Ask { question, command } => {
                        match answer(frame, editor, setting.label, question, command) {
                            Some(command) => command,
                            None => continue,
                        }
                    }
                    ui::Does::Browse(_) => {
                        focused = Some(at);
                        if self.subjects(frame, &choice.label).await? {
                            return Ok(true);
                        }
                        continue;
                    }
                },
            };
            focused = Some(at);
            frame.park();
            let after = self.execute(&command).await;
            frame.unpark();
            if let After::Quit = after {
                return Ok(true);
            }
        }
    }

    /// Every subject of the dictionary, as `cat list` shows them, to turn
    /// on or off one by one: the ones that are on first, then the biggest.
    /// Letters find one. Returns whether to quit.
    async fn subjects(&mut self, frame: &mut ui::Frame, title: &str) -> Result<bool> {
        let lang = self.hunter.settings.lang;
        frame.park();
        let spinner = self.asking_for_topics(true);
        let all = topic_list(&self.cache, &self.wiki, lang).await;
        if let Some(spinner) = spinner {
            spinner.stop();
        }
        frame.unpark();
        let mut all = match all {
            Ok(all) => all,
            Err(e) => {
                ui::error(&format!("{e:#}"));
                return Ok(false);
            }
        };
        // `cat finance` is the built-in finance, never the topic Finance alone.
        all.retain(|(topic, _)| !topics::is_built_in(topic));
        // In the order they had when the list opened, so that nothing moves
        // while things are turned on and off.
        let picked = setup::picked(&self.hunter.settings);
        all.sort_by_key(|(topic, _)| {
            let at = picked.iter().position(|part| *part == topic.to_lowercase());
            at.unwrap_or(usize::MAX)
        });
        let title = title.trim_end_matches('…');
        let room = (ui::rows().saturating_sub(8)).clamp(3, 15);
        let (mut find, mut focused) = (String::new(), 0);
        loop {
            let picked = setup::picked(&self.hunter.settings);
            let picked: Vec<&str> = picked.iter().map(String::as_str).collect();
            let choices: Vec<ui::Choice> = all
                .iter()
                .filter(|(topic, _)| topic.to_lowercase().contains(&find))
                .map(|(topic, size)| setup::subject(&picked, topic, *size))
                .collect();
            focused = focused.min(choices.len().saturating_sub(1));
            frame.draw(&ui::long_list(title, &choices, focused, &find, room));
            let last = choices.len().saturating_sub(1);
            let command = match tokio::task::block_in_place(ui::press) {
                ui::Pressed::Up => {
                    focused = focused.checked_sub(1).unwrap_or(last);
                    continue;
                }
                ui::Pressed::Down => {
                    focused = if focused >= last { 0 } else { focused + 1 };
                    continue;
                }
                ui::Pressed::Letter(letter) if letter.is_alphabetic() => {
                    find.extend(letter.to_lowercase());
                    focused = 0;
                    continue;
                }
                ui::Pressed::Erase if !find.is_empty() => {
                    find.pop();
                    focused = 0;
                    continue;
                }
                ui::Pressed::Back | ui::Pressed::Erase | ui::Pressed::Quit => return Ok(false),
                ui::Pressed::Letter(_) | ui::Pressed::Other => continue,
                ui::Pressed::Go => match choices.get(focused).map(|choice| &choice.does) {
                    Some(ui::Does::Run(command)) => command.clone(),
                    _ => continue,
                },
            };
            frame.park();
            let after = self.execute(&command).await;
            frame.unpark();
            if let After::Quit = after {
                return Ok(true);
            }
        }
    }
}

/// What enter does on a line of the setup: a line without a choice in force
/// is a list of things to do, or a door to more settings.
fn enter_on(setting: &ui::Setting) -> &'static str {
    match setting.current {
        Some(_) => "change this",
        None => "open",
    }
}

/// The list of a setting: the arrows move, enter picks, escape goes back.
/// Returns the command of what was picked, or nothing when nothing changes.
fn choose(
    frame: &mut ui::Frame,
    editor: &mut DefaultEditor,
    setting: &ui::Setting,
) -> Option<String> {
    let mut focused = setting.current.unwrap_or(0);
    loop {
        frame.draw(&ui::choices(setting, focused));
        match tokio::task::block_in_place(ui::press) {
            ui::Pressed::Up => {
                focused = focused.checked_sub(1).unwrap_or(setting.choices.len() - 1)
            }
            ui::Pressed::Down => focused = (focused + 1) % setting.choices.len(),
            ui::Pressed::Back | ui::Pressed::Erase | ui::Pressed::Quit => return None,
            ui::Pressed::Letter(_) | ui::Pressed::Other => {}
            ui::Pressed::Go => match &setting.choices[focused].does {
                // What is in force already needs no doing.
                ui::Does::Run(_) if setting.current == Some(focused) => return None,
                ui::Does::Run(command) => return Some(command.clone()),
                ui::Does::Ask { question, command } => {
                    if let Some(command) = answer(frame, editor, setting.label, question, command) {
                        return Some(command);
                    }
                }
                // Only a list of several opens another list.
                ui::Does::Browse(command) => return Some(command.clone()),
            },
        }
    }
}

/// The question of a choice, in the place of its list: `command <answer>`,
/// or nothing when nothing was typed.
fn answer(
    frame: &mut ui::Frame,
    editor: &mut DefaultEditor,
    label: &str,
    question: &str,
    command: &str,
) -> Option<String> {
    frame.clear();
    println!("  {}", style(label).bold());
    let prompt = ui::orange().bold().apply_to(format!("{question} ›"));
    let prompt = format!("  {prompt} ");
    let answer = tokio::task::block_in_place(|| editor.readline(&prompt));
    ui::wipe(2);
    let answer = answer.ok()?;
    let answer = answer.trim();
    (!answer.is_empty()).then(|| format!("{command} {answer}"))
}
