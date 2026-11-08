//! Context sizes a native extension reports at its turn boundaries (plan 217
//! AC6).
//!
//! Copilot's `session.usage_info` event, which carries the session's current
//! token count, is ephemeral: it never reaches `events.jsonl`, so the
//! transcript reader reports Copilot's `contextUsedTokens` as unknown. The
//! Copilot extension sees the event live and sends the latest count on the
//! `seat.activity` post it already makes at each turn boundary
//! (`/v1/activity`); [`WithReportedContext`] fills an unknown fact from it.
//! A fact the transcript knows always wins. In memory only: after a daemon
//! restart a seat's size is unknown until its next turn boundary. No launch,
//! no poll: the report rides a request the seat already makes.
//!
//! **Only the seat itself may report** (review S153-001). A size moves the
//! cold-wake guard and the prime's large-seat FYI, and the pane and session a
//! report names are in every roster, so they prove nothing. A report must
//! present the seat's capability (plan 223, which folded the old context
//! grant into it): issued only by the secured issuer to the registration that
//! proved itself with its spawn's nonce or the current capability, never from
//! public facts (F88 row 2, closed), and checked against the digest of the
//! seat's current incarnation ([`crate::capability`]). A new process
//! incarnation, or a tombstone, revokes it.
//!
//! **Generations** (review F002). Every activity post bumps the seat's
//! generation ([`ReportedContext::touch`]), so the PA watchdog re-reads an idle
//! seat's size only when something happened since it last read it, including
//! a whole turn that fell between two rounds.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use pij_core::error::{PijError, Result};
use pij_core::model::{ProcIdentity, SeatId, SystemState};

use crate::capability::Capabilities;
use pij_core::ports::{Registry, SessionStatusPort};
use pij_core::session_status::{Fact, SessionStatusReply, SessionTarget};

/// One seat's latest report.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Report {
    session: Option<String>,
    tokens: u64,
    window: Option<u64>,
}

#[derive(Debug, Default)]
struct State {
    reports: HashMap<SeatId, Report>,
    generations: HashMap<SeatId, u64>,
}

/// One authorized size report, as `/v1/activity` carries it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContextReport {
    /// The session it was reported from.
    pub session: Option<String>,
    /// The session's context size, in tokens.
    pub tokens: u64,
    /// The model's context window, when the runtime knows it.
    pub window: Option<u64>,
}

/// The latest context size each seat reported, who may report it, and how
/// often each seat's activity changed.
#[derive(Debug, Default)]
pub struct ReportedContext {
    state: Mutex<State>,
    /// One critical section for a native registration (which may replace a
    /// seat's process, revoking its capability) and for applying a size
    /// report (review S153-002). Both are rare, short and per turn boundary
    /// at most.
    gate: Arc<tokio::sync::Mutex<()>>,
}

impl ReportedContext {
    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().expect("reported context state")
    }

    /// Run `work` (a native registration: its registry write and its
    /// capability resync) inside the critical section, as an owned task. Once admitted
    /// it holds the section until `work` has finished, even if the caller
    /// stops waiting (review F004): a registry write that outlives a cancelled
    /// request can never let a size report in before it commits.
    ///
    /// # Errors
    /// The owned task failed (panicked or was aborted at shutdown).
    pub async fn exclusively<T: Send + 'static>(
        &self,
        work: impl std::future::Future<Output = T> + Send + 'static,
    ) -> Result<T> {
        let held = Arc::clone(&self.gate).lock_owned().await;
        tokio::spawn(async move {
            let _held = held;
            work.await
        })
        .await
        .map_err(|error| PijError::Adapter {
            adapter: "daemon/reported-context".into(),
            message: format!("registration task failed: {error}"),
        })
    }

    /// Apply an authorized report: `seat`'s activity and size, only if, at
    /// this moment and inside the critical section, the seat is live, still
    /// the incarnation `proc` the request was checked against, and `token` is
    /// still that incarnation's capability. `None` means the request's
    /// incarnation was replaced (or its capability revoked) since it was
    /// checked: nothing is changed. `Some(changed)` is whether the stored
    /// state changed.
    ///
    /// # Errors
    /// Registry failures.
    #[allow(clippy::too_many_arguments)]
    pub async fn apply(
        &self,
        registry: &dyn Registry,
        capabilities: &Capabilities,
        seat: &SeatId,
        proc: Option<ProcIdentity>,
        token: &str,
        observed: SystemState,
        report: ContextReport,
    ) -> Result<Option<bool>> {
        let _gate = self.gate.lock().await;
        let Some(current) = registry
            .get(seat)
            .await?
            .filter(|row| row.tombstoned_at.is_none() && row.proc == proc)
        else {
            return Ok(None);
        };
        if !crate::capability::present(registry, capabilities, &current, token)
            .await?
            .valid()
        {
            return Ok(None);
        }
        let changed = registry.set_activity(seat, observed, None).await?.is_some();
        self.record(seat, report.session, report.tokens, report.window);
        Ok(Some(changed))
    }

    /// Record `seat`'s latest size, for the session it reported from. The
    /// caller has checked the seat's capability.
    pub fn record(&self, seat: &SeatId, session: Option<String>, tokens: u64, window: Option<u64>) {
        let mut state = self.lock();
        state.reports.insert(
            seat.clone(),
            Report {
                session,
                tokens,
                window,
            },
        );
        *state.generations.entry(seat.clone()).or_default() += 1;
    }

    /// Note an activity post from `seat`: its size may have changed.
    pub fn touch(&self, seat: &SeatId) {
        *self.lock().generations.entry(seat.clone()).or_default() += 1;
    }

    /// How many times `seat`'s activity or size has changed since the daemon
    /// started.
    pub fn generation(&self, seat: &SeatId) -> u64 {
        self.lock().generations.get(seat).copied().unwrap_or(0)
    }

    fn get(&self, seat: &SeatId, session: &str) -> Option<Report> {
        self.lock()
            .reports
            .get(seat)
            .filter(|report| report.session.as_deref().is_none_or(|own| own == session))
            .cloned()
    }
}

/// A session-status source whose unknown context facts are filled from
/// [`ReportedContext`].
pub struct WithReportedContext {
    inner: Arc<dyn SessionStatusPort>,
    reports: Arc<ReportedContext>,
}

impl WithReportedContext {
    /// Wrap `inner`.
    pub fn new(inner: Arc<dyn SessionStatusPort>, reports: Arc<ReportedContext>) -> Self {
        Self { inner, reports }
    }
}

#[async_trait]
impl SessionStatusPort for WithReportedContext {
    async fn status(&self, target: &SessionTarget) -> Result<SessionStatusReply> {
        let reply = self.inner.status(target).await?;
        let SessionStatusReply::Status(mut status) = reply else {
            return Ok(reply);
        };
        if let Some(report) = self.reports.get(&target.seat, &target.session) {
            if status.context_used_tokens == Fact::Unknown {
                status.context_used_tokens = Fact::native(report.tokens);
            }
            if let (Fact::Unknown, Some(window)) = (&status.context_window_tokens, report.window) {
                status.context_window_tokens = Fact::native(window);
            }
        }
        Ok(SessionStatusReply::Status(status))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pij_core::model::Harness;
    use pij_core::session_status::SeatStatus;
    use pij_testkit::fakes::FakeSessionStatus;

    fn target(session: &str) -> SessionTarget {
        SessionTarget {
            seat: "pij-c".into(),
            harness: Harness::Copilot,
            session: session.into(),
        }
    }

    fn tokens(reply: SessionStatusReply) -> (Fact<u64>, Fact<u64>) {
        match reply {
            SessionStatusReply::Status(status) => {
                (status.context_used_tokens, status.context_window_tokens)
            }
            other => panic!("{other:?}"),
        }
    }

    /// Plan 217 AC6: an unknown size is filled from the seat's report for the
    /// same session; a known one, or another session's report, is not.
    #[tokio::test]
    async fn a_reported_size_fills_only_an_unknown_fact_for_its_own_session() {
        let fake = FakeSessionStatus::new()
            .with_reply("s1", SessionStatusReply::Status(SeatStatus::unknown()))
            .with_reply(
                "s2",
                SessionStatusReply::Status(SeatStatus {
                    context_used_tokens: Fact::derived(5),
                    ..SeatStatus::unknown()
                }),
            );
        let reports = Arc::new(ReportedContext::default());
        let source = WithReportedContext::new(Arc::new(fake), Arc::clone(&reports));
        assert_eq!(
            tokens(source.status(&target("s1")).await.unwrap()),
            (Fact::Unknown, Fact::Unknown)
        );
        reports.record(&"pij-c".into(), Some("s1".into()), 640_000, Some(952_000));
        assert_eq!(
            tokens(source.status(&target("s1")).await.unwrap()),
            (Fact::native(640_000), Fact::native(952_000))
        );
        reports.record(&"pij-c".into(), Some("s2".into()), 640_000, None);
        assert_eq!(
            tokens(source.status(&target("s2")).await.unwrap()).0,
            Fact::derived(5),
            "the transcript wins"
        );
        assert_eq!(
            tokens(source.status(&target("s1")).await.unwrap()).0,
            Fact::Unknown,
            "another session's report never applies"
        );
    }

    /// Review S153-002: a report applies only between registrations, never
    /// inside one that may be replacing the process or rotating the grant.
    #[tokio::test]
    async fn a_report_waits_for_a_registration_in_progress() {
        let proc = Some(ProcIdentity {
            pid: 10,
            proc_start: 1,
        });
        let mut seat = pij_core::model::SeatDescriptor::new("pij-c", Harness::Copilot, "/abs");
        seat.proc = proc;
        let registry: Arc<dyn Registry> =
            Arc::new(pij_testkit::fakes::FakeRegistry::new().with_seat(seat.clone()));
        let capabilities = Arc::new(Capabilities::default());
        let token =
            crate::capability::test_support::issue(registry.as_ref(), &capabilities, seat.clone())
                .await;
        let reports = Arc::new(ReportedContext::default());
        let registration = Arc::clone(&reports.gate).lock_owned().await;
        let report = tokio::spawn({
            let (reports, registry, capabilities, id) = (
                Arc::clone(&reports),
                Arc::clone(&registry),
                Arc::clone(&capabilities),
                seat.id,
            );
            async move {
                reports
                    .apply(
                        registry.as_ref(),
                        &capabilities,
                        &id,
                        proc,
                        &token,
                        SystemState::Working,
                        ContextReport {
                            session: None,
                            tokens: 5,
                            window: None,
                        },
                    )
                    .await
                    .unwrap()
            }
        });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(
            !report.is_finished(),
            "the report waits for the registration"
        );
        drop(registration);
        assert!(report.await.unwrap().is_some());
    }

    /// Review S153-001, now through the capability (plan 223): a report
    /// applies only with its own live incarnation's capability.
    #[tokio::test]
    async fn a_report_needs_its_own_incarnations_capability() {
        let first = Some(ProcIdentity {
            pid: 10,
            proc_start: 1,
        });
        let mut seat = pij_core::model::SeatDescriptor::new("pij-c", Harness::Copilot, "/abs");
        seat.proc = first;
        let registry: Arc<dyn Registry> =
            Arc::new(pij_testkit::fakes::FakeRegistry::new().with_seat(seat.clone()));
        let capabilities = Capabilities::default();
        let reports = ReportedContext::default();
        let token =
            crate::capability::test_support::issue(registry.as_ref(), &capabilities, seat.clone())
                .await;
        let report = |proc, token: String| {
            let (registry, capabilities, reports, id) = (
                Arc::clone(&registry),
                &capabilities,
                &reports,
                seat.id.clone(),
            );
            async move {
                reports
                    .apply(
                        registry.as_ref(),
                        capabilities,
                        &id,
                        proc,
                        &token,
                        SystemState::Working,
                        ContextReport {
                            session: None,
                            tokens: 5,
                            window: None,
                        },
                    )
                    .await
                    .unwrap()
            }
        };
        assert!(report(first, token.clone()).await.is_some());
        assert_eq!(report(first, "0".repeat(64)).await, None, "forged");
        let next = Some(ProcIdentity {
            pid: 11,
            proc_start: 2,
        });
        let mut relaunched = seat.clone();
        relaunched.proc = next;
        registry.put(relaunched.clone()).await.unwrap();
        assert_eq!(
            report(next, token.clone()).await,
            None,
            "the seat was relaunched"
        );
        assert_eq!(registry.capability(&seat.id).await.unwrap(), None);
        let renewed =
            crate::capability::test_support::issue(registry.as_ref(), &capabilities, relaunched)
                .await;
        assert_ne!(renewed, token);
        assert_eq!(report(next, token).await, None, "the old one is gone");
        assert!(report(next, renewed).await.is_some());
    }
}
