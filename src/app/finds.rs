//! What was found: the list, the Excel file and the numbers.

use super::*;

impl App {
    /// The available names so far: all of them, those of a category
    /// (`l finance`), of a language (`l en`) or of a TLD (`l .nl`).
    pub(super) fn list(&self, category: &str) -> Result<()> {
        let lang = Lang::parse(category);
        let tld = category.strip_prefix('.');
        let now = self.cache.now();
        let finds: Vec<Record> = self
            .cache
            .all()?
            .into_iter()
            .filter(|r| r.status == Status::Available && r.is_fresh(now))
            .filter(|r| match (tld, lang) {
                (Some(tld), _) => r.tld.eq_ignore_ascii_case(tld),
                (None, Some(lang)) => Lang::from_code(&r.lang) == lang,
                (None, None) => category.is_empty() || topics::matches(r, category),
            })
            .collect();
        if let Some(tld) = tld.filter(|_| finds.is_empty()) {
            ui::info(&format!("No available .{tld} names yet."));
            return Ok(());
        }
        if let Some(lang) = lang.filter(|_| finds.is_empty()) {
            ui::info(&format!(
                "No available {} names yet. `lang {}` hunts for them.",
                lang.name(),
                lang.code()
            ));
            return Ok(());
        }
        if finds.is_empty() && !category.is_empty() {
            ui::info(&format!(
                "No available {category} names yet. `cat {category}` hunts for them."
            ));
            return Ok(());
        }
        if finds.is_empty() {
            ui::info("No available names yet, press enter to go hunting.");
            return Ok(());
        }
        const SHOWN: usize = 40;
        println!("{}", ui::header());
        for (i, record) in finds.iter().take(SHOWN).enumerate() {
            println!("{}", ui::row(Some(i + 1), record));
        }
        if finds.len() > SHOWN {
            ui::info(
                &style(format!(
                    "… and {} more, `export` has them all",
                    finds.len() - SHOWN
                ))
                .dim()
                .to_string(),
            );
        }
        println!();
        Ok(())
    }
}

/// Available names without a meaning get one before they are exported:
/// the ones that were found while the dictionary had no request to spare.
/// It takes a request per word, so ctrl-c goes on without the rest. Nice to
/// have, so a failure is only mentioned.
pub(crate) async fn meaning_backlog(cache: &Cache, wiki: &Wiktionary) {
    let identified = wiki.contact().is_some();
    let ask = async {
        let mut words = cache.unexplained()?;
        let waiting = words.len();
        // Anonymous clients get few dictionary requests: no more than fit
        // in this minute, so nobody waits an hour for an Excel file.
        if !identified {
            words.truncate(wiki.spare_requests());
        }
        if words.is_empty() {
            return Ok((0, waiting));
        }
        let spinner = ui::Spinner::start();
        let mut ctrl_c = Box::pin(tokio::signal::ctrl_c());
        let mut asked = 0;
        let mut failure = None;
        for (code, word) in &words {
            spinner.status(&format!(
                "asking Wiktionary what the words mean · {asked}/{} · ctrl-c skips the rest",
                words.len()
            ));
            tokio::select! {
                _ = &mut ctrl_c => break,
                meaning = wiki.meaning(Lang::from_code(code), word) => match meaning {
                    // Nothing to say is an answer too: it is not asked again.
                    Ok(meaning) => cache.explain(code, word, &meaning.unwrap_or_default())?,
                    Err(e) => {
                        failure = Some(e);
                        break;
                    }
                },
            }
            asked += 1;
        }
        spinner.stop();
        match failure {
            Some(e) if asked == 0 => Err(e),
            _ => anyhow::Ok((asked, waiting - asked)),
        }
    };
    match ask.await {
        Ok((0, 0)) => {}
        Ok((asked, left)) => {
            let mut news = Vec::new();
            if asked > 0 {
                news.push(format!(
                    "Looked up what {asked} words of earlier finds mean."
                ));
            }
            let waiting = ui::thousands(left as u64);
            match (left, identified) {
                (0, _) => {}
                (_, true) => news.push(format!(
                    "{waiting} words have no meaning yet, the next export goes on with them."
                )),
                (_, false) => news.push(format!(
                    "{waiting} words have no meaning yet: `contact` looks them up 20x faster."
                )),
            }
            for line in news {
                ui::info(&style(line).dim().to_string());
            }
        }
        Err(e) => ui::warn(&format!("could not look up what the words mean ({e:#})")),
    }
}

/// Everything that was checked, as an Excel file.
pub(crate) fn to_excel(cache: &Cache, path: Option<PathBuf>) -> Result<()> {
    let records = cache.all()?;
    if records.is_empty() {
        ui::info("Nothing to export yet, go hunting first.");
        return Ok(());
    }
    let now = cache.now();
    let path = path.unwrap_or_else(|| PathBuf::from(export_name(now)));
    let path = if path.extension().is_some() {
        path
    } else {
        path.with_extension("xlsx")
    };
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    }
    let summary = export::write(&path, &records, now)?;
    ui::info(&format!(
        "{} {} written: {} checked domains, {} of them available.",
        style("▤").green(),
        style(path.display()).bold(),
        summary.total,
        style(summary.available).green().bold()
    ));
    println!();
    Ok(())
}

/// What an export is called when nobody names it: after the day it is made.
pub(crate) fn export_name(now: i64) -> String {
    format!("domainjaeger-{}.xlsx", ui::local_day(now))
}

pub(crate) fn stats_of(cache: &Cache) -> Result<()> {
    let records = cache.all()?;
    ui::stats(
        &cache.stats()?,
        &hunt::by_tld(&records),
        &topics::tally(&records),
    );
    if let Some(latest) = records.iter().map(|r| r.checked_at).max() {
        ui::info(
            &style(format!("Last check: {}", ui::local_time(latest)))
                .dim()
                .to_string(),
        );
        println!();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::World;

    #[test]
    fn an_export_is_named_after_its_day_unless_it_has_a_name() {
        let name = export_name(1_700_000_000);
        assert!(
            name.starts_with("domainjaeger-2023-11-1") && name.ends_with(".xlsx"),
            "{name}"
        );

        let world = World::new();
        let dir = std::env::temp_dir().join(format!("domainjaeger-finds-{}", std::process::id()));
        // Nothing found yet: nothing is written.
        to_excel(&world.cache, Some(dir.join("empty"))).unwrap();
        assert!(!dir.exists());

        world
            .cache
            .put(&Record::sample("cook.ing", world.cache.now()))
            .unwrap();
        // The folder is made, and a name without an ending gets .xlsx.
        to_excel(&world.cache, Some(dir.join("deep").join("names"))).unwrap();
        assert!(dir.join("deep").join("names.xlsx").is_file());
        to_excel(&world.cache, Some(dir.join("names.xls"))).unwrap();
        assert!(dir.join("names.xls").is_file());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test(start_paused = true)]
    async fn meanings_that_were_missed_are_asked_before_an_export() {
        let world = World::new();
        let sense = r#"{"en":[{"definitions":[{"definition":"The art of preparing food"}]}]}"#;
        world.web.on("definition/cooking", 200, sense);
        let cooking = Record {
            word: "cooking".into(),
            ..Record::sample("cook.ing", world.cache.now())
        };
        let unknown = Record {
            word: "qzxing".into(),
            ..Record::sample("qzx.ing", world.cache.now())
        };
        world.cache.put(&cooking).unwrap();
        world.cache.put(&unknown).unwrap();
        let wiki = world.wiki(&[1, 2, 3]);

        meaning_backlog(&world.cache, &wiki).await;
        let meaning = |domain| world.cache.get(domain).unwrap().unwrap().meaning;
        assert_eq!(
            meaning("cook.ing").as_deref(),
            Some("The art of preparing food")
        );
        // A word the dictionary has nothing on is not asked about again.
        assert_eq!(meaning("qzx.ing").as_deref(), Some(""));
        let asked = world.web.asked().len();
        meaning_backlog(&world.cache, &wiki).await;
        assert_eq!(world.web.asked().len(), asked);
        // And the numbers can be shown, with or without finds.
        stats_of(&world.cache).unwrap();
        stats_of(&World::new().cache).unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn the_list_can_be_narrowed_down() {
        let world = World::new();
        world
            .cache
            .put(&Record::sample("cook.ing", world.cache.now()))
            .unwrap();
        let catalog = Catalog::of(&["ing"]);
        let hunter = world.hunter(&["ing"], crate::testing::settings());
        let (wiki, checker) = (world.wiki(&[1, 2, 3]), world.checker());
        let app = App::new(
            world.cache.clone(),
            wiki,
            checker,
            catalog,
            hunter,
            true,
            false,
        );
        // By extension, language or category: none of them fails on an empty answer.
        for filter in ["", ".ing", ".nl", "en", "nl", "finance", "music"] {
            app.list(filter).unwrap();
        }
    }
}
