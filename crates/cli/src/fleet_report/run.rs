//! The composition: plan, fold, read seats, anonymise, analyze, write.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use pij_core::fleet::{Corpus, PriceTable, Seat, analyze};
use pij_core::model::{Envelope, ErrorKind};
use pij_unisphere::fleet::{FleetScope, read_fleet, under};
use serde_json::{Value, json};

use super::{
    Facts, FleetArgs, anonymise_corpus, parse_offset, parse_worktrees, plan, write_output,
};

const COMMAND: &str = "pij fleet-report";

fn refused(kind: ErrorKind, message: impl Into<String>) -> Envelope<Value> {
    Envelope::refused(COMMAND, kind, message)
}

/// The machine's UTC offset, from `date +%z`; UTC when it cannot be read.
fn local_offset_min() -> i32 {
    Command::new("date")
        .arg("+%z")
        .output()
        .ok()
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .and_then(|text| parse_offset(text.trim()))
        .unwrap_or(0)
}

fn git_worktrees(folder: &Path) -> Result<Vec<PathBuf>, String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(folder)
        .args(["worktree", "list", "--porcelain"])
        .output()
        .map_err(|error| format!("could not run git: {error}"))?;
    if !out.status.success() {
        return Err(format!(
            "git worktree list failed in {}: {}",
            folder.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(parse_worktrees(&String::from_utf8_lossy(&out.stdout)))
}

/// The seats a report is about: bound to an in-scope session, sending an
/// in-scope peer message, or registered under a scope folder while the window was open.
pub fn relevant_seats(
    seats: Vec<Seat>,
    corpus: &Corpus,
    folders: &[PathBuf],
    since_ms: i64,
    until_ms: i64,
) -> Vec<Seat> {
    let sessions: BTreeSet<&str> = corpus
        .sessions
        .iter()
        .filter_map(|s| s.session_id.as_deref())
        .collect();
    let senders: BTreeSet<&str> = corpus
        .turns
        .iter()
        .filter_map(|t| t.sender.as_deref())
        .collect();
    seats
        .into_iter()
        .filter(|seat| {
            seat.sessions.iter().any(|s| sessions.contains(s.as_str()))
                || senders.contains(seat.id.as_str())
                || (under(&seat.folder, folders)
                    && seat.spawned_ms.is_none_or(|at| at < until_ms)
                    && seat.ended_ms.is_none_or(|at| at >= since_ms))
        })
        .collect()
}

/// Run `pij fleet-report`. Never contacts the daemon, never writes the store.
pub async fn run(mut args: FleetArgs, state_dir: &Path) -> Envelope<Value> {
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX));
    if !args.folder.is_absolute() {
        match std::env::current_dir() {
            Ok(cwd) => args.folder = cwd.join(&args.folder),
            Err(error) => {
                return refused(ErrorKind::Refused, format!("E-RS-FLEET-FOLDER: {error}"));
            }
        }
    }
    args.folder = match args.folder.canonicalize() {
        Ok(path) => path,
        Err(error) => {
            return refused(
                ErrorKind::NotFound,
                format!("E-RS-FLEET-FOLDER: {}: {error}", args.folder.display()),
            );
        }
    };
    let plan = match plan(&args, now_ms, local_offset_min(), &git_worktrees, state_dir) {
        Ok(plan) => plan,
        Err(message) => return refused(ErrorKind::Refused, message),
    };
    if let Err(message) = super::check_out(&plan.out) {
        return refused(ErrorKind::Refused, message);
    }
    let scope = FleetScope {
        folders: plan.folders.clone(),
        since_ms: plan.window.since_ms,
        until_ms: plan.window.until_ms,
        harnesses: plan.harnesses.clone(),
        include_content: plan.include_content && !plan.anonymise,
        threads: plan.threads,
    };
    let read =
        tokio::task::spawn_blocking(move || read_fleet(pij_daemon::session_roots(), &scope)).await;
    let transcripts = match read {
        Ok(Ok(transcripts)) => transcripts,
        Ok(Err(error)) => return refused(ErrorKind::Adapter, error.to_string()),
        Err(error) => return refused(ErrorKind::Adapter, format!("the fold task failed: {error}")),
    };
    let mut facts = Facts {
        pij_version: env!("CARGO_PKG_VERSION").to_string(),
        generated_ms: now_ms,
        prep_table_schema: Some(transcripts.prep.table_schema_version),
        prep_policies: transcripts.prep.policies.clone(),
        prep_bytes_read: transcripts.prep.bytes_read,
        prep_sources: (
            transcripts.prep.sources_discovered,
            transcripts.prep.sources_folded,
        ),
        ..Facts::default()
    };
    for (source, error) in &transcripts.prep.unreadable {
        facts
            .warnings
            .push(format!("unreadable transcript {source}: {error}"));
    }
    let mut corpus = transcripts.corpus;
    let store_path = state_dir.join("pij.sqlite");
    match pij_store::fleet::read_seats(&store_path).await {
        Ok(store) => {
            facts.store_schema = Some(store.schema_version);
            match pij_store::fleet::read_messages(
                &store_path,
                plan.window.since_ms,
                plan.window.until_ms,
            )
            .await
            {
                Ok(messages) => corpus.messages = messages,
                Err(error) => facts.warnings.push(format!("no pij messages ({error})")),
            }
            corpus.primes = store.primes;
            corpus.prime_projects = store.prime_projects;
            let relevant = relevant_seats(
                store.seats.clone(),
                &corpus,
                &plan.folders,
                plan.window.since_ms,
                plan.window.until_ms,
            );
            // A seat that messaged a seat of this report is drawn as local, not remote.
            let ids: BTreeSet<&str> = relevant.iter().map(|s| s.id.as_str()).collect();
            let counterparts: BTreeSet<String> = corpus
                .messages
                .iter()
                .filter(|m| ids.contains(m.from.as_str()) || ids.contains(m.to.as_str()))
                .flat_map(|m| [m.from.clone(), m.to.clone()])
                .collect();
            let mut seats = relevant;
            let have: BTreeSet<String> = seats.iter().map(|s| s.id.clone()).collect();
            seats.extend(
                store
                    .seats
                    .into_iter()
                    .filter(|s| counterparts.contains(&s.id) && !have.contains(&s.id)),
            );
            corpus.seats = seats;
        }
        Err(error) => facts
            .warnings
            .push(format!("no pij seats: every session is unseated ({error})")),
    }
    if plan.anonymise {
        anonymise_corpus(&mut corpus);
    }
    let report = analyze(&corpus, plan.window, &PriceTable::default());
    let files = match write_output(&plan, &corpus, &report, &facts) {
        Ok(files) => files,
        Err(error) => {
            return refused(
                ErrorKind::Adapter,
                format!("E-RS-FLEET-WRITE: {}: {error}", plan.out.display()),
            );
        }
    };
    let k = &report.key_figures;
    Envelope::ok(
        COMMAND,
        json!({
            "out": plan.out,
            "page": plan.out.join("index.html"),
            "files": files,
            "window": { "since_ms": plan.window.since_ms, "until_ms": plan.window.until_ms },
            "folders": plan.folders.len(),
            "calls": report.totals.calls,
            "turns": report.totals.turns,
            "sessions": report.totals.sessions,
            "unseated_sessions": report.totals.unseated_sessions,
            "seats": report.totals.seats,
            "total_usd": k.total_usd,
            "reads_share": k.reads_share,
            "status_share": k.status_share,
            "avoidable_cold_wakes": k.avoidable_cold_wakes,
            "idle_cold_wakes": k.idle_cold_wakes,
            "warnings": facts.warnings,
        }),
    )
}
