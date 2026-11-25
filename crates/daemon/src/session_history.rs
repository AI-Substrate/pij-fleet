//! A seat's past harness sessions, read back from the event log (PRD 008
//! req-13, plan 250; blueprint 005 c-session-history, decisions d-10, d-11).
//!
//! Every registry write publishes the whole seat descriptor as a `seat.put`
//! event, and the event log is never pruned, so a seat's sessions are a fold
//! over its `seat.put` facts in order: a session starts at the first fact
//! with its session id and ends at the first with another, or at the seat's
//! tombstone. Why each one changed comes from the events beside it: the spawn
//! (`spawn.launched`), a revive (`seat.revive-postmortem`, `seat.resumed`), a
//! compaction (`watchdog.compaction-wake`), or a new session on the same
//! process with none of those (a rebind).
//!
//! A past session's context size is read from at most [`TAIL_CAP`] bytes at
//! the end of its transcript, never the whole file, and is unknown when the
//! file is gone or the tail holds no usage figure.
//!
//! Cost: one indexed event-log read per seat (seat, kind) and at most
//! [`TAIL_CAP`] bytes read per session listed. In-process; nothing launches;
//! nothing runs on a timer.

use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use pij_core::model::Event;
use serde_json::{Value, json};

/// The event kinds the fold reads, in one indexed query per seat.
pub const KINDS: [&str; 6] = [
    "seat.put",
    "seat.tombstone",
    "spawn.launched",
    "seat.revive-postmortem",
    "seat.resumed",
    "watchdog.compaction-wake",
];

/// The most of a transcript read for its last usage figure (decision d-11).
pub const TAIL_CAP: u64 = 256 * 1024;

/// Sessions listed per seat unless the caller asks for another number.
pub const DEFAULT_LIMIT: usize = 20;

/// One harness session a seat used.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Session {
    /// The harness's session id.
    pub session: Option<String>,
    /// The harness.
    pub harness: String,
    /// The last model recorded in it.
    pub model: Option<String>,
    /// The folder the seat ran in.
    pub folder: String,
    /// The process that ran it, as `pid` (the last recorded).
    pub pid: Option<u64>,
    /// When it started (ms since the epoch).
    pub started_at: u64,
    /// When it ended, or `None` while it runs.
    pub ended_at: Option<u64>,
    /// Why it started: `spawn`, `revive`, `compact`, `rebind`, or `relaunch`
    /// (a new process with no spawn or revive beside it).
    pub why: String,
    /// What happened inside it that kept its id (a compaction).
    pub inside: Vec<String>,
}

fn text(value: &Value) -> Option<String> {
    value
        .as_str()
        .filter(|text| !text.is_empty())
        .map(str::to_string)
}

/// Fold a seat's events (oldest first) into its sessions, oldest first.
pub fn fold(events: &[Event]) -> Vec<Session> {
    let mut sessions: Vec<Session> = Vec::new();
    let mut pending: Option<&'static str> = None;
    let mut open = false;
    for event in events {
        let payload: Value = serde_json::from_str(&event.payload).unwrap_or(Value::Null);
        match event.kind.as_str() {
            "spawn.launched" => pending = Some("spawn"),
            "seat.revive-postmortem" | "seat.resumed" => pending = Some("revive"),
            "watchdog.compaction-wake" => {
                if open && let Some(current) = sessions.last_mut() {
                    current.inside.push(format!("compacted at {}", event.at));
                }
                pending = Some("compact");
            }
            "seat.tombstone" => {
                if open && let Some(current) = sessions.last_mut() {
                    current.ended_at = Some(event.at);
                }
                open = false;
            }
            "seat.put" => {
                let session = text(&payload["session"]);
                let pid = payload["proc"]["pid"].as_u64();
                let current = sessions.last().filter(|_| open);
                let same = current.is_some_and(|current| current.session == session);
                if same {
                    let current = sessions.last_mut().expect("an open session");
                    if let Some(model) = text(&payload["model"]) {
                        current.model = Some(model);
                    }
                    if pid.is_some() {
                        current.pid = pid;
                    }
                    if payload["tombstoned_at"].is_number() {
                        current.ended_at = payload["tombstoned_at"].as_u64();
                        open = false;
                    }
                    continue;
                }
                if session.is_none() {
                    // A write before a session binds: none starts yet.
                    continue;
                }
                let previous_pid = sessions.last().and_then(|last| last.pid);
                if open && let Some(current) = sessions.last_mut() {
                    current.ended_at = Some(event.at);
                }
                let why = match pending.take() {
                    Some(why) => why.to_string(),
                    None if sessions.is_empty() => "spawn".to_string(),
                    None if pid.is_some() && pid == previous_pid => "rebind".to_string(),
                    None => "relaunch".to_string(),
                };
                sessions.push(Session {
                    session,
                    harness: text(&payload["harness"]).unwrap_or_default(),
                    model: text(&payload["model"]),
                    folder: text(&payload["folder"]).unwrap_or_default(),
                    pid,
                    started_at: event.at,
                    ended_at: payload["tombstoned_at"].as_u64(),
                    why,
                    inside: Vec::new(),
                });
                open = sessions.last().is_some_and(|last| last.ended_at.is_none());
            }
            _ => {}
        }
    }
    sessions
}

/// Where each harness keeps a session's transcript.
#[derive(Clone, Debug, Default)]
pub struct Roots {
    /// Claude homes; each `projects/<folder>/<session>.jsonl`.
    pub claude_homes: Vec<PathBuf>,
    /// OMP's sessions dir; `--<folder, '/' as '-'>--/<stamp>_<session>.jsonl`.
    pub omp_sessions: Option<PathBuf>,
    /// Copilot's session-state dir; `<session>/events.jsonl`.
    pub copilot_sessions: Option<PathBuf>,
}

/// The transcript for `session`, when the harness's layout names it from the
/// harness, session id and folder: one stat (Claude, Copilot) or one listing
/// of one folder (OMP). Codex dates its folders, so it is not derived.
pub fn transcript(roots: &Roots, harness: &str, session: &str, folder: &str) -> Option<PathBuf> {
    if session.is_empty() || session.contains(['/', '\\']) || session.contains("..") {
        return None;
    }
    match harness {
        "copilot" => roots
            .copilot_sessions
            .as_ref()
            .map(|root| root.join(session).join("events.jsonl"))
            .filter(|path| path.is_file()),
        "claude" => {
            let project: String = folder
                .chars()
                .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
                .collect();
            roots
                .claude_homes
                .iter()
                .map(|home| {
                    home.join("projects")
                        .join(&project)
                        .join(format!("{session}.jsonl"))
                })
                .find(|path| path.is_file())
        }
        "omp" => {
            let dir = roots
                .omp_sessions
                .as_ref()?
                .join(format!("-{}--", folder.replace('/', "-")));
            let suffix = format!("_{session}.jsonl");
            std::fs::read_dir(dir)
                .ok()?
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .find(|path| {
                    path.file_name()
                        .and_then(|name| name.to_str())
                        .is_some_and(|name| name.ends_with(&suffix))
                })
        }
        _ => None,
    }
}

/// The context in use at the last model call a transcript's tail records:
/// Claude's and OMP's `usage` (fresh input plus cache read and cache write),
/// Copilot's `session.usage_record` (every token class but output).
pub fn last_usage(line: &Value) -> Option<u64> {
    if line["type"] == "session.usage_record" {
        let details = line["data"]["usage"]["copilotUsage"]["tokenDetails"].as_array()?;
        return Some(
            details
                .iter()
                .filter(|detail| detail["tokenType"] != "output")
                .filter_map(|detail| detail["tokenCount"].as_u64())
                .sum(),
        );
    }
    let usage = &line["message"]["usage"];
    if let Some(input) = usage["input_tokens"].as_u64() {
        return Some(
            input
                + usage["cache_creation_input_tokens"].as_u64().unwrap_or(0)
                + usage["cache_read_input_tokens"].as_u64().unwrap_or(0),
        );
    }
    if let Some(input) = usage["input"].as_u64() {
        return Some(
            input
                + usage["cacheRead"].as_u64().unwrap_or(0)
                + usage["cacheWrite"].as_u64().unwrap_or(0),
        );
    }
    None
}

/// The last usage figure in at most [`TAIL_CAP`] bytes at the end of `path`:
/// `None` when the file is gone or the tail holds none.
pub fn context_at_end(path: &Path) -> Option<u64> {
    let mut file = std::fs::File::open(path).ok()?;
    let length = file.metadata().ok()?.len();
    let start = length.saturating_sub(TAIL_CAP);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut tail = Vec::with_capacity(usize::try_from(length - start).unwrap_or(0));
    file.take(TAIL_CAP).read_to_end(&mut tail).ok()?;
    let text = String::from_utf8_lossy(&tail);
    let mut lines = text.lines();
    if start > 0 {
        // The first line of a tail that starts mid-file is partial.
        lines.next();
    }
    lines
        .rev()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find_map(|line| last_usage(&line))
}

/// One session as the answer shows it, with its transcript and context.
pub fn shown(session: &Session, roots: &Roots) -> Value {
    let transcript = session
        .session
        .as_deref()
        .and_then(|id| transcript(roots, &session.harness, id, &session.folder));
    let context = transcript.as_deref().and_then(context_at_end);
    json!({
        "session": session.session,
        "harness": session.harness,
        "model": session.model,
        "pid": session.pid,
        "started_at": session.started_at,
        "ended_at": session.ended_at,
        "why": session.why,
        "inside": session.inside,
        "transcript": transcript.map(|path| path.display().to_string()),
        "context_tokens": context.map_or_else(|| json!("unknown"), |tokens| json!(tokens)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(seq: u64, kind: &str, payload: Value) -> Event {
        Event {
            seq: Some(pij_core::model::Seq(seq)),
            v: 1,
            at: seq * 1000,
            kind: kind.to_string(),
            seat: Some("pij-a".into()),
            payload: payload.to_string(),
        }
    }

    fn put(seq: u64, session: Option<&str>, pid: u64) -> Event {
        event(
            seq,
            "seat.put",
            json!({"session": session, "harness": "omp", "model": "m", "folder": "/w", "proc": {"pid": pid}}),
        )
    }

    /// Plan 250 ac-0001: spawned, compacted (the id kept, then a new one),
    /// revived, then rebound on the same process; and three sessions on three
    /// processes in nine seconds show as what they were.
    #[test]
    fn the_fold_names_each_session_and_why_it_changed() {
        let events = [
            event(1, "spawn.launched", json!({})),
            put(2, None, 10),
            put(3, Some("s1"), 10),
            event(4, "watchdog.compaction-wake", json!({})),
            put(5, Some("s1"), 10),
            event(6, "watchdog.compaction-wake", json!({})),
            put(7, Some("s2"), 10),
            event(8, "seat.tombstone", json!({})),
            event(9, "seat.revive-postmortem", json!({})),
            put(10, Some("s3"), 11),
            put(11, Some("s4"), 11),
            put(12, Some("s5"), 12),
        ];
        let sessions = fold(&events);
        let whys: Vec<(&str, &str)> = sessions
            .iter()
            .map(|session| {
                (
                    session.session.as_deref().unwrap_or("-"),
                    session.why.as_str(),
                )
            })
            .collect();
        assert_eq!(
            whys,
            [
                ("s1", "spawn"),
                ("s2", "compact"),
                ("s3", "revive"),
                ("s4", "rebind"),
                ("s5", "relaunch")
            ],
            "{sessions:?}"
        );
        assert_eq!(
            sessions[0].started_at, 3000,
            "a session starts when it binds"
        );
        assert_eq!(
            sessions[0].inside,
            ["compacted at 4000", "compacted at 6000"]
        );
        assert_eq!(sessions[0].ended_at, Some(7000));
        assert_eq!(sessions[1].ended_at, Some(8000), "the tombstone ends it");
        assert_eq!(sessions[4].ended_at, None, "the last one runs");
    }

    /// Plan 250 ac-0003: the last usage figure in the tail; a file gone, or
    /// one whose figure sits before the cap, is unknown; a whole file is
    /// never read.
    #[test]
    fn context_is_read_from_a_capped_tail_or_is_unknown() {
        let dir = pij_testkit::fresh_dir("pij-session-tail");
        let path = dir.join("t.jsonl");
        let usage = json!({"message": {"usage": {"input": 3, "cacheRead": 100, "cacheWrite": 20}}});
        let copilot = json!({"type": "session.usage_record", "data": {"usage": {"copilotUsage": {"tokenDetails": [
            {"tokenType": "input", "tokenCount": 2}, {"tokenType": "cache_read", "tokenCount": 400},
            {"tokenType": "cache_write_1h", "tokenCount": 9}, {"tokenType": "output", "tokenCount": 50}]}}}});
        std::fs::write(
            &path,
            format!("{usage}\n{copilot}\n{{\"type\":\"other\"}}\n"),
        )
        .unwrap();
        assert_eq!(context_at_end(&path), Some(411), "the last figure wins");
        let filler = "x".repeat(usize::try_from(TAIL_CAP).unwrap());
        std::fs::write(&path, format!("{usage}\n{filler}\n")).unwrap();
        assert_eq!(context_at_end(&path), None, "past the cap: unknown");
        assert_eq!(context_at_end(&dir.join("gone.jsonl")), None);
        let claude = json!({"message": {"usage": {"input_tokens": 1, "cache_creation_input_tokens": 2, "cache_read_input_tokens": 3}}});
        assert_eq!(last_usage(&claude), Some(6));
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn a_transcript_is_found_by_each_harness_layout_and_never_by_a_path_in_the_id() {
        let dir = pij_testkit::fresh_dir("pij-session-layout");
        let roots = Roots {
            claude_homes: vec![dir.join("claude")],
            omp_sessions: Some(dir.join("omp")),
            copilot_sessions: Some(dir.join("copilot")),
        };
        let claude = dir.join("claude/projects/-w-a-b/c1.jsonl");
        let omp = dir.join("omp/--w-a.b--/2026-11-25T00-00-00-000Z_o1.jsonl");
        let copilot = dir.join("copilot/p1/events.jsonl");
        for path in [&claude, &omp, &copilot] {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "{}\n").unwrap();
        }
        assert_eq!(transcript(&roots, "claude", "c1", "/w/a.b"), Some(claude));
        assert_eq!(transcript(&roots, "omp", "o1", "/w/a.b"), Some(omp));
        assert_eq!(transcript(&roots, "copilot", "p1", "/x"), Some(copilot));
        assert_eq!(transcript(&roots, "copilot", "../p1", "/x"), None);
        assert_eq!(transcript(&roots, "codex", "x", "/x"), None);
        std::fs::remove_dir_all(dir).ok();
    }
}
