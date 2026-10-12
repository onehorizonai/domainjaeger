//! Hunting: a round of the dictionary, a word of your own, and another
//! look at names that were checked long ago.

use super::*;

impl App {
    /// One round of hunting, printed as it happens.
    pub(super) async fn round(&mut self) -> Result<Round> {
        self.rounds += 1;
        let started = Instant::now();
        println!("{}", ui::header());
        loop {
            let (round, skipped) = self.pass().await?;
            let wanted = self.hunter.settings.count;
            let lang = self.hunter.settings.lang;
            if !self.ran_dry(&round) {
                let category = self.hunter.settings.category.as_ref();
                let remark = ui::remark(&round, wanted, category, lang);
                ui::round_footer(self.rounds, &round, started.elapsed(), &remark);
                if !self.focused && !skipped.is_empty() && self.verbose {
                    ui::info(
                        &style(format!(
                            "Skipped (cannot be verified): {}",
                            skipped.join(" ")
                        ))
                        .dim()
                        .to_string(),
                    );
                }
                self.tip(&round);
                return Ok(round);
            }
            // Hardly any word ends in com or nl. Rather than stop there,
            // the hunt goes on with the whole word before the dot.
            let tlds = self.hunter.tlds();
            if round.checked > 0 {
                let remark = "That was every word that ends in these extensions.";
                ui::round_footer(self.rounds, &round, started.elapsed(), remark);
            }
            self.set_whole(&tlds.join(" "))?;
            let (word, _) = lang.examples().whole.split_once('.').unwrap_or_default();
            ui::info(&format!(
                "No words left that end in {}. From here the whole word goes before the dot, like {}.",
                self.dotted(&tlds),
                ui::domain(word, &tlds[0])
            ));
            ui::info(
                &style("Name style is now Whole word. `whole off` puts the dot back inside.")
                    .dim()
                    .to_string(),
            );
            println!();
        }
    }

    /// The dot-inside style has nothing left for the extensions that were
    /// picked, and the round is not full yet.
    pub(super) fn ran_dry(&self, round: &Round) -> bool {
        let settings = &self.hunter.settings;
        round.exhausted
            && !round.interrupted
            && round.hits.len() < settings.count
            && self.focused
            && !settings.whole
    }

    /// Where nearly everything is taken, a word of your own opens things up.
    pub(super) fn tip(&self, round: &Round) {
        let settings = &self.hunter.settings;
        let crowded = round.capped && round.hits.len() < settings.count;
        if !crowded || !settings.whole || !settings.affix.is_empty() {
            return;
        }
        let tld = self.hunter.tlds().into_iter().next().unwrap_or_default();
        let (word, _) = settings
            .lang
            .examples()
            .whole
            .split_once('.')
            .unwrap_or_default();
        ui::info(
            &style(format!(
                "Nearly every word is taken here. A word of your own helps: `after box` looks for {word}box.{tld}."
            ))
            .dim()
            .to_string(),
        );
    }

    /// One go at the dictionary with the setup as it is: rows as they are
    /// found, and what was left out along the way.
    pub(super) async fn pass(&mut self) -> Result<(Round, Vec<String>)> {
        let wanted = self.hunter.settings.count;
        let (verbose, focused) = (self.verbose, self.focused);
        let spinner = ui::Spinner::start();
        let (mut hits, mut taken, mut unknown) = (0, 0, 0);
        let mut doing = String::new();
        let mut skipped: Vec<String> = Vec::new();
        let mut on_event = |event: Event| {
            match event {
                Event::Busy(text) => doing = text,
                Event::Checked(record) => match record.status {
                    Status::Available => {
                        hits += 1;
                        spinner.line(&ui::row(Some(hits), &record));
                    }
                    status => {
                        if status == Status::Taken {
                            taken += 1
                        } else {
                            unknown += 1
                        }
                        if verbose {
                            spinner.line(&ui::row(None, &record));
                        }
                    }
                },
                Event::Skipped { tld, reason } => {
                    if focused {
                        spinner.line(&format!(
                            "  {} .{tld} {}",
                            style("!").yellow().bold(),
                            style(reason).yellow()
                        ));
                    }
                    skipped.push(format!(".{tld}"));
                }
            }
            spinner.status(&ui::tally(hits, wanted, taken, unknown, &doing));
        };
        let round = self.hunter.round(&mut on_event).await;
        spinner.stop();
        Ok((round?, skipped))
    }

    /// In the shell a failed round is reported, not fatal.
    pub(super) async fn round_or_report(&mut self) {
        if let Err(e) = self.round().await {
            ui::error(&format!("{e:#}"));
        }
    }

    pub(super) async fn check_word(&mut self, word: &str) -> Result<()> {
        let word = word.trim().to_lowercase();
        if !hacks::is_plain(&word) {
            bail!("\"{word}\" has to be plain a-z to fit in a domain name");
        }
        let spinner = ui::Spinner::start();
        let mut on_event = |event: Event| {
            if let Event::Busy(text) = event {
                spinner.status(&text);
            }
        };
        let result = self.hunter.check_word(&word, &mut on_event).await;
        spinner.stop();
        let Looked {
            known: in_dictionary,
            meaning,
            tone,
            mut records,
        } = result?;

        let syllables = records.first().and_then(|r| r.syllables);
        let dictionary = self.hunter.settings.lang.dictionary();
        let about = match (in_dictionary, syllables) {
            (true, Some(n)) => format!(
                "in {dictionary} · you say it in {n} part{}",
                if n == 1 { "" } else { "s" }
            ),
            (true, None) => {
                format!("in {dictionary} · longer than the word length you set, or not counted")
            }
            (false, _) => format!("not a word {dictionary} knows"),
        };
        ui::info(&format!(
            "{} {}",
            style(&word).bold(),
            style(format!("({about})")).dim()
        ));
        let warning = (!tone.is_empty()).then(|| style(format!("⚠ {tone}")).yellow().to_string());
        let meaning = meaning.map(|meaning| style(meaning).dim().to_string());
        if warning.is_some() || meaning.is_some() {
            let parts: Vec<String> = warning.into_iter().chain(meaning).collect();
            ui::info(&parts.join(&style(" · ").dim().to_string()));
        }
        if records.is_empty() {
            ui::info(
                "No extension fits the end of this word, not even with a small spelling mistake.",
            );
        } else {
            records.sort_by_key(|r| {
                (
                    r.status != Status::Available,
                    r.kind != "exact",
                    r.domain.clone(),
                )
            });
            println!("{}", ui::header());
            for (i, record) in records.iter().enumerate() {
                println!("{}", ui::row(Some(i + 1), record));
            }
        }
        println!();
        Ok(())
    }

    pub(super) async fn refresh(&mut self) -> Result<()> {
        let stale = self.cache.stale()?;
        if stale.is_empty() {
            ui::info("Nothing to check again: every name was checked less than 4 weeks ago.");
            return Ok(());
        }
        ui::info(&format!("Verifying {} names again…", stale.len()));
        let before: std::collections::HashMap<String, Status> =
            stale.iter().map(|r| (r.domain.clone(), r.status)).collect();
        let total = stale.len();
        let spinner = ui::Spinner::start();
        let mut done = 0;
        let mut on_event = |event: Event| {
            if let Event::Checked(record) = event {
                done += 1;
                spinner.status(&format!("{done}/{total} {}", record.domain));
                // Only changes are news.
                if before.get(&record.domain) != Some(&record.status) {
                    spinner.line(&ui::row(None, &record));
                }
            }
        };
        let round = self.hunter.recheck(stale, &mut on_event).await;
        spinner.stop();
        let round = round?;
        ui::info(&format!(
            "{} re-checked: {} available, {} taken, {} unclear.",
            round.checked,
            round.hits.len(),
            round.taken,
            round.unknown
        ));
        Ok(())
    }
}
