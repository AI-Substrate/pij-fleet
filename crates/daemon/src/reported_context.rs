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
//! report names are in every roster, so they prove nothing. Registration
//! issues a random per-incarnation grant ([`ReportedContext::grant`]),
//! returned only in that registration's own response; a report must present
//! it ([`ReportedContext::authorized`]). A new process incarnation gets a new
//! grant and the old one stops working.
//!
//! **Generations** (review F002). Every activity post bumps the seat's
//! generation ([`ReportedContext::touch`]), so the PA watchdog re-reads an idle
//! seat's size only when something happened since it last read it, including
//! a whole turn that fell between two rounds.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use pij_core::error::{PijError, Result};
use pij_core::model::{ProcIdentity, SeatId};
use pij_core::ports::SessionStatusPort;
use pij_core::session_status::{Fact, SessionStatusReply, SessionTarget};

/// One seat's latest report.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Report {
    session: Option<String>,
    tokens: u64,
    window: Option<u64>,
}

/// One seat's report grant: the token and the incarnation it was issued to.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Grant {
    token: String,
    proc: Option<ProcIdentity>,
}

#[derive(Debug, Default)]
struct State {
    reports: HashMap<SeatId, Report>,
    grants: HashMap<SeatId, Grant>,
    generations: HashMap<SeatId, u64>,
}

/// The latest context size each seat reported, who may report it, and how
/// often each seat's activity changed.
#[derive(Debug, Default)]
pub struct ReportedContext {
    state: Mutex<State>,
}

impl ReportedContext {
    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().expect("reported context state")
    }

    /// The report grant for `seat`'s incarnation `proc`: the standing one when
    /// the incarnation is unchanged (a heartbeat re-registration), else a new
    /// random token that replaces it.
    ///
    /// # Errors
    /// The system random source failed.
    pub fn grant(&self, seat: &SeatId, proc: Option<ProcIdentity>) -> Result<String> {
        let mut state = self.lock();
        if let Some(grant) = state.grants.get(seat).filter(|grant| grant.proc == proc) {
            return Ok(grant.token.clone());
        }
        let mut secret = [0_u8; 32];
        getrandom::fill(&mut secret).map_err(|error| PijError::Adapter {
            adapter: "daemon/reported-context".into(),
            message: format!("report grant: {error}"),
        })?;
        let token = secret
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        state.grants.insert(
            seat.clone(),
            Grant {
                token: token.clone(),
                proc,
            },
        );
        Ok(token)
    }

    /// Does `presented` match `seat`'s grant for its current incarnation
    /// `proc`? No grant, another incarnation's grant, or a different token is
    /// `false`.
    pub fn authorized(&self, seat: &SeatId, proc: Option<ProcIdentity>, presented: &str) -> bool {
        self.lock().grants.get(seat).is_some_and(|grant| {
            grant.proc == proc && crate::http::auth::constant_time_eq(&grant.token, presented)
        })
    }

    /// Record `seat`'s latest size, for the session it reported from. The
    /// caller has checked [`Self::authorized`].
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

    /// Review S153-001: a grant is per incarnation; only its own token passes.
    #[test]
    fn a_grant_is_per_incarnation_and_only_its_token_passes() {
        let reports = ReportedContext::default();
        let seat: SeatId = "pij-c".into();
        let first = Some(ProcIdentity {
            pid: 10,
            proc_start: 1,
        });
        let token = reports.grant(&seat, first).unwrap();
        assert_eq!(
            reports.grant(&seat, first).unwrap(),
            token,
            "a heartbeat keeps it"
        );
        assert!(reports.authorized(&seat, first, &token));
        assert!(!reports.authorized(&seat, first, "forged"));
        assert!(!reports.authorized(&"pij-other".into(), first, &token));
        let next = Some(ProcIdentity {
            pid: 11,
            proc_start: 2,
        });
        assert!(
            !reports.authorized(&seat, next, &token),
            "the seat was relaunched"
        );
        let renewed = reports.grant(&seat, next).unwrap();
        assert_ne!(renewed, token);
        assert!(
            !reports.authorized(&seat, first, &token),
            "the old grant is gone"
        );
        assert!(reports.authorized(&seat, next, &renewed));
    }
}
