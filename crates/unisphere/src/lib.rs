//! `pij-unisphere`: pij's [`SessionStatusPort`] over the in-process Unisphere SDK.
//!
//! This is the only crate that names Unisphere. It turns the SDK's
//! harness-neutral `SessionStatus` into pij's [`SeatStatus`], so a Unisphere
//! rename costs one edit here and none in core or the daemon.
//!
//! **One read position per seat.** The SDK returns an opaque cursor with every
//! answer. Handing it back reads only the bytes the harness appended since. When
//! a transcript rotates or a seat's session changes, the SDK re-reads cold and
//! says why (`SeatStatus::reset`). On a failed read, the seat keeps its previous
//! cursor. A cursor resumes once, so reads of one seat are serialized: a
//! concurrent `pij state` for the same seat waits and then uses the latest
//! cursor, rather than handing the SDK a spent one that would refold cold.
//!
//! **A missing transcript is looked for once per [`MISS_TTL`] (F209).** A
//! Copilot seat names its transcript's one path, so a miss is one `stat`,
//! never a walk of every session. Harnesses whose path needs more than the
//! session id (Claude's project folder, OMP's timestamp, Codex's date) are
//! still found by the SDK's walk of their roots, and the kept miss bounds
//! that walk to one per seat per lifetime. Reads of one seat queue on its
//! slot, so concurrent reads of a missing seat share the first one's answer.

pub mod fleet;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use pij_core::error::{PijError, Result};
use pij_core::model::{Harness, SeatId};
use pij_core::ports::SessionStatusPort;
use pij_core::session_status::{
    Basis, CacheTtl, Fact, SEAT_STATUS_VERSION, SeatStatus, SessionStatusReply, SessionTarget,
};
use unisphere_sdk::prep::{PrepBinding, PrepSourceSet, default_set, derived_root_label};
use unisphere_sdk::status::{
    Basis as UniBasis, Fact as UniFact, SessionStatus, StatusCursor, StatusFailureKind,
    StatusService, StatusTarget,
};

pub(crate) const ADAPTER: &str = "unisphere/session-status";
/// Unisphere's adapter id for Claude Code transcripts.
pub(crate) const CLAUDE_CODE: &str = "claude-code";
/// Unisphere's adapter id for Oh My Pi (pij `omp`) session files.
pub(crate) const OH_MY_PI: &str = "oh-my-pi";
/// Unisphere's adapter id for Codex rollouts.
pub(crate) const CODEX: &str = "codex";
/// Unisphere's adapter id for Copilot CLI event logs.
pub(crate) const COPILOT_CLI: &str = "copilot-cli";

/// Where each readable harness keeps its sessions.
#[derive(Clone, Debug, Default)]
pub struct SessionRoots {
    /// Every Claude configuration home (`~/.claude`, `~/.claude-alt`, …); each
    /// home's `projects/` directory is one discovery root.
    pub claude_homes: Vec<PathBuf>,
    /// OMP's session directory (`~/.omp/agent/sessions`).
    pub omp_sessions: Option<PathBuf>,
    /// Codex's session directory (`$CODEX_HOME/sessions`, `~/.codex/sessions`).
    pub codex_sessions: Option<PathBuf>,
    /// Copilot CLI's session directory (`~/.copilot/session-state`), one
    /// `<session>/events.jsonl` per session.
    pub copilot_sessions: Option<PathBuf>,
}

/// How long a seat's "no transcript" answer is kept before the next look
/// (F209). A TTL, not a root's mtime: every root's mtime moves with every
/// other session on a busy fleet, so it would expire on nearly every read. A
/// transcript that appears is found within this long.
pub const MISS_TTL: Duration = Duration::from_secs(30);

/// Session facts from harness transcripts, read through `unisphere-sdk`.
pub struct UnisphereSessionStatus {
    service: Arc<StatusService>,
    copilot_sessions: Option<PathBuf>,
    miss_ttl: Duration,
    cursors: Mutex<HashMap<SeatId, Arc<tokio::sync::Mutex<SeatRead>>>>,
}

/// One seat's read state, held across a read.
#[derive(Default)]
struct SeatRead {
    cursor: Option<StatusCursor>,
    /// The last read found no transcript: for which target, when, and why.
    missed: Option<(StatusTarget, Instant, String)>,
}

impl UnisphereSessionStatus {
    /// A source over every Claude configuration home (`~/.claude`,
    /// `~/.claude-alt`, …). Each home's `projects/` directory is one discovery root.
    pub fn new(claude_homes: Vec<PathBuf>) -> Self {
        Self::with_roots(SessionRoots {
            claude_homes,
            ..SessionRoots::default()
        })
    }

    /// A source over every readable harness's session directories.
    pub fn with_roots(roots: SessionRoots) -> Self {
        Self::from_parts(bindings(), roots, MISS_TTL)
    }

    /// The same source, keeping a miss for `ttl` instead of [`MISS_TTL`]:
    /// for tests that need a miss to expire.
    #[must_use]
    pub fn with_miss_ttl(mut self, ttl: Duration) -> Self {
        self.miss_ttl = ttl;
        self
    }

    fn from_parts(bindings: Vec<PrepBinding>, roots: SessionRoots, miss_ttl: Duration) -> Self {
        Self {
            copilot_sessions: roots.copilot_sessions.clone(),
            service: Arc::new(StatusService::new(bindings, source_sets(roots))),
            miss_ttl,
            cursors: Mutex::new(HashMap::new()),
        }
    }
}

/// One discovery root per readable harness directory.
pub(crate) fn source_sets(roots: SessionRoots) -> Vec<PrepSourceSet> {
    let SessionRoots {
        claude_homes,
        omp_sessions,
        codex_sessions,
        copilot_sessions,
    } = roots;
    let mut sets: Vec<PrepSourceSet> = claude_homes
        .into_iter()
        .enumerate()
        .map(|(index, home)| {
            let root = home.join("projects");
            // The catalogue label for the first home, a derived one for the
            // rest: the label is part of every source key and must not repeat.
            if index == 0 {
                default_set(CLAUDE_CODE, root)
            } else {
                PrepSourceSet {
                    harness: CLAUDE_CODE.to_string(),
                    label: derived_root_label(&root),
                    root,
                }
            }
        })
        .collect();
    sets.extend(omp_sessions.map(|root| default_set(OH_MY_PI, root)));
    sets.extend(codex_sessions.map(|root| default_set(CODEX, root)));
    sets.extend(copilot_sessions.map(|root| default_set(COPILOT_CLI, root)));
    sets
}

/// The fold and loader of every readable harness.
pub(crate) fn bindings() -> Vec<PrepBinding> {
    vec![
        PrepBinding {
            fold: Arc::new(unisphere_adapter_claude::ClaudePrepFold),
            loader: Arc::new(unisphere_loader_jsonl::FileSessionLoader),
        },
        PrepBinding {
            fold: Arc::new(unisphere_adapter_omp::OmpPrepFold),
            loader: Arc::new(unisphere_loader_jsonl::FileSessionLoader),
        },
        PrepBinding {
            fold: Arc::new(unisphere_adapter_codex::CodexPrepFold),
            loader: Arc::new(unisphere_loader_jsonl::FileSessionLoader),
        },
        // The events.jsonl fold only: the legacy `<session>.json` fold needs
        // loader-snapshot, which pulls rusqlite (see the workspace Cargo.toml).
        PrepBinding {
            fold: Arc::new(unisphere_adapter_copilot_cli::CopilotCliPrepFold),
            loader: Arc::new(unisphere_loader_jsonl::FileSessionLoader),
        },
    ]
}

/// Unisphere's adapter id for a pij harness, when the SDK can read it.
fn unisphere_harness(harness: Harness) -> Option<&'static str> {
    match harness {
        Harness::Claude => Some(CLAUDE_CODE),
        Harness::Omp => Some(OH_MY_PI),
        Harness::Codex => Some(CODEX),
        // Copilot persists no per-call usage: its context stays unknown.
        Harness::Copilot => Some(COPILOT_CLI),
        // A fold exists upstream but is provisional until Unisphere verifies it.
        Harness::Pi => None,
    }
}

impl UnisphereSessionStatus {
    /// Whether a read position is held for `seat` (tests only).
    #[cfg(test)]
    async fn holds_cursor(&self, seat: &SeatId) -> bool {
        let slot = self
            .cursors
            .lock()
            .expect("session-status cursor map")
            .get(seat)
            .cloned();
        match slot {
            Some(slot) => slot.lock().await.cursor.is_some(),
            None => false,
        }
    }
}

#[async_trait]
impl SessionStatusPort for UnisphereSessionStatus {
    async fn status(&self, target: &SessionTarget) -> Result<SessionStatusReply> {
        let Some(harness) = unisphere_harness(target.harness) else {
            return Ok(SessionStatusReply::Unsupported);
        };
        // Copilot's one accepted layout is `<session-state>/<id>/events.jsonl`:
        // name it, so a miss is one stat and never a walk of every session.
        let transcript = match target.harness {
            Harness::Copilot => self
                .copilot_sessions
                .as_ref()
                .map(|root| root.join(&target.session).join("events.jsonl")),
            Harness::Claude | Harness::Omp | Harness::Codex | Harness::Pi => None,
        };
        let request = StatusTarget {
            harness: harness.to_string(),
            session_id: target.session.clone(),
            transcript,
        };
        let slot = Arc::clone(
            self.cursors
                .lock()
                .expect("session-status cursor map")
                .entry(target.seat.clone())
                .or_default(),
        );
        let now_ms = now_ms()?;
        let service = Arc::clone(&self.service);
        let miss_ttl = self.miss_ttl;
        // The whole take -> fold -> store runs in a detached task that owns the
        // seat's slot. A caller that stops waiting (the cold-wake guard's bound)
        // drops only the handle; the fold still finishes and keeps its cursor,
        // and a later read of the seat queues behind it and resumes warm.
        let read = tokio::spawn(async move {
            // Held across the read: one reader per seat, always the latest
            // cursor, and a queued reader sees the miss the one before it kept.
            let mut seat = slot.lock_owned().await;
            if let Some((missed, at, detail)) = &seat.missed
                && *missed == request
                && at.elapsed() < miss_ttl
            {
                return Ok(SessionStatusReply::NotFound {
                    detail: detail.clone(),
                });
            }
            let previous = seat.cursor.take();
            let asked = request.clone();
            // A cold fold reads a whole transcript: keep it off the reactor.
            let (answer, previous) = tokio::task::spawn_blocking(move || {
                let answer = service.status_incremental(&asked, previous.as_ref(), now_ms);
                (answer, previous)
            })
            .await
            .map_err(|error| PijError::Adapter {
                adapter: ADAPTER.to_string(),
                message: format!("status read task failed: {error}"),
            })?;
            seat.missed = None;
            match answer {
                Ok((status, next)) => {
                    seat.cursor = Some(next);
                    Ok(SessionStatusReply::Status(seat_status(&status)))
                }
                Err(failure) => {
                    // The SDK leaves a failed read's cursor valid: keep it.
                    seat.cursor = previous;
                    match failure.kind {
                        StatusFailureKind::UnsupportedHarness => {
                            Ok(SessionStatusReply::Unsupported)
                        }
                        StatusFailureKind::TranscriptNotFound => {
                            let detail = format!("{}: {}", failure.code(), failure.message);
                            seat.missed = Some((request, Instant::now(), detail.clone()));
                            Ok(SessionStatusReply::NotFound { detail })
                        }
                        _ => Err(PijError::Adapter {
                            adapter: ADAPTER.to_string(),
                            message: format!("{}: {}", failure.code(), failure.message),
                        }),
                    }
                }
            }
        });
        read.await.map_err(|error| PijError::Adapter {
            adapter: ADAPTER.to_string(),
            message: format!("status read task failed: {error}"),
        })?
    }
}

fn now_ms() -> Result<i64> {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| PijError::Adapter {
            adapter: ADAPTER.to_string(),
            message: format!("system clock before the epoch: {error}"),
        })?;
    i64::try_from(elapsed.as_millis()).map_err(|_| PijError::Adapter {
        adapter: ADAPTER.to_string(),
        message: "system clock beyond i64 milliseconds".to_string(),
    })
}

/// The pij basis for a Unisphere basis. `table` carries the table's version,
/// which Unisphere names separately (e.g. `model-windows@1`).
fn basis(basis: UniBasis, table: Option<&str>) -> Option<Basis> {
    Some(match basis {
        UniBasis::Native => Basis::Native,
        UniBasis::Derived => Basis::Derived,
        UniBasis::MtimeFallback => Basis::MtimeFallback,
        UniBasis::Table => Basis::Table {
            version: table?.rsplit_once('@')?.1.parse().ok()?,
        },
    })
}

fn fact<T: Clone, U>(
    fact: Option<&UniFact<T>>,
    table: Option<&str>,
    map: impl FnOnce(&T) -> Option<U>,
) -> Fact<U> {
    let Some(fact) = fact else {
        return Fact::Unknown;
    };
    match (basis(fact.basis, table), map(&fact.value)) {
        (Some(basis), Some(value)) => Fact::Known { value, basis },
        _ => Fact::Unknown,
    }
}

/// A native count the SDK carries without a basis: known when present.
fn native_u64(value: Option<u64>) -> Fact<u64> {
    value.map_or(Fact::Unknown, Fact::native)
}

/// Map the SDK's answer into pij's shape. Unknown stays unknown, never 0.
pub fn seat_status(status: &SessionStatus) -> SeatStatus {
    let last_call = status.last_call.as_ref();
    SeatStatus {
        version: SEAT_STATUS_VERSION,
        model: fact(status.model.current.as_ref(), None, |model: &String| {
            Some(model.clone())
        }),
        context_used_tokens: fact(status.context.used_tokens.as_ref(), None, |v: &u64| {
            Some(*v)
        }),
        context_window_tokens: fact(
            status.context.window_tokens.as_ref(),
            status.context.window_table.as_deref(),
            |v: &u64| Some(*v),
        ),
        last_call_at_ms: native_u64(
            last_call
                .and_then(|call| call.at_ms)
                .and_then(|at| u64::try_from(at).ok()),
        ),
        last_call_input_tokens: native_u64(last_call.and_then(|call| call.input)),
        last_call_cache_read_tokens: native_u64(last_call.and_then(|call| call.cache_read)),
        last_call_cache_write_5m_tokens: native_u64(last_call.and_then(|call| call.cache_write_5m)),
        last_call_cache_write_1h_tokens: native_u64(last_call.and_then(|call| call.cache_write_1h)),
        // Any recorded lifetime, not just 5m/1h: Copilot's GPT models record
        // 1800 s, which the SDK labels `30m` (F214).
        cache_ttl: fact(
            last_call.and_then(|call| call.ttl_bucket.as_ref()),
            None,
            |bucket: &String| CacheTtl::from_label(bucket),
        ),
        compactions: status
            .compaction
            .counts
            .as_ref()
            .map_or(Fact::Unknown, |counts| {
                Fact::native(counts.manual + counts.auto + counts.unknown_trigger)
            }),
        reset: status.source.reset.clone(),
    }
}

#[cfg(test)]
mod tests {
    use unisphere_sdk::status::{CompactionCounts, LastCall};

    use super::*;

    fn empty() -> SessionStatus {
        SessionStatus::empty(StatusTarget {
            harness: CLAUDE_CODE.to_string(),
            session_id: "s".to_string(),
            transcript: None,
        })
    }

    #[test]
    fn an_empty_status_maps_to_every_fact_unknown_never_zero() {
        let mapped = seat_status(&empty());
        assert_eq!(
            mapped,
            SeatStatus {
                // The adapter adds nothing an empty status did not say.
                ..SeatStatus::unknown()
            }
        );
    }

    #[test]
    fn known_facts_keep_their_basis_and_the_window_table_version() {
        let mut status = empty();
        status.model.current = Some(UniFact::new("claude-opus-5".into(), UniBasis::Native));
        status.context.used_tokens = Some(UniFact::new(142_000, UniBasis::Derived));
        status.context.window_tokens = Some(UniFact::new(1_000_000, UniBasis::Table));
        status.context.window_table = Some("model-windows@3".into());
        status.last_call = Some(LastCall {
            at_ms: Some(1_790_000_000_000),
            input: Some(0),
            cache_read: Some(140_000),
            cache_write_1h: Some(2_000),
            cache_write_5m: None,
            ttl_bucket: Some(UniFact::new("1h".into(), UniBasis::Derived)),
            ..LastCall::default()
        });
        status.compaction.counts = Some(CompactionCounts {
            manual: 1,
            auto: 2,
            unknown_trigger: 0,
        });
        status.source.reset = Some("rotated".into());

        let mapped = seat_status(&status);
        assert_eq!(mapped.model, Fact::native("claude-opus-5".to_string()));
        assert_eq!(mapped.context_used_tokens, Fact::derived(142_000));
        assert_eq!(
            mapped.context_window_tokens,
            Fact::Known {
                value: 1_000_000,
                basis: Basis::Table { version: 3 }
            }
        );
        assert_eq!(mapped.last_call_at_ms, Fact::native(1_790_000_000_000));
        assert_eq!(
            mapped.last_call_input_tokens,
            Fact::native(0),
            "a recorded 0 is a fact"
        );
        assert_eq!(mapped.last_call_cache_write_5m_tokens, Fact::Unknown);
        assert_eq!(mapped.cache_ttl, Fact::derived(CacheTtl::ONE_HOUR));
        assert_eq!(mapped.compactions, Fact::native(3));
        assert_eq!(mapped.reset.as_deref(), Some("rotated"));
    }

    #[test]
    fn an_unversioned_table_or_unreadable_bucket_is_unknown_not_guessed() {
        let mut status = empty();
        status.context.window_tokens = Some(UniFact::new(200_000, UniBasis::Table));
        status.last_call = Some(LastCall {
            ttl_bucket: Some(UniFact::new("five-ish".into(), UniBasis::Derived)),
            ..LastCall::default()
        });
        let mapped = seat_status(&status);
        assert_eq!(mapped.context_window_tokens, Fact::Unknown);
        assert_eq!(mapped.cache_ttl, Fact::Unknown);
    }

    /// F214: Copilot records 1800 s for GPT models; the SDK labels it `30m`
    /// natively. It is a 30-minute TTL, never "unknown".
    #[test]
    fn a_native_thirty_minute_ttl_is_thirty_minutes() {
        let mut status = empty();
        status.last_call = Some(LastCall {
            ttl_bucket: Some(UniFact::new("30m".into(), UniBasis::Native)),
            ..LastCall::default()
        });
        assert_eq!(
            seat_status(&status).cache_ttl,
            Fact::native(CacheTtl::from_seconds(1_800).expect("30m"))
        );
    }

    fn record(uuid: &str, parent: &str, kind: &str, at: &str, usage: &str) -> String {
        let message = if kind == "user" {
            r#"{"role":"user","content":"x"}"#.to_string()
        } else {
            format!(
                r#"{{"id":"{uuid}-r","role":"assistant","model":"claude-fixture","content":[{{"type":"text","text":"y"}}],"usage":{usage}}}"#
            )
        };
        format!(
            r#"{{"uuid":"{uuid}","parentUuid":{parent},"sessionId":"s-1","type":"{kind}","timestamp":"{at}","message":{message}}}"#
        ) + "\n"
    }

    /// A seat's reads are serialized and always hand the SDK the latest cursor,
    /// so repeated and concurrent reads stay warm instead of refolding a spent one.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn repeated_and_concurrent_reads_of_one_seat_stay_warm() {
        let home = std::env::temp_dir().join(format!("pij-unisphere-{}", std::process::id()));
        let project = home.join("projects").join("proj");
        std::fs::create_dir_all(&project).unwrap();
        let transcript = project.join("s-1.jsonl");
        let usage = r#"{"input_tokens":10,"output_tokens":2,"cache_read_input_tokens":4,"cache_creation_input_tokens":1}"#;
        std::fs::write(
            &transcript,
            record("u1", "null", "user", "2026-09-08T00:00:00Z", "")
                + &record("a1", "\"u1\"", "assistant", "2026-09-08T00:00:01Z", usage),
        )
        .unwrap();
        let source = Arc::new(UnisphereSessionStatus::new(vec![home.clone()]));
        let target = SessionTarget {
            seat: SeatId("pij-cursor".into()),
            harness: Harness::Claude,
            session: "s-1".into(),
        };
        let read = |source: Arc<UnisphereSessionStatus>, target: SessionTarget| async move {
            match source.status(&target).await.expect("status") {
                SessionStatusReply::Status(status) => status,
                other => panic!("{other:?}"),
            }
        };
        let cold = read(Arc::clone(&source), target.clone()).await;
        assert_eq!(cold.context_used_tokens.value(), Some(&15));
        assert_eq!(cold.reset, None);

        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&transcript)
            .unwrap();
        std::io::Write::write_all(
            &mut file,
            record("a2", "\"a1\"", "assistant", "2026-09-08T00:00:02Z", usage).as_bytes(),
        )
        .unwrap();
        let (first, second) = tokio::join!(
            read(Arc::clone(&source), target.clone()),
            read(Arc::clone(&source), target.clone()),
        );
        for status in [&first, &second] {
            assert_eq!(
                status.reset, None,
                "a spent or cloned cursor would refold cold"
            );
            assert_eq!(status.context_used_tokens.value(), Some(&15));
        }
        std::fs::remove_dir_all(&home).unwrap();
    }

    /// Review finding 1: a caller that gives up on a read (the cold-wake
    /// guard's 3 s wait) must not throw away the fold it started. The read
    /// finishes, its cursor is kept, and the next read is warm.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_cancelled_read_still_keeps_its_cursor() {
        let home =
            std::env::temp_dir().join(format!("pij-unisphere-cancel-{}", std::process::id()));
        let project = home.join("projects").join("proj");
        std::fs::create_dir_all(&project).unwrap();
        let usage = r#"{"input_tokens":10,"output_tokens":2,"cache_read_input_tokens":4,"cache_creation_input_tokens":1}"#;
        std::fs::write(
            project.join("s-1.jsonl"),
            record("u1", "null", "user", "2026-09-08T00:00:00Z", "")
                + &record("a1", "\"u1\"", "assistant", "2026-09-08T00:00:01Z", usage),
        )
        .unwrap();
        let source = UnisphereSessionStatus::new(vec![home.clone()]);
        let target = SessionTarget {
            seat: SeatId("pij-cancel".into()),
            harness: Harness::Claude,
            session: "s-1".into(),
        };
        // Hold the seat's slot so the read cannot finish before the caller
        // gives up. A zero timeout alone raced a tiny fold on a fast runner.
        let slot = Arc::clone(
            source
                .cursors
                .lock()
                .expect("cursor map")
                .entry(target.seat.clone())
                .or_default(),
        );
        let held = slot.lock_owned().await;
        let cancelled =
            tokio::time::timeout(std::time::Duration::ZERO, source.status(&target)).await;
        assert!(cancelled.is_err(), "the caller gave up");
        drop(held);
        let mut kept = false;
        for _ in 0..200 {
            if source.holds_cursor(&target.seat).await {
                kept = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert!(
            kept,
            "the abandoned read's cursor was lost, so the next read refolds cold"
        );
        std::fs::remove_dir_all(&home).unwrap();
    }

    /// A seat compacted since its last call has the harness's post-compaction
    /// context, not the large pre-compaction one: otherwise the cold-wake guard
    /// would refuse a freshly compacted seat as if it were still large.
    #[tokio::test]
    async fn a_compaction_since_the_last_call_sets_the_context_to_its_post_tokens() {
        let home =
            std::env::temp_dir().join(format!("pij-unisphere-compact-{}", std::process::id()));
        let project = home.join("projects").join("proj");
        std::fs::create_dir_all(&project).unwrap();
        let usage = r#"{"input_tokens":1,"output_tokens":2,"cache_read_input_tokens":499999,"cache_creation_input_tokens":0}"#;
        let compaction = r#"{"uuid":"c1","parentUuid":"a1","sessionId":"s-1","type":"system","subtype":"compact_boundary","timestamp":"2026-09-08T00:05:00Z","compactMetadata":{"trigger":"auto","preTokens":500000,"postTokens":8260}}"#;
        std::fs::write(
            project.join("s-1.jsonl"),
            record("u1", "null", "user", "2026-09-08T00:00:00Z", "")
                + &record("a1", "\"u1\"", "assistant", "2026-09-08T00:00:01Z", usage)
                + compaction
                + "\n",
        )
        .unwrap();
        let source = UnisphereSessionStatus::new(vec![home.clone()]);
        let reply = source
            .status(&SessionTarget {
                seat: SeatId("pij-compacted".into()),
                harness: Harness::Claude,
                session: "s-1".into(),
            })
            .await
            .expect("status");
        let SessionStatusReply::Status(status) = reply else {
            panic!("expected facts, got {reply:?}");
        };
        assert_eq!(status.context_used_tokens.value(), Some(&8_260));
        std::fs::remove_dir_all(&home).unwrap();
    }

    /// An OMP seat is read from `~/.omp/agent/sessions/<project>/<ts>_<id>.jsonl`,
    /// located by its session id alone.
    #[tokio::test]
    async fn an_omp_seat_is_read_from_its_session_file() {
        let sessions =
            std::env::temp_dir().join(format!("pij-unisphere-omp-{}", std::process::id()));
        let project = sessions.join("--work-demo--");
        std::fs::create_dir_all(&project).unwrap();
        let lines = [
            r#"{"type":"session","version":3,"id":"sess-omp","timestamp":"2026-09-08T00:00:00.000Z","cwd":"/work/demo"}"#,
            r#"{"type":"message","id":"e1","parentId":null,"timestamp":"2026-09-08T00:00:01.000Z","message":{"role":"user","content":[{"type":"text","text":"hi"}],"timestamp":1788825601000}}"#,
            r#"{"type":"message","id":"e2","parentId":"e1","timestamp":"2026-09-08T00:00:02.000Z","message":{"role":"assistant","api":"anthropic-messages","provider":"anthropic","model":"claude-opus-5-5","stopReason":"stop","usage":{"input":10,"output":5,"cacheRead":400,"cacheWrite":100,"totalTokens":515,"cttl":{"ephemeral5m":100}},"content":[{"type":"text","text":"hello"}],"timestamp":1788825602000}}"#,
        ];
        std::fs::write(
            project.join("2026-09-08T00-00-00-000Z_sess-omp.jsonl"),
            lines.join("\n") + "\n",
        )
        .unwrap();
        let source = UnisphereSessionStatus::with_roots(SessionRoots {
            omp_sessions: Some(sessions.clone()),
            ..SessionRoots::default()
        });
        let reply = source
            .status(&SessionTarget {
                seat: SeatId("pij-omp".into()),
                harness: Harness::Omp,
                session: "sess-omp".into(),
            })
            .await
            .expect("status");
        let SessionStatusReply::Status(status) = reply else {
            panic!("expected OMP facts, got {reply:?}");
        };
        assert!(status.context_used_tokens.value().is_some(), "{status:?}");
        assert_eq!(status.last_call_at_ms.value(), Some(&1_788_825_602_000));
        std::fs::remove_dir_all(&sessions).unwrap();
    }

    /// A Copilot seat is read from `~/.copilot/session-state/<id>/events.jsonl`.
    /// Copilot persists no per-call usage, so its context is unknown, and the
    /// cold-wake guard allows it.
    #[tokio::test]
    async fn a_copilot_seat_is_read_with_its_context_unknown() {
        let sessions =
            std::env::temp_dir().join(format!("pij-unisphere-copilot-{}", std::process::id()));
        let session = sessions.join("sess-cp");
        std::fs::create_dir_all(&session).unwrap();
        let lines = [
            r#"{"type":"session.start","id":"e0","parentId":null,"timestamp":"2026-09-08T00:00:00.000Z","data":{"sessionId":"sess-cp","version":1,"producer":"copilot-agent","copilotVersion":"0.0.0","startTime":"2026-09-08T00:00:00.000Z","selectedModel":"claude-opus-5.5","context":{"cwd":"/work/demo"}}}"#,
            r#"{"type":"user.message","id":"e1","parentId":"e0","timestamp":"2026-09-08T00:00:01.000Z","data":{"content":"hi","delivery":"idle","interactionId":"int-1"}}"#,
            r#"{"type":"assistant.message","id":"e2","parentId":"e1","timestamp":"2026-09-08T00:00:02.000Z","data":{"messageId":"m1","apiCallId":"api-1","requestId":"r1","model":"claude-opus-5.5","content":"hello","outputTokens":5,"toolRequests":[]}}"#,
        ];
        std::fs::write(session.join("events.jsonl"), lines.join("\n") + "\n").unwrap();
        let source = UnisphereSessionStatus::with_roots(SessionRoots {
            copilot_sessions: Some(sessions.clone()),
            ..SessionRoots::default()
        });
        let reply = source
            .status(&SessionTarget {
                seat: SeatId("pij-copilot".into()),
                harness: Harness::Copilot,
                session: "sess-cp".into(),
            })
            .await
            .expect("status");
        let SessionStatusReply::Status(status) = reply else {
            panic!("expected Copilot facts, got {reply:?}");
        };
        assert_eq!(status.context_used_tokens, Fact::Unknown, "{status:?}");
        std::fs::remove_dir_all(&sessions).unwrap();
    }

    #[tokio::test]
    async fn harnesses_the_sdk_cannot_read_are_unsupported() {
        let source = UnisphereSessionStatus::new(Vec::new());
        for harness in [Harness::Pi] {
            let reply = source
                .status(&SessionTarget {
                    seat: SeatId("pij-x".into()),
                    harness,
                    session: "s".into(),
                })
                .await
                .expect("status");
            assert_eq!(reply, SessionStatusReply::Unsupported, "{harness:?}");
        }
    }

    /// The real JSONL loader, counting its stats and its walks.
    #[derive(Default)]
    struct Counting {
        stats: std::sync::atomic::AtomicUsize,
        walks: std::sync::atomic::AtomicUsize,
    }

    impl Counting {
        fn stats(&self) -> usize {
            self.stats.load(std::sync::atomic::Ordering::SeqCst)
        }
        fn walks(&self) -> usize {
            self.walks.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    impl unisphere_core::prep::PrepLoader for Counting {
        fn kind(&self) -> unisphere_core::prep::PrepSourceKind {
            unisphere_loader_jsonl::FileSessionLoader.kind()
        }
        fn discover(
            &self,
            root: &std::path::Path,
            accept: &dyn Fn(&str) -> bool,
            previous: &unisphere_core::prep::PrepDirIndex,
        ) -> std::result::Result<unisphere_core::prep::PrepDiscovery, unisphere_core::PipelineError>
        {
            self.walks.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            unisphere_loader_jsonl::FileSessionLoader.discover(root, accept, previous)
        }
        fn stat(
            &self,
            root: &std::path::Path,
            path: &std::path::Path,
        ) -> std::result::Result<unisphere_core::prep::PrepSourceStat, unisphere_core::PipelineError>
        {
            self.stats.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            unisphere_loader_jsonl::FileSessionLoader.stat(root, path)
        }
        fn read(
            &self,
            stat: &unisphere_core::prep::PrepSourceStat,
            from: Option<&unisphere_core::ReadCursor>,
            limits: unisphere_core::prep::PrepReadLimits,
        ) -> std::result::Result<unisphere_core::prep::PrepBatch, unisphere_core::PipelineError>
        {
            unisphere_loader_jsonl::FileSessionLoader.read(stat, from, limits)
        }
        fn anchor(
            &self,
            stat: &unisphere_core::prep::PrepSourceStat,
            offset: u64,
        ) -> std::result::Result<String, unisphere_core::PipelineError> {
            unisphere_loader_jsonl::FileSessionLoader.anchor(stat, offset)
        }
        fn record_at(
            &self,
            path: &std::path::Path,
            address: &unisphere_core::prep::NativeAddress,
            max_bytes: usize,
        ) -> std::result::Result<Vec<u8>, unisphere_core::PipelineError> {
            unisphere_loader_jsonl::FileSessionLoader.record_at(path, address, max_bytes)
        }
    }

    /// A source whose every binding reads through `loader`.
    fn counted(
        loader: &Arc<Counting>,
        roots: SessionRoots,
        ttl: Duration,
    ) -> UnisphereSessionStatus {
        let bindings = bindings()
            .into_iter()
            .map(|binding| PrepBinding {
                fold: binding.fold,
                loader: Arc::clone(loader) as Arc<dyn unisphere_core::prep::PrepLoader>,
            })
            .collect();
        UnisphereSessionStatus::from_parts(bindings, roots, ttl)
    }

    /// A fresh scratch directory for one test.
    fn scratch(name: &str) -> PathBuf {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "pij-unisphere-{name}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn copilot_transcript(dir: &std::path::Path, session: &str) {
        std::fs::create_dir_all(dir).unwrap();
        let start = format!(
            r#"{{"type":"session.start","id":"e0","parentId":null,"timestamp":"2026-09-08T00:00:00.000Z","data":{{"sessionId":"{session}","version":1,"producer":"copilot-agent","copilotVersion":"0.0.0","startTime":"2026-09-08T00:00:00.000Z","selectedModel":"claude-opus-5.5","context":{{"cwd":"/work/demo"}}}}}}"#
        );
        std::fs::write(dir.join("events.jsonl"), start + "\n").unwrap();
    }

    fn copilot(seat: &str, session: &str) -> SessionTarget {
        SessionTarget {
            seat: SeatId(seat.into()),
            harness: Harness::Copilot,
            session: session.into(),
        }
    }

    /// F209: a Copilot seat's transcript is `<root>/<id>/events.jsonl` and
    /// nowhere else. One copied anywhere deeper is reachable only by a walk
    /// of every session, so a seat that has only that one is not found.
    #[tokio::test]
    async fn a_missing_copilot_transcript_is_not_found_by_walking_every_session() {
        let sessions = scratch("copilot-walk");
        copilot_transcript(&sessions.join("nested").join("sess-gone"), "sess-gone");
        let source = UnisphereSessionStatus::with_roots(SessionRoots {
            copilot_sessions: Some(sessions.clone()),
            ..SessionRoots::default()
        });
        let reply = source
            .status(&copilot("pij-gone", "sess-gone"))
            .await
            .expect("status");
        assert!(
            matches!(reply, SessionStatusReply::NotFound { .. }),
            "found by a walk: {reply:?}"
        );
        std::fs::remove_dir_all(&sessions).unwrap();
    }

    /// F209: a missing Copilot transcript costs one stat and no walk; the
    /// miss is kept for its lifetime, so concurrent and repeated reads look
    /// no further, and a transcript that appears is found once it expires.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_copilot_miss_is_one_stat_per_lifetime_shared_by_concurrent_reads() {
        let sessions = scratch("copilot-miss");
        let loader = Arc::new(Counting::default());
        let ttl = Duration::from_millis(500);
        let source = Arc::new(counted(
            &loader,
            SessionRoots {
                copilot_sessions: Some(sessions.clone()),
                ..SessionRoots::default()
            },
            ttl,
        ));
        let target = copilot("pij-missing", "sess-late");
        let reads: Vec<_> = (0..8)
            .map(|_| {
                let (source, target) = (Arc::clone(&source), target.clone());
                tokio::spawn(async move { source.status(&target).await.expect("status") })
            })
            .collect();
        for read in reads {
            let reply = read.await.unwrap();
            assert!(
                matches!(reply, SessionStatusReply::NotFound { .. }),
                "{reply:?}"
            );
        }
        assert_eq!(
            (loader.stats(), loader.walks()),
            (1, 0),
            "eight reads, one stat"
        );

        copilot_transcript(&sessions.join("sess-late"), "sess-late");
        let kept = source.status(&target).await.expect("status");
        assert!(
            matches!(kept, SessionStatusReply::NotFound { .. }),
            "{kept:?}"
        );
        assert_eq!(loader.stats(), 1, "within the lifetime, no look");

        tokio::time::sleep(ttl + Duration::from_millis(100)).await;
        let found = source.status(&target).await.expect("status");
        assert!(matches!(found, SessionStatusReply::Status(_)), "{found:?}");
        assert_eq!(loader.walks(), 0);
        std::fs::remove_dir_all(&sessions).unwrap();
    }

    /// F209: a Claude transcript's folder is named for the launch directory,
    /// so a miss still walks the projects: once per lifetime, not per read.
    #[tokio::test]
    async fn a_claude_miss_walks_once_per_lifetime() {
        let home = scratch("claude-miss");
        std::fs::create_dir_all(home.join("projects")).unwrap();
        let loader = Arc::new(Counting::default());
        let ttl = Duration::from_millis(300);
        let source = counted(
            &loader,
            SessionRoots {
                claude_homes: vec![home.clone()],
                ..SessionRoots::default()
            },
            ttl,
        );
        let target = SessionTarget {
            seat: SeatId("pij-claude-missing".into()),
            harness: Harness::Claude,
            session: "s-1".into(),
        };
        for _ in 0..5 {
            let reply = source.status(&target).await.expect("status");
            assert!(
                matches!(reply, SessionStatusReply::NotFound { .. }),
                "{reply:?}"
            );
        }
        assert_eq!(loader.walks(), 1, "five reads, one walk");

        let project = home.join("projects").join("proj");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(
            project.join("s-1.jsonl"),
            record("u1", "null", "user", "2026-09-08T00:00:00Z", ""),
        )
        .unwrap();
        tokio::time::sleep(ttl + Duration::from_millis(100)).await;
        let found = source.status(&target).await.expect("status");
        assert!(matches!(found, SessionStatusReply::Status(_)), "{found:?}");
        assert_eq!(loader.walks(), 2);
        std::fs::remove_dir_all(&home).unwrap();
    }

    /// R249-001: a kept miss answers only the target it was kept for. A seat
    /// whose session changes is looked up at once, and a hit clears the old
    /// miss, so going back to the old session looks again rather than
    /// answering a miss from before the hit.
    #[tokio::test]
    async fn a_kept_miss_answers_only_its_own_target_and_a_hit_clears_it() {
        let sessions = scratch("copilot-target");
        let loader = Arc::new(Counting::default());
        let source = counted(
            &loader,
            SessionRoots {
                copilot_sessions: Some(sessions.clone()),
                ..SessionRoots::default()
            },
            Duration::from_secs(60),
        );
        let old = copilot("pij-moving", "sess-old");
        let new = copilot("pij-moving", "sess-new");
        copilot_transcript(&sessions.join("sess-new"), "sess-new");

        let missed = source.status(&old).await.expect("status");
        assert!(
            matches!(missed, SessionStatusReply::NotFound { .. }),
            "{missed:?}"
        );
        assert_eq!(loader.stats(), 1);

        let found = source.status(&new).await.expect("status");
        assert!(
            matches!(found, SessionStatusReply::Status(_)),
            "a new target is looked up: {found:?}"
        );
        let after_hit = loader.stats();
        assert!(after_hit > 1, "the new session was stat'd");

        let again = source.status(&old).await.expect("status");
        assert!(
            matches!(again, SessionStatusReply::NotFound { .. }),
            "{again:?}"
        );
        assert!(
            loader.stats() > after_hit,
            "the hit cleared the old miss, so the old session is looked up again"
        );
        assert_eq!(loader.walks(), 0);
        std::fs::remove_dir_all(&sessions).unwrap();
    }
}
