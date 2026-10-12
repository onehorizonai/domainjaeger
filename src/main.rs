//! domainjaeger hunts for available domain names made of real words, like
//! cook.ing, mike.ai or (with a small typo) sn.app.
//!
//! This file is where the pieces are put together: the command line is
//! read, the doors to the outside world (`net`) are opened, and the
//! application (`app`) gets what it needs to do its work.

/// Printing takes the setup off the screen first, when it was left standing
/// there while a command runs: see `ui::Frame::park`.
macro_rules! println {
    ($($arg:tt)*) => {{
        $crate::ui::make_room();
        ::std::println!($($arg)*);
    }};
}

mod app;
mod cache;
mod check;
mod cli;
mod export;
mod hacks;
mod hunt;
mod lang;
mod net;
#[cfg(test)]
mod testing;
mod tld;
mod tone;
mod topics;
mod ui;
mod wiktionary;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Result, bail};
use clap::Parser;
use console::style;

use app::{App, CONTACT_KEY, WHOLE_NEEDS_TLDS, WHOLE_TLDS};
use app::{category_of, fixed_part, meaning_backlog, stats_of, to_excel};
use cache::Cache;
use check::Checker;
use cli::{Cli, Command};
use hacks::Affix;
use hunt::{Hunter, Settings};
use net::{Clock, Online, Port43, SystemClock, Web};
use tld::Catalog;
use wiktionary::Wiktionary;

#[tokio::main]
async fn main() {
    if let Err(e) = run(Cli::parse()).await {
        ui::error(&format!("{e:#}"));
        std::process::exit(1);
    }
}

/// One cache per user, so it does not matter where you run this from.
fn default_cache(home: &Path) -> PathBuf {
    home.join(".domainjaeger").join("cache.sqlite")
}

async fn run(cli: Cli) -> Result<()> {
    let home = std::env::home_dir().unwrap_or_default();
    let db = cli.db.clone().unwrap_or_else(|| default_cache(&home));
    // The three doors to the outside world: the clock, the web and WHOIS.
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    let cache = Arc::new(Cache::open(&db, clock)?);

    // This one never touches the network.
    if let Some(Command::Stats) = &cli.command {
        stats_of(&cache)?;
        let saved = format!("Saved in: {}", db.display());
        ui::info(&style(saved).dim().to_string());
        println!();
        return Ok(());
    }

    // A contact only ever goes to the dictionaries. Everybody else gets
    // the name of the tool and where it lives.
    let introduction = format!(
        "domainjaeger/{} (+{})",
        env!("CARGO_PKG_VERSION"),
        wiktionary::HOMEPAGE
    );
    let web: Arc<dyn Web> = Arc::new(Online::new(&introduction)?);

    let mut syllables = cli.syllables.clone();
    syllables.sort_unstable();
    syllables.dedup();
    let contact = match cli.contact.as_deref().map(str::trim) {
        Some(contact) if wiktionary::valid_contact(contact) => {
            cache.kv_put(CONTACT_KEY, contact)?;
            Some(contact.to_string())
        }
        Some(_) => bail!("--contact has to be an e-mail address or a URL"),
        None => cache
            .kv_get(CONTACT_KEY, i64::MAX)?
            .filter(|c| !c.is_empty()),
    };
    let wiki = Arc::new(Wiktionary::new(web.clone(), &syllables, contact));
    if let Some(Command::Export { path }) = &cli.command {
        meaning_backlog(&cache, &wiki).await;
        return to_excel(&cache, path.clone());
    }

    let catalog = Catalog::load(web.as_ref(), &cache).await?;
    for warning in &catalog.warnings {
        ui::warn(warning);
    }
    let category = match cli.category.as_deref().map(str::trim) {
        None | Some("" | "all" | "off" | "none") => None,
        Some(wanted) => Some(category_of(&cache, &wiki, cli.lang, wanted).await?),
    };
    let checker = Arc::new(Checker::new(
        web,
        Arc::new(Port43),
        cache.clone(),
        catalog.bootstrap.clone(),
    ));
    let affix = Affix {
        prefix: fixed_part(cli.prefix.as_deref().unwrap_or_default())?,
        suffix: fixed_part(cli.suffix.as_deref().unwrap_or_default())?,
    };
    // A word with a fixed part is a whole name.
    let whole = cli.whole || !affix.is_empty();
    let settings = Settings {
        count: cli.count.max(1),
        max_checks: cli.max_checks.max(1),
        min_label: cli.min_label.max(1),
        typos: !cli.no_typos,
        category,
        lang: cli.lang,
        whole,
        rude: !cli.no_rude,
        affix,
    };
    let (tlds, focused) = match cli.tlds.as_deref() {
        Some("all") if whole => bail!("{WHOLE_NEEDS_TLDS}"),
        Some(wanted) if wanted != "all" => (catalog.select(wanted)?, true),
        None if whole => (catalog.select(WHOLE_TLDS)?, true),
        _ => (catalog.defaults(), false),
    };
    let hunter = Hunter::new(
        wiki.clone(),
        checker.clone(),
        cache.clone(),
        tlds,
        focused,
        settings,
    );
    let mut app = App::new(cache, wiki, checker, catalog, hunter, focused, cli.verbose);
    app.run(cli.command).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cache_lives_in_a_folder_of_its_own_under_home() {
        let path = default_cache(Path::new("/Users/someone"));
        assert_eq!(
            path,
            PathBuf::from("/Users/someone/.domainjaeger/cache.sqlite")
        );
    }
}
