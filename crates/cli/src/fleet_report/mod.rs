//! `pij fleet-report` (plan 162): one command writes a project's Context Tax
//! report — `report.json`, a static page that renders it, and the raw tables —
//! into one folder.
//!
//! Unisphere folds the transcripts in-process (`pij_unisphere::fleet`), pij's
//! store is read read-only (`pij_store::fleet`), and `pij_core::fleet` computes
//! every aggregate. This module plans the run, anonymises, and writes the folder.
//! It never talks to the daemon and never writes the store.

mod anonymise;
mod run;
mod write;

#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};

use pij_core::fleet::Window;
use serde::Serialize;

pub use anonymise::anonymise_corpus;
pub use run::{relevant_seats, run};
pub use write::{csv_cell, write_output};

/// The command's arguments, as typed.
#[derive(Clone, Debug)]
pub struct FleetArgs {
    /// The project root.
    pub folder: PathBuf,
    /// Add every `git worktree list` path of the folder.
    pub with_worktrees: bool,
    /// Window start: RFC 3339, a local `YYYY-MM-DD`, or `<N>d`/`<N>h` ago.
    pub since: Option<String>,
    /// Window end, same forms; default now.
    pub until: Option<String>,
    /// Comma-separated harnesses (`claude-code,omp,…`); default every one.
    pub harness: Option<String>,
    /// Output folder; default under `~/.pij-rs/fleet-reports/`.
    pub out: Option<PathBuf>,
    /// Table format: `jsonl` or `csv` (`parquet` is refused in P1).
    pub format: String,
    /// Keep turn-opener heads (local use only).
    pub include_content: bool,
    /// Replace names with roles and letters; drop paths and content.
    pub anonymise: bool,
    /// Reuse a Unisphere prep folder (refused in P1).
    pub prep_target: Option<PathBuf>,
    /// The report clock, `+HH:MM`; default the machine's.
    pub utc_offset: Option<String>,
    /// Fold threads.
    pub threads: usize,
}

/// A table format.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    /// One JSON object per line.
    Jsonl,
    /// RFC 4180 CSV with a header row.
    Csv,
}

/// A resolved run.
#[derive(Clone, Debug, PartialEq)]
pub struct Plan {
    /// The project root.
    pub folder: PathBuf,
    /// Every folder whose sessions are in scope (the root first).
    pub folders: Vec<PathBuf>,
    /// The report window and clock.
    pub window: Window,
    /// Harness filter; empty is every harness.
    pub harnesses: Vec<String>,
    /// Table format.
    pub format: Format,
    /// Output folder.
    pub out: PathBuf,
    /// Keep opener heads.
    pub include_content: bool,
    /// Anonymise names, paths and content.
    pub anonymise: bool,
    /// Fold threads.
    pub threads: usize,
}

/// What the run learned beyond the corpus, for the manifest.
#[derive(Clone, Debug, Default, Serialize)]
pub struct Facts {
    /// pij-rs version.
    pub pij_version: String,
    /// When the report was generated, UTC ms.
    pub generated_ms: i64,
    /// The store's schema, when it was read.
    pub store_schema: Option<u32>,
    /// Unisphere's prep table schema version.
    pub prep_table_schema: Option<u32>,
    /// Fold policy per harness.
    pub prep_policies: std::collections::BTreeMap<String, String>,
    /// Native bytes read.
    pub prep_bytes_read: u64,
    /// Sources discovered and folded.
    pub prep_sources: (u64, u64),
    /// Things the reader should know (an unreadable source, a missing store).
    pub warnings: Vec<String>,
}

/// Every `worktree <path>` of `git worktree list --porcelain`.
pub fn parse_worktrees(porcelain: &str) -> Vec<PathBuf> {
    porcelain
        .lines()
        .filter_map(|line| line.strip_prefix("worktree "))
        .map(PathBuf::from)
        .collect()
}

/// Days since 1970-01-01 of a proleptic Gregorian date (Hinnant's `days_from_civil`).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let yoe = year.rem_euclid(400);
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// `+HH:MM`, `-HH:MM` or `+HHMM`, in minutes east of UTC.
pub fn parse_offset(text: &str) -> Option<i32> {
    let (sign, rest) = match text.as_bytes().first()? {
        b'+' => (1, &text[1..]),
        b'-' => (-1, &text[1..]),
        _ => return None,
    };
    let digits: String = rest.chars().filter(|c| *c != ':').collect();
    if digits.len() != 4 || !digits.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let hours: i32 = digits[..2].parse().ok()?;
    let minutes: i32 = digits[2..].parse().ok()?;
    (hours <= 14 && minutes < 60).then_some(sign * (hours * 60 + minutes))
}

const TIME_FORMS: &str =
    "use RFC 3339 (2026-09-24T10:00:00+10:00), a local date (2026-09-24), or a span ago (7d, 36h)";

/// An instant: RFC 3339, a date at local midnight, or `<N>d` / `<N>h` before `now_ms`.
///
/// # Errors
/// A message naming the accepted forms.
pub fn parse_time(text: &str, now_ms: i64, offset_min: i32) -> Result<i64, String> {
    let text = text.trim();
    let bad = || format!("`{text}` is not a time: {TIME_FORMS}");
    for (suffix, unit) in [("d", 86_400_000i64), ("h", 3_600_000)] {
        if let Some(count) = text.strip_suffix(suffix)
            && let Ok(count) = count.parse::<i64>()
        {
            return Ok(now_ms - count * unit);
        }
    }
    if text.len() == 10 {
        let parts: Vec<&str> = text.split('-').collect();
        if let [year, month, day] = parts.as_slice()
            && let (Ok(year), Ok(month), Ok(day)) = (
                year.parse::<i64>(),
                month.parse::<i64>(),
                day.parse::<i64>(),
            )
            && (1..=12).contains(&month)
            && (1..=31).contains(&day)
        {
            let days = days_from_civil(year, month, day);
            return Ok(days * 86_400_000 - i64::from(offset_min) * 60_000);
        }
        return Err(bad());
    }
    let parsed = time::OffsetDateTime::parse(text, &time::format_description::well_known::Rfc3339)
        .map_err(|_| bad())?;
    i64::try_from(parsed.unix_timestamp_nanos() / 1_000_000).map_err(|_| bad())
}

/// `YYYYMMDDTHHMMSSZ` of `ts_ms`.
fn stamp(ts_ms: i64) -> String {
    let at = time::OffsetDateTime::from_unix_timestamp(ts_ms.div_euclid(1_000))
        .unwrap_or(time::OffsetDateTime::UNIX_EPOCH);
    format!(
        "{:04}{:02}{:02}T{:02}{:02}{:02}Z",
        at.year(),
        u8::from(at.month()),
        at.day(),
        at.hour(),
        at.minute(),
        at.second()
    )
}

/// Resolve the arguments into a run.
///
/// # Errors
/// An `E-RS-FLEET-*` message for anything P1 cannot honour.
pub fn plan(
    args: &FleetArgs,
    now_ms: i64,
    offset_min: i32,
    worktrees: &dyn Fn(&Path) -> Result<Vec<PathBuf>, String>,
    state_dir: &Path,
) -> Result<Plan, String> {
    if !args.folder.is_absolute() {
        return Err(format!(
            "E-RS-FLEET-FOLDER: `{}` must be an absolute path",
            args.folder.display()
        ));
    }
    let format = match args.format.as_str() {
        "jsonl" => Format::Jsonl,
        "csv" => Format::Csv,
        "parquet" => {
            return Err(
                "E-RS-FLEET-FORMAT: parquet needs a Parquet writer pij does not ship yet; \
                        use --format jsonl or --format csv"
                    .to_string(),
            );
        }
        other => {
            return Err(format!(
                "E-RS-FLEET-FORMAT: unknown format `{other}`; use jsonl or csv"
            ));
        }
    };
    if let Some(target) = &args.prep_target {
        return Err(format!(
            "E-RS-FLEET-PREP-TARGET: reusing a Unisphere prep folder ({}) arrives with Unisphere's \
             report slice; fleet-report folds the transcripts in-process today",
            target.display()
        ));
    }
    let offset_min = match &args.utc_offset {
        Some(text) => parse_offset(text).ok_or_else(|| {
            format!("E-RS-FLEET-WINDOW: `{text}` is not a UTC offset like +10:00")
        })?,
        None => offset_min,
    };
    let at = |value: &Option<String>, default: i64| match value {
        Some(text) => {
            parse_time(text, now_ms, offset_min).map_err(|e| format!("E-RS-FLEET-WINDOW: {e}"))
        }
        None => Ok(default),
    };
    let until_ms = at(&args.until, now_ms)?;
    let since_ms = at(&args.since, now_ms - 7 * 86_400_000)?;
    if since_ms >= until_ms {
        return Err("E-RS-FLEET-WINDOW: --since must come before --until".to_string());
    }
    let mut harnesses = Vec::new();
    for name in args.harness.iter().flat_map(|list| list.split(',')) {
        let name = name.trim();
        if name.is_empty() {
            continue;
        }
        let id = pij_unisphere::fleet::unisphere_harness_id(name).ok_or_else(|| {
            format!("E-RS-FLEET-HARNESS: `{name}` is not a harness fleet-report can read (claude-code, omp, codex, copilot)")
        })?;
        harnesses.push(id.to_string());
    }
    let mut folders = vec![args.folder.clone()];
    if args.with_worktrees {
        for path in worktrees(&args.folder).map_err(|e| format!("E-RS-FLEET-WORKTREES: {e}"))? {
            if !folders.contains(&path) {
                folders.push(path);
            }
        }
    }
    let out = match &args.out {
        Some(out) => out.clone(),
        None => {
            let name = if args.anonymise {
                "project".to_string()
            } else {
                args.folder
                    .file_name()
                    .map_or_else(|| "root".to_string(), |n| n.to_string_lossy().into_owned())
            };
            state_dir
                .join("fleet-reports")
                .join(format!("{name}-{}", stamp(now_ms)))
        }
    };
    Ok(Plan {
        folder: args.folder.clone(),
        folders,
        window: Window {
            since_ms,
            until_ms,
            utc_offset_min: offset_min,
        },
        harnesses,
        format,
        out,
        include_content: args.include_content,
        anonymise: args.anonymise,
        threads: args.threads.max(1),
    })
}
