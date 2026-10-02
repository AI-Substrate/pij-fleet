use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use pij_core::error::Result;
use pij_core::model::{Harness, SeatDescriptor};
use pij_core::ports::{LivenessPort, Registry, SeatFilter};
use serde::Deserialize;

use crate::events::EventBus;
use crate::http::Registration;
use crate::http::role::RoleService;
use crate::registration::RegistrationService;

const ADAPTER: &str = "daemon/claude-bind";

pub(crate) struct ClaudeBinder {
    home: Option<PathBuf>,
    registry: Arc<dyn Registry>,
    liveness: Arc<dyn LivenessPort>,
    utc_offset_minutes: fn() -> Result<i32>,
    registration: RegistrationService,
}

impl ClaudeBinder {
    pub(crate) fn new(
        registry: Arc<dyn Registry>,
        liveness: Arc<dyn LivenessPort>,
        event_bus: Arc<EventBus>,
        roles: Arc<RoleService>,
    ) -> Self {
        let home = env::var_os("HOME").map(PathBuf::from);
        Self::from_home(
            home,
            registry,
            liveness,
            event_bus,
            pij_harnesses::claude_homes(),
            pij_harnesses::proc::local_utc_offset_minutes,
            roles,
        )
    }

    fn from_home(
        home: Option<PathBuf>,
        registry: Arc<dyn Registry>,
        liveness: Arc<dyn LivenessPort>,
        event_bus: Arc<EventBus>,
        claude_homes: Vec<PathBuf>,
        utc_offset_minutes: fn() -> Result<i32>,
        roles: Arc<RoleService>,
    ) -> Self {
        let registration = RegistrationService::new(
            Arc::clone(&registry),
            Arc::clone(&liveness),
            Arc::clone(&event_bus),
            claude_homes,
            roles,
        );
        Self {
            home,
            registry,
            liveness,
            utc_offset_minutes,
            registration,
        }
    }

    pub(crate) async fn bind_once(&self) -> Result<usize> {
        let seats = self.registry.list(SeatFilter::default()).await?;
        let mut bound = 0;
        for seat in seats.into_iter().filter(is_pending_spawned_claude) {
            match self.bind_seat(&seat).await {
                Ok(true) => bound += 1,
                Ok(false) => {}
                Err(error) => eprintln!("pij-rs claude binder: seat {}: {error}", seat.id),
            }
        }
        Ok(bound)
    }

    async fn bind_seat(&self, seat: &SeatDescriptor) -> Result<bool> {
        let Some(record) = self.matching_record(seat) else {
            return Ok(false);
        };
        let Some(observed_start) = self.liveness.proc_start(record.pid).await? else {
            return Ok(false);
        };
        let utc_offset_minutes = (self.utc_offset_minutes)()?;
        if !pij_harnesses::proc::process_start_matches_utc_record(
            &record.proc_start,
            observed_start,
            utc_offset_minutes,
        )? {
            return Ok(false);
        }
        let claim = Registration {
            supersedes: None,
            id: seat.id.to_string(),
            harness: Harness::Claude.as_str().to_string(),
            folder: seat.folder.clone(),
            extension_build: None,
            extension_path: None,
            pane: seat.pane.clone(),
            pid: Some(record.pid),
            proc_start: Some(observed_start),
            spawn_id: seat.spawn_id.clone(),
            model: seat.model.clone(),
            actual_model: None,
            actual_model_observed: false,
            provider: seat.provider.clone(),
            effort: seat.effort.clone(),
            parent: seat.parent.clone(),
            role: None,
            relay: seat.relay,
        };
        match self.registration.register(claim).await {
            Ok(_) => Ok(true),
            Err(error) => Err(pij_core::error::PijError::Adapter {
                adapter: ADAPTER.to_string(),
                message: error.to_string(),
            }),
        }
    }

    fn matching_record(&self, seat: &SeatDescriptor) -> Option<ClaudeSessionRecord> {
        let home = self.home.as_deref()?;
        let pane = seat.pane.as_deref()?;
        let suffix = format!(".{pane}");

        // Claude 2.1.251 publishes this record before its first user turn, while
        // the cwd-scoped transcript does not exist until that turn. Requiring a
        // fresh JSONL would therefore deadlock the first queued delivery. The
        // session record is the stronger join: trusted cwd + exact tmux pane +
        // live pid. The folder-trust modal publishes no record, so absence stays
        // the honest not-ready signal. TS parity deliberately reads ~/.claude;
        // a separate ~/.claude-alt home exists but is not this launcher's home.
        let sessions_dir = home.join(".claude/sessions");
        let mut records = json_files(&sessions_dir)
            .into_iter()
            .filter_map(|path| read_json::<ClaudeSessionRecord>(&path))
            .filter(|record| {
                record.cwd == seat.folder
                    && record
                        .tmux
                        .as_deref()
                        .is_some_and(|tmux| tmux.ends_with(&suffix))
            });
        let record = records.next()?;
        records.next().is_none().then_some(record)
    }
}

fn is_pending_spawned_claude(seat: &SeatDescriptor) -> bool {
    seat.harness == Harness::Claude
        && seat.proc.is_none()
        && seat.pane.is_some()
        && seat.spawn_id.is_some()
        && seat.tombstoned_at.is_none()
}

fn json_files(dir: &Path) -> Vec<PathBuf> {
    files_with_extension(dir, "json")
}

fn files_with_extension(dir: &Path, extension: &str) -> Vec<PathBuf> {
    let mut paths = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(std::result::Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == extension))
        .collect::<Vec<_>>();
    paths.sort();
    paths
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Option<T> {
    serde_json::from_slice(&fs::read(path).ok()?).ok()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClaudeSessionRecord {
    pid: u32,
    proc_start: String,
    cwd: String,
    tmux: Option<String>,
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::sync::Arc;

    use pij_core::model::{Harness, SeatDescriptor};
    use pij_core::ports::Registry;
    use pij_testkit::fakes::{FakeLiveness, FakeRegistry, FakeSpine};

    use super::ClaudeBinder;
    use crate::events::EventBus;

    fn zero_utc_offset() -> pij_core::error::Result<i32> {
        Ok(0)
    }

    fn event_bus() -> Arc<EventBus> {
        Arc::new(EventBus::new(Arc::new(FakeSpine::new()), 8).expect("event bus"))
    }

    async fn role_service(
        registry: Arc<dyn Registry>,
        bus: Arc<EventBus>,
    ) -> Arc<crate::http::role::RoleService> {
        let pool = pij_store::open("").await.expect("isolated role storage");
        Arc::new(crate::http::role::RoleService::new(
            registry,
            pij_store::SqliteOrchestration::new(pool),
            bus,
        ))
    }

    struct TestHome(PathBuf);

    impl TestHome {
        fn new() -> Self {
            Self(pij_testkit::fresh_dir("pij-claude-bind"))
        }

        fn sessions_dir(&self) -> PathBuf {
            self.0.join(".claude/sessions")
        }

        fn write_session(
            &self,
            session_id: &str,
            cwd: &str,
            pane: &str,
            pid: u32,
            proc_start: &str,
        ) {
            let sessions = self.sessions_dir();
            fs::create_dir_all(&sessions).unwrap();
            fs::write(
                sessions.join(format!("{pid}.json")),
                serde_json::json!({
                    "pid": pid,
                    "sessionId": session_id,
                    "procStart": proc_start,
                    "cwd": cwd,
                    "tmux": format!("sanitized:@1.{pane}")
                })
                .to_string(),
            )
            .unwrap();
        }
    }

    impl Drop for TestHome {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn pending(pane: &str) -> SeatDescriptor {
        let mut seat = SeatDescriptor::new("pij-claude", Harness::Claude, "/abs/worktree");
        seat.pane = Some(pane.to_string());
        seat.spawn_id = Some("spawn-claude".to_string());
        seat.model = Some("claude-opus-5".to_string());
        seat.cross_session_inbound_accept = Some(true);
        seat
    }

    #[tokio::test]
    async fn session_record_and_exact_pane_bind_before_a_transcript_exists() {
        let home = TestHome::new();
        home.write_session(
            "native-1",
            "/abs/worktree",
            "%42",
            4242,
            "Mon Aug 31 10:15:00 2026",
        );
        let registry = Arc::new(FakeRegistry::new());
        registry.put(pending("%42")).await.unwrap();
        let liveness = Arc::new(
            FakeLiveness::new().with_proc(pij_core::model::ProcIdentity {
                pid: 4242,
                proc_start: 20260831101500,
            }),
        );
        let bus = event_bus();
        let roles = role_service(registry.clone(), bus.clone()).await;
        let binder = ClaudeBinder::from_home(
            Some(home.0.clone()),
            registry.clone(),
            liveness,
            bus,
            Vec::new(),
            zero_utc_offset,
            roles,
        );

        assert_eq!(binder.bind_once().await.unwrap(), 1);
        let bound = registry.get(&"pij-claude".into()).await.unwrap().unwrap();
        assert_eq!(bound.proc.unwrap().pid, 4242);
        assert_eq!(bound.cross_session_inbound_accept, Some(true));
        assert_eq!(bound.model.as_deref(), Some("claude-opus-5"));
    }

    #[tokio::test]
    async fn wrong_pane_record_cannot_bind_the_seat() {
        let home = TestHome::new();
        home.write_session(
            "native-wrong",
            "/abs/worktree",
            "%99",
            4242,
            "Mon Aug 31 10:15:00 2026",
        );
        let registry = Arc::new(FakeRegistry::new());
        registry.put(pending("%42")).await.unwrap();
        let liveness = Arc::new(
            FakeLiveness::new().with_proc(pij_core::model::ProcIdentity {
                pid: 4242,
                proc_start: 20260831101500,
            }),
        );
        let bus = event_bus();
        let roles = role_service(registry.clone(), bus.clone()).await;
        let binder = ClaudeBinder::from_home(
            Some(home.0.clone()),
            registry.clone(),
            liveness,
            bus,
            Vec::new(),
            zero_utc_offset,
            roles,
        );

        assert_eq!(binder.bind_once().await.unwrap(), 0);
        assert!(
            registry
                .get(&"pij-claude".into())
                .await
                .unwrap()
                .unwrap()
                .proc
                .is_none()
        );
    }

    #[tokio::test]
    async fn live_recycled_pid_with_a_different_start_time_cannot_bind() {
        let home = TestHome::new();
        home.write_session(
            "native-stale",
            "/abs/worktree",
            "%42",
            4242,
            "Mon Aug 31 10:14:59 2026",
        );
        let registry = Arc::new(FakeRegistry::new());
        registry.put(pending("%42")).await.unwrap();
        let liveness = Arc::new(
            FakeLiveness::new().with_proc(pij_core::model::ProcIdentity {
                pid: 4242,
                proc_start: 20260831101500,
            }),
        );
        let bus = event_bus();
        let roles = role_service(registry.clone(), bus.clone()).await;
        let binder = ClaudeBinder::from_home(
            Some(home.0.clone()),
            registry.clone(),
            liveness,
            bus,
            Vec::new(),
            zero_utc_offset,
            roles,
        );

        assert_eq!(binder.bind_once().await.unwrap(), 0);
        assert!(
            registry
                .get(&"pij-claude".into())
                .await
                .unwrap()
                .unwrap()
                .proc
                .is_none()
        );
    }
}
