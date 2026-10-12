//! Excel export of everything that was checked.

use std::path::Path;

use anyhow::{Context, Result};
use chrono::{Datelike, Local, TimeZone, Timelike};
use rust_xlsxwriter::{Color, ExcelDateTime, Format, FormatAlign, Workbook, Worksheet};

use crate::cache::Record;
use crate::check::Status;
use crate::hacks::Affix;
use crate::lang::Lang;
use crate::topics;

const COLUMNS: &[(&str, f64)] = &[
    ("Domain", 24.0),
    ("Status", 12.0),
    ("Word", 20.0),
    ("Meaning", 44.0),
    ("Tone", 18.0),
    ("Language", 10.0),
    ("Category", 18.0),
    ("Word parts", 11.0),
    ("Ending", 8.0),
    ("Name style", 19.0),
    ("Spelling", 10.0),
    ("Reads as", 20.0),
    ("Typo", 18.0),
    ("Known by % (NL)", 16.0),
    ("Official word list", 17.0),
    ("Dictionary topics", 30.0),
    ("Verified via", 13.0),
    ("Checked", 18.0),
    ("Valid until", 18.0),
    ("Needs re-check", 15.0),
];

pub struct Summary {
    pub total: usize,
    pub available: usize,
}

/// Writes two sheets: the available names that are still within their four
/// weeks, and the complete list of checked domains.
pub fn write(path: &Path, records: &[Record], now: i64) -> Result<Summary> {
    let available: Vec<&Record> = records
        .iter()
        .filter(|r| r.status == Status::Available && r.is_fresh(now))
        .collect();
    let all: Vec<&Record> = records.iter().collect();

    let mut workbook = Workbook::new();
    sheet(workbook.add_worksheet(), "Available", &available, now)?;
    sheet(workbook.add_worksheet(), "All checked", &all, now)?;
    workbook
        .save(path)
        .with_context(|| format!("cannot write {}", path.display()))?;
    Ok(Summary {
        total: all.len(),
        available: available.len(),
    })
}

fn sheet(sheet: &mut Worksheet, name: &str, records: &[&Record], now: i64) -> Result<()> {
    let header = Format::new()
        .set_bold()
        .set_font_color(Color::White)
        .set_background_color(Color::RGB(0xF2_6B_1D))
        .set_align(FormatAlign::Left);
    let date = Format::new()
        .set_num_format("yyyy-mm-dd hh:mm")
        .set_align(FormatAlign::Left);
    let bold = Format::new().set_bold();
    let fill = |background: u32, font: u32| {
        Format::new()
            .set_background_color(Color::RGB(background))
            .set_font_color(Color::RGB(font))
    };
    let green = fill(0xC6_EF_CE, 0x00_61_00);
    let red = fill(0xFF_C7_CE, 0x9C_00_06);
    let yellow = fill(0xFF_EB_9C, 0x9C_57_00);

    sheet.set_name(name)?;
    for (col, (title, width)) in COLUMNS.iter().enumerate() {
        sheet.write_string_with_format(0, col as u16, *title, &header)?;
        sheet.set_column_width(col as u16, *width)?;
    }
    for (i, r) in records.iter().enumerate() {
        let row = i as u32 + 1;
        let status_format = match r.status {
            Status::Available => &green,
            Status::Taken => &red,
            Status::Unknown => &yellow,
        };
        sheet.write_string_with_format(row, 0, &r.domain, &bold)?;
        sheet.write_string_with_format(row, 1, r.status.as_str(), status_format)?;
        sheet.write_string(row, 2, &r.word)?;
        // In English, whatever the language of the word.
        sheet.write_string(row, 3, r.meaning.as_deref().unwrap_or_default())?;
        // What the dictionary flags the word as, e.g. "vulgar, sexual".
        if let Some(tone) = r.tone.as_deref().filter(|tone| !tone.is_empty()) {
            sheet.write_string_with_format(row, 4, tone, &yellow)?;
        }
        let lang = Lang::from_code(&r.lang);
        sheet.write_string(row, 5, lang.name())?;
        let word_topics = r.topics.as_deref().unwrap_or_default();
        sheet.write_string(row, 6, topics::labels(word_topics).join(", "))?;
        if let Some(syllables) = r.syllables {
            sheet.write_number(row, 7, syllables)?;
        }
        sheet.write_string(row, 8, format!(".{}", r.tld))?;
        // leni.ng is a hack, lening.nl a whole word, speedbox.nl a word
        // with a fixed part.
        let name = match (r.whole, Affix::of(&r.label, &r.word)) {
            (true, Some(_)) if r.kind == "exact" => "with an extra word",
            (true, _) => "whole word",
            (false, _) => "dot inside the word",
        };
        sheet.write_string(row, 9, name)?;
        let is_typo = r.kind != "exact";
        sheet.write_string(row, 10, if is_typo { "typo" } else { "exact" })?;
        sheet.write_string(row, 11, &r.spelled)?;
        if is_typo {
            sheet.write_string(row, 12, &r.kind)?;
        }
        if let Some(prevalence) = r.prevalence {
            sheet.write_number(row, 13, prevalence)?;
        }
        // Only Dutch has an official word list.
        if lang == Lang::Dutch {
            sheet.write_string(row, 14, if r.official { "yes" } else { "no" })?;
        }
        sheet.write_string(row, 15, word_topics)?;
        sheet.write_string(row, 16, &r.source)?;
        sheet.write_datetime_with_format(row, 17, &excel_time(r.checked_at)?, &date)?;
        sheet.write_datetime_with_format(row, 18, &excel_time(r.valid_until())?, &date)?;
        if r.is_fresh(now) {
            sheet.write_string(row, 19, "no")?;
        } else {
            sheet.write_string_with_format(row, 19, "yes", &yellow)?;
        }
    }
    sheet.set_freeze_panes(1, 1)?;
    sheet.autofilter(0, 0, records.len() as u32, COLUMNS.len() as u16 - 1)?;
    Ok(())
}

/// Timestamps are shown in the local time zone of whoever runs the export.
fn excel_time(timestamp: i64) -> Result<ExcelDateTime> {
    let t = Local
        .timestamp_opt(timestamp, 0)
        .single()
        .context("timestamp out of range")?;
    Ok(
        ExcelDateTime::from_ymd(t.year() as u16, t.month() as u8, t.day() as u8)?.and_hms(
            t.hour() as u16,
            t.minute() as u8,
            t.second() as f64,
        )?,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_excel_file_has_what_was_found_and_what_was_checked() {
        let now = 1_700_000_000;
        let fresh = Record::sample("cook.ing", now);
        let stale = Record::sample("bak.ing", now - crate::cache::DOMAIN_TTL_SECS);
        let taken = Record {
            status: Status::Taken,
            ..Record::sample("mak.ing", now)
        };
        let typo = Record {
            kind: "doubled letter".into(),
            spelled: "snapp".into(),
            word: "snap".into(),
            meaning: Some("A quick breaking sound".into()),
            tone: Some("vulgar".into()),
            whole: false,
            ..Record::sample("sn.app", now)
        };
        let dir = std::env::temp_dir().join(format!("domainjaeger-export-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("names.xlsx");
        let summary = write(&path, &[fresh, stale, taken, typo], now).unwrap();
        // Everything is listed; "available" is what can still be trusted.
        assert_eq!((summary.total, summary.available), (4, 2));
        // An Excel file is a zip archive, and this one is not empty.
        let bytes = std::fs::read(&path).unwrap();
        assert!(
            bytes.starts_with(b"PK") && bytes.len() > 4_000,
            "{} bytes",
            bytes.len()
        );
        // Nothing to write is still a valid file with its headers.
        let empty = write(&dir.join("empty.xlsx"), &[], now).unwrap();
        assert_eq!((empty.total, empty.available), (0, 0));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn excel_gets_times_it_understands() {
        assert!(excel_time(1_700_000_000).is_ok());
        assert!(excel_time(0).is_ok());
    }
}
