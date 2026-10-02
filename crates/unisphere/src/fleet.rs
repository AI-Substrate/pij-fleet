//! The transcript half of `pij fleet-report` (plan 162): Unisphere's in-process
//! prep, scoped to the sessions of one project, mapped into pij-core's fleet rows.
//!
//! Unisphere owns the transcript facts (calls, turns, events, each session's
//! working directory). This module folds every source the report's window can
//! touch into memory (no prep target directory is written), keeps the sessions
//! whose working directory lies under one of the scope's folders, plus their
//! subagents, and converts the rows. No Unisphere type crosses its API.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use pij_core::error::{PijError, Result};
use pij_core::fleet::{Call, Corpus, Event, Session, Tokens, Turn};
use unisphere_sdk::prep::{
    CallSighting, PREP_TABLE_SCHEMA_VERSION, PrepCallRow, PrepCommit, PrepCompactReport,
    PrepEventKind, PrepLoaded, PrepOptions, PrepReadLimits, PrepRequest, PrepRows, PrepSourceState,
    PrepState, PrepStore, TurnOrigin, run_prep,
};
use unisphere_sdk::{PipelineError, ReadLimits, SnapshotLimits};

use crate::{ADAPTER, SessionRoots, bindings, source_sets};

/// Commit-wave budget: rows held between commits stay bounded by this much input.
const MAX_RUN_BYTES: u64 = 512 * 1024 * 1024;
/// Some harnesses write single records far above the prep default.
const MAX_RECORD_BYTES: usize = 64 * 1024 * 1024;

/// Which sessions a fleet report reads.
#[derive(Clone, Debug)]
pub struct FleetScope {
    /// Absolute folders; a session whose working directory is one of them, or
    /// below one, is in scope.
    pub folders: Vec<PathBuf>,
    /// Sessions with a call at or after this instant (UTC ms)…
    pub since_ms: i64,
    /// …and before this one are in scope, with all their rows.
    pub until_ms: i64,
    /// Unisphere harness ids to read; empty reads every readable harness.
    pub harnesses: Vec<String>,
    /// Keep turn-opener heads (local use only).
    pub include_content: bool,
    /// Fold threads.
    pub threads: usize,
}

/// What the prep did, for the report's manifest.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PrepSummary {
    /// Unisphere's prep table schema version.
    pub table_schema_version: u32,
    /// Fold policy per harness, e.g. `claude-code` → `claude-code/prep@3`.
    pub policies: BTreeMap<String, String>,
    /// Native bytes read.
    pub bytes_read: u64,
    /// Sources discovered under every root.
    pub sources_discovered: u64,
    /// Sources folded (modified inside the window).
    pub sources_folded: u64,
    /// Sources that failed to read, with Unisphere's reason.
    pub unreadable: Vec<(String, String)>,
}

/// The scoped transcript facts.
#[derive(Clone, Debug, Default)]
pub struct FleetTranscripts {
    /// Sessions, calls, turns and events of the scope (no seats: those are pij's).
    pub corpus: Corpus,
    /// What the prep did.
    pub prep: PrepSummary,
}

/// Map a pij or Unisphere harness name to Unisphere's id.
pub fn unisphere_harness_id(name: &str) -> Option<&'static str> {
    match name {
        "claude" | "claude-code" => Some(crate::CLAUDE_CODE),
        "omp" | "oh-my-pi" => Some(crate::OH_MY_PI),
        "codex" => Some(crate::CODEX),
        "copilot" | "copilot-cli" => Some(crate::COPILOT_CLI),
        _ => None,
    }
}

/// An in-memory prep target: rows and the last committed state stay in this process.
#[derive(Default)]
struct MemoryStore {
    rows: Mutex<PrepRows>,
    state: Mutex<Option<PrepState>>,
}

impl PrepStore for MemoryStore {
    fn state(&self) -> std::result::Result<Option<PrepState>, PipelineError> {
        Ok(self.state.lock().expect("prep state").clone())
    }

    fn load(&self) -> std::result::Result<PrepLoaded, PipelineError> {
        Ok(PrepLoaded {
            state: None,
            orphans_removed: 0,
        })
    }

    fn commit(
        &self,
        rows: &PrepRows,
        state: &PrepState,
    ) -> std::result::Result<PrepCommit, PipelineError> {
        let mut held = self.rows.lock().expect("prep rows");
        held.calls.extend(rows.calls.iter().cloned());
        held.turns.extend(rows.turns.iter().cloned());
        held.events.extend(rows.events.iter().cloned());
        // Triggers carry opener heads, only with content; tool uses are not a P1 table.
        held.triggers.extend(
            rows.triggers
                .iter()
                .filter(|t| t.content_head.is_some())
                .cloned(),
        );
        *self.state.lock().expect("prep state") = Some(state.clone());
        Ok(PrepCommit::default())
    }

    fn compact(&self) -> std::result::Result<PrepCompactReport, PipelineError> {
        Ok(PrepCompactReport::default())
    }
}

/// Is `cwd` one of `folders` or below one?
pub fn under(cwd: &str, folders: &[PathBuf]) -> bool {
    let cwd = Path::new(cwd);
    folders.iter().any(|folder| cwd.starts_with(folder))
}

/// Fold every source under `roots` that the scope's window can touch, then keep the scope's sessions.
///
/// # Errors
/// [`PijError::Adapter`] when Unisphere refuses the prep as a whole (a bad
/// root set or limit); a single unreadable source is reported in
/// [`PrepSummary::unreadable`] instead.
pub fn read_fleet(roots: SessionRoots, scope: &FleetScope) -> Result<FleetTranscripts> {
    let wanted: BTreeSet<&str> = scope
        .harnesses
        .iter()
        .filter_map(|name| unisphere_harness_id(name))
        .collect();
    let sets: Vec<_> = source_sets(roots)
        .into_iter()
        .filter(|set| wanted.is_empty() || wanted.contains(set.harness.as_str()))
        .filter(|set| set.root.is_dir())
        .collect();
    let store = MemoryStore::default();
    let request = PrepRequest {
        target: PathBuf::new(),
        roots: sets,
        options: PrepOptions {
            include_content: scope.include_content,
        },
        limits: PrepReadLimits {
            read: ReadLimits {
                max_record_bytes: MAX_RECORD_BYTES,
                max_batch_bytes: MAX_RECORD_BYTES,
                ..ReadLimits::default()
            },
            snapshot: SnapshotLimits::default(),
        },
        threads: scope.threads.max(1),
        max_run_bytes: MAX_RUN_BYTES,
        // A file last written before the window cannot hold a call inside it.
        modified_since_ns: Some(i128::from(scope.since_ms) * 1_000_000),
    };
    let report = run_prep(&bindings(), &store, &request).map_err(|error| PijError::Adapter {
        adapter: ADAPTER.to_string(),
        message: format!("fleet prep failed: {error}"),
    })?;
    let mut prep = PrepSummary {
        table_schema_version: PREP_TABLE_SCHEMA_VERSION,
        bytes_read: report.bytes_read,
        ..PrepSummary::default()
    };
    for set in &report.sets {
        prep.sources_discovered += set.discovered;
        if let Some(policy) = &set.policy {
            prep.policies.insert(set.harness.clone(), policy.clone());
        }
    }
    for outcome in &report.sources {
        if let Some(error) = &outcome.error {
            prep.unreadable
                .push((outcome.source.clone(), error.clone()));
        }
    }
    let state = store
        .state
        .into_inner()
        .expect("prep state")
        .unwrap_or_default();
    let rows = store.rows.into_inner().expect("prep rows");
    prep.sources_folded = state.sources.len() as u64;
    let corpus = scoped(&state, rows, scope);
    Ok(FleetTranscripts { corpus, prep })
}

/// Keep the sessions in scope and convert their rows.
fn scoped(state: &PrepState, rows: PrepRows, scope: &FleetScope) -> Corpus {
    let calls = merge_calls(rows.calls);
    // A session is in scope when it worked in a scope folder and made a call in the window.
    let mut called_in_window: BTreeSet<&str> = BTreeSet::new();
    for call in &calls {
        if call
            .ts_ms
            .is_some_and(|ts| scope.since_ms <= ts && ts < scope.until_ms)
        {
            called_in_window.insert(call.source.as_str());
        }
    }
    let in_folder = |source: &PrepSourceState| {
        source
            .facts
            .cwd
            .as_deref()
            .is_some_and(|cwd| under(cwd, &scope.folders))
    };
    let main_ids: BTreeSet<&str> = state
        .sources
        .values()
        .filter(|s| !s.meta.is_sub && in_folder(s))
        .filter_map(|s| s.facts.session_id.as_deref())
        .collect();
    // Subagent sessions come with their parent.
    let keep: BTreeSet<&str> = state
        .sources
        .iter()
        .filter(|(key, s)| {
            called_in_window.contains(key.as_str())
                && (in_folder(s)
                    || (s.meta.is_sub
                        && [
                            s.facts.parent_session_id.as_deref(),
                            s.facts.session_id.as_deref(),
                        ]
                        .into_iter()
                        .flatten()
                        .any(|id| main_ids.contains(id))))
        })
        .map(|(key, _)| key.as_str())
        .collect();
    let set_harness: HashMap<&str, &str> = state
        .sets
        .iter()
        .map(|(key, set)| (key.as_str(), set.harness.as_str()))
        .collect();
    let sessions = keep
        .iter()
        .filter_map(|key| state.sources.get(*key).map(|s| (*key, s)))
        .map(|(key, s)| Session {
            source: key.to_string(),
            harness: set_harness
                .get(s.set.as_str())
                .copied()
                .unwrap_or_default()
                .to_string(),
            session_id: s.facts.session_id.clone(),
            parent_session_id: s.facts.parent_session_id.clone(),
            is_sub: s.meta.is_sub,
            cwd: s.facts.cwd.clone(),
            first_ms: s.facts.first_event_ms,
            last_ms: s.facts.last_event_ms,
        })
        .collect();
    let is_sub: HashMap<&str, bool> = state
        .sources
        .iter()
        .map(|(key, s)| (key.as_str(), s.meta.is_sub))
        .collect();
    let calls = calls
        .into_iter()
        .filter(|c| keep.contains(c.source.as_str()))
        .filter_map(|c| {
            let sub = is_sub.get(c.source.as_str()).copied().unwrap_or(false);
            call(c, sub)
        })
        .collect();
    // The last opener before a turn's first call is the one that opened it.
    let mut heads: HashMap<(String, i64), String> = HashMap::new();
    for trigger in rows.triggers {
        if let Some(head) = trigger.content_head {
            heads.insert((trigger.source, trigger.next_turn_no), head);
        }
    }
    let turns = rows
        .turns
        .into_iter()
        .filter(|t| keep.contains(t.source.as_str()))
        .map(|t| Turn {
            head: heads.remove(&(t.source.clone(), t.turn_no)),
            source: t.source,
            turn_no: t.turn_no,
            origin: origin(t.origin).to_string(),
            sender: t.sender,
            pij_msg_id: t.pij_msg_id,
            started_ms: t.started_ts_ms,
        })
        .collect();
    let events = rows
        .events
        .into_iter()
        .filter(|e| keep.contains(e.source.as_str()))
        .map(|e| Event {
            source: e.source,
            ts_ms: e.ts_ms,
            kind: event_kind(e.kind).to_string(),
            subkind: e.subkind,
            trigger: e.trigger,
            model: e.model,
            pre_tokens: e.pre_tokens,
            post_tokens: e.post_tokens,
            duration_ms: e.duration_ms,
            last_context: e.last_context,
            gap_ms: e.gap_ms.filter(|gap| *gap >= 0),
            resets_at: e.resets_at,
        })
        .collect();
    Corpus {
        sessions,
        calls,
        turns,
        events,
        ..Corpus::default()
    }
}

/// Merge the sightings of one `(source, generation, msg_id, request_id)` call, as
/// Unisphere's `calls_v` does: counters take the per-field maximum, `stop_reason`
/// the last non-null value, every other field the first sighting's. A call with
/// neither id is its own row.
fn merge_calls(rows: Vec<PrepCallRow>) -> Vec<PrepCallRow> {
    let mut out: Vec<PrepCallRow> = Vec::with_capacity(rows.len());
    let mut index: HashMap<(String, u32, String, String), usize> = HashMap::new();
    for row in rows {
        let key = match (&row.msg_id, &row.request_id) {
            (None, None) => None,
            (msg, request) => Some((
                row.source.clone(),
                row.generation,
                msg.clone().unwrap_or_default(),
                request.clone().unwrap_or_default(),
            )),
        };
        let Some(key) = key else {
            out.push(row);
            continue;
        };
        match index.get(&key) {
            None => {
                index.insert(key, out.len());
                out.push(row);
            }
            Some(&at) => {
                let first = &mut out[at];
                let max = |a: &mut Option<i64>, b: Option<i64>| {
                    *a = match (*a, b) {
                        (Some(x), Some(y)) => Some(x.max(y)),
                        (x, y) => x.or(y),
                    };
                };
                max(&mut first.input, row.input);
                max(&mut first.cw_1h, row.cw_1h);
                max(&mut first.cw_5m, row.cw_5m);
                max(&mut first.cache_read, row.cache_read);
                max(&mut first.output, row.output);
                if row.stop_reason.is_some() {
                    first.stop_reason = row.stop_reason;
                }
                if first.sighting == CallSighting::Update && row.sighting == CallSighting::First {
                    // An update folded before its first sighting keeps the first's ordering facts.
                    first.ts = row.ts;
                    first.ts_ms = row.ts_ms;
                    first.gap_ms = row.gap_ms;
                    first.turn_no = row.turn_no;
                    first.call_in_turn = row.call_in_turn;
                    first.sighting = CallSighting::First;
                }
            }
        }
    }
    out
}

fn count(value: Option<i64>) -> u64 {
    value.unwrap_or(0).max(0) as u64
}

fn call(row: PrepCallRow, is_sub: bool) -> Option<Call> {
    Some(Call {
        ts_ms: row.ts_ms?,
        source: row.source,
        is_sub,
        model: row.model,
        tokens: Tokens {
            input: count(row.input),
            cw_1h: count(row.cw_1h),
            cw_5m: count(row.cw_5m),
            cache_read: count(row.cache_read),
            output: count(row.output),
        },
        gap_ms: row.gap_ms.filter(|gap| *gap >= 0),
        turn_no: row.turn_no.unwrap_or(0),
        call_in_turn: row.call_in_turn.unwrap_or(0),
    })
}

fn origin(origin: TurnOrigin) -> &'static str {
    match origin {
        TurnOrigin::Start => "start",
        TurnOrigin::Human => "human",
        TurnOrigin::Peer => "peer",
        TurnOrigin::TaskNotification => "task-notification",
        TurnOrigin::Coordinator => "coordinator",
        TurnOrigin::AutoContinuation => "auto-continuation",
        TurnOrigin::CompactSummary => "compact-summary",
        TurnOrigin::ManualCompact => "manual-compact",
        TurnOrigin::Scheduled => "scheduled",
        TurnOrigin::Loop => "loop",
        TurnOrigin::SubagentTask => "subagent-task",
        TurnOrigin::Other => "other",
    }
}

fn event_kind(kind: PrepEventKind) -> &'static str {
    match kind {
        PrepEventKind::Compaction => "compaction",
        PrepEventKind::Recap => "recap",
        PrepEventKind::ScheduledFire => "scheduled_fire",
        PrepEventKind::LimitNotice => "limit_notice",
        PrepEventKind::QueueOp => "queue_op",
        PrepEventKind::ModelSwitch => "model_switch",
        PrepEventKind::ApiError => "api_error",
        PrepEventKind::SystemOther => "system_other",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2026-09-24T00:00:00Z.
    const SINCE: i64 = 1_790_208_000_000;

    fn user(uuid: &str, session: &str, cwd: &str, at: &str, text: &str) -> String {
        format!(
            r#"{{"uuid":"{uuid}","parentUuid":null,"sessionId":"{session}","cwd":"{cwd}","type":"user","timestamp":"{at}","message":{{"role":"user","content":"{text}"}}}}"#
        ) + "\n"
    }

    fn assistant(uuid: &str, msg: &str, session: &str, cwd: &str, at: &str, output: u64) -> String {
        format!(
            r#"{{"uuid":"{uuid}","parentUuid":null,"sessionId":"{session}","cwd":"{cwd}","type":"assistant","timestamp":"{at}","requestId":"req-{msg}","message":{{"id":"{msg}","role":"assistant","model":"claude-opus-5-5","content":[{{"type":"text","text":"y"}}],"usage":{{"input_tokens":10,"output_tokens":{output},"cache_read_input_tokens":40000,"cache_creation_input_tokens":100}}}}}}"#
        ) + "\n"
    }

    fn transcript(session: &str, cwd: &str, at: &str) -> String {
        user("u1", session, cwd, at, "hello")
            + &assistant("a1", &format!("m-{session}"), session, cwd, at, 5)
    }

    struct Home(PathBuf);

    impl Home {
        fn new(name: &str) -> Self {
            let home =
                std::env::temp_dir().join(format!("pij-fleet-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&home);
            std::fs::create_dir_all(home.join("projects")).unwrap();
            Self(home)
        }

        fn write(&self, file: &str, body: &str) {
            let path = self.0.join("projects").join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, body).unwrap();
        }

        fn read(&self, folders: &[&str], include_content: bool) -> FleetTranscripts {
            read_fleet(
                SessionRoots {
                    claude_homes: vec![self.0.clone()],
                    ..SessionRoots::default()
                },
                &FleetScope {
                    folders: folders.iter().map(PathBuf::from).collect(),
                    since_ms: SINCE,
                    until_ms: SINCE + 86_400_000,
                    harnesses: Vec::new(),
                    include_content,
                    threads: 2,
                },
            )
            .expect("fleet read")
        }
    }

    impl Drop for Home {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn session_ids(read: &FleetTranscripts) -> Vec<String> {
        let mut ids: Vec<String> = read
            .corpus
            .sessions
            .iter()
            .filter_map(|s| s.session_id.clone())
            .collect();
        ids.sort();
        ids
    }

    /// A session is in scope when its working directory is a scope folder or
    /// below one — by path component, so `/work/demo-two` is not under `/work/demo`.
    #[test]
    fn sessions_are_scoped_by_working_directory_components() {
        let home = Home::new("scope");
        home.write(
            "p/in.jsonl",
            &transcript("in", "/work/demo", "2026-09-24T01:00:00Z"),
        );
        home.write(
            "p/below.jsonl",
            &transcript("below", "/work/demo/sub", "2026-09-24T01:00:00Z"),
        );
        home.write(
            "p/sibling.jsonl",
            &transcript("sibling", "/work/demo-two", "2026-09-24T01:00:00Z"),
        );
        home.write(
            "p/out.jsonl",
            &transcript("out", "/work/other", "2026-09-24T01:00:00Z"),
        );
        let read = home.read(&["/work/demo"], false);
        assert_eq!(session_ids(&read), ["below", "in"]);
        assert_eq!(read.corpus.calls.len(), 2);
        assert!(
            read.prep.policies.contains_key("claude-code"),
            "{:?}",
            read.prep
        );
    }

    /// A subagent rides with its parent even when it recorded no working directory.
    #[test]
    fn a_subagent_session_comes_with_its_parent() {
        let home = Home::new("sub");
        home.write(
            "p/par.jsonl",
            &transcript("par", "/work/demo", "2026-09-24T01:00:00Z"),
        );
        home.write(
            "p/par/subagents/agent-x.jsonl",
            &(user("s1", "par", "", "2026-09-24T01:01:00Z", "task")
                + &assistant("s2", "m-sub", "par", "", "2026-09-24T01:01:01Z", 3)),
        );
        let read = home.read(&["/work/demo"], false);
        let subs: Vec<&Session> = read.corpus.sessions.iter().filter(|s| s.is_sub).collect();
        assert_eq!(subs.len(), 1, "{:?}", read.corpus.sessions);
        assert!(read.corpus.calls.iter().any(|c| c.is_sub));
    }

    /// A subagent with no working directory rides along only with an in-scope parent.
    #[test]
    fn a_subagent_of_an_out_of_scope_parent_stays_out() {
        let home = Home::new("sub-out");
        home.write(
            "p/par.jsonl",
            &transcript("par", "/work/other", "2026-09-24T01:00:00Z"),
        );
        home.write(
            "p/par/subagents/agent-x.jsonl",
            &(user("s1", "par", "", "2026-09-24T01:01:00Z", "task")
                + &assistant("s2", "m-sub", "par", "", "2026-09-24T01:01:01Z", 3)),
        );
        let read = home.read(&["/work/demo"], false);
        assert!(
            read.corpus.sessions.is_empty(),
            "{:?}",
            read.corpus.sessions
        );
    }

    /// Only sessions with a call inside the window are in scope; they bring every row.
    #[test]
    fn a_session_needs_a_call_in_the_window_and_brings_all_its_rows() {
        let home = Home::new("window");
        home.write(
            "p/old.jsonl",
            &transcript("old", "/work/demo", "2026-09-20T01:00:00Z"),
        );
        home.write(
            "p/edge.jsonl",
            &(transcript("edge", "/work/demo", "2026-09-23T23:00:00Z")
                + &assistant(
                    "a2",
                    "m-edge-2",
                    "edge",
                    "/work/demo",
                    "2026-09-24T00:30:00Z",
                    9,
                )),
        );
        let read = home.read(&["/work/demo"], false);
        assert_eq!(session_ids(&read), ["edge"]);
        assert_eq!(
            read.corpus.calls.len(),
            2,
            "the pre-window call rides along"
        );
    }

    /// End to end, repeated records of one API call are one call (the fold merges
    /// records it sees in one batch).
    #[test]
    fn repeated_records_of_one_call_are_one_call() {
        let home = Home::new("dedupe");
        let at = "2026-09-24T01:00:00Z";
        home.write(
            "p/d.jsonl",
            &(user("u1", "d", "/work/demo", at, "go")
                + &assistant("a1", "m-1", "d", "/work/demo", at, 5)
                + &assistant("a2", "m-1", "d", "/work/demo", at, 90)),
        );
        let read = home.read(&["/work/demo"], false);
        assert_eq!(read.corpus.calls.len(), 1);
        assert_eq!(read.corpus.calls[0].tokens.output, 90);
        assert_eq!(read.corpus.calls[0].tokens.cache_read, 40_000);
    }

    fn sighting(sighting: CallSighting, output: i64, stop: Option<&str>) -> PrepCallRow {
        PrepCallRow {
            source: "claude-code/default/p/s.jsonl".into(),
            generation: 0,
            native_offset: Some(0),
            native_key: None,
            sighting,
            msg_id: Some("m-1".into()),
            request_id: Some("r-1".into()),
            ts: None,
            ts_ms: Some(SINCE),
            model: None,
            stop_reason: stop.map(Into::into),
            input: Some(1),
            cw_1h: Some(5),
            cw_5m: None,
            cache_read: Some(7),
            output: Some(output),
            cache_write_basis: unisphere_sdk::prep::CacheWriteBasis::Split,
            is_sidechain: false,
            gap_ms: Some(-1),
            turn_no: Some(1),
            call_in_turn: Some(1),
            records: 1,
        }
    }

    /// An `Update` sighting (a later batch raising a call's counters) merges into
    /// its `First` as calls_v does: per-field maximum, last non-null stop reason.
    #[test]
    fn an_update_sighting_merges_into_its_first() {
        let mut other = sighting(CallSighting::First, 1, None);
        other.msg_id = Some("m-2".into());
        let merged = merge_calls(vec![
            sighting(CallSighting::First, 5, None),
            other,
            sighting(CallSighting::Update, 90, Some("end_turn")),
        ]);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].output, Some(90));
        assert_eq!(merged[0].stop_reason.as_deref(), Some("end_turn"));
        assert_eq!(merged[0].cw_1h, Some(5));
    }

    /// Opener heads exist only with `include_content`; metadata mode carries none.
    #[test]
    fn opener_heads_are_read_only_with_content() {
        let home = Home::new("content");
        home.write(
            "p/c.jsonl",
            &transcript("c", "/work/demo", "2026-09-24T01:00:00Z"),
        );
        let bare = home.read(&["/work/demo"], false);
        assert!(
            bare.corpus.turns.iter().all(|t| t.head.is_none()),
            "{:?}",
            bare.corpus.turns
        );
        let rich = home.read(&["/work/demo"], true);
        assert!(
            rich.corpus
                .turns
                .iter()
                .any(|t| t.head.as_deref() == Some("hello")),
            "{:?}",
            rich.corpus.turns
        );
    }
}
