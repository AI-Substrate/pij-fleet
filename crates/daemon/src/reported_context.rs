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

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use pij_core::error::Result;
use pij_core::model::SeatId;
use pij_core::ports::SessionStatusPort;
use pij_core::session_status::{Fact, SessionStatusReply, SessionTarget};

/// One seat's latest report.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Report {
    session: Option<String>,
    tokens: u64,
    window: Option<u64>,
}

/// The latest context size each seat reported.
#[derive(Debug, Default)]
pub struct ReportedContext {
    reports: Mutex<HashMap<SeatId, Report>>,
}

impl ReportedContext {
    /// Record `seat`'s latest size, for the session it reported from.
    pub fn record(&self, seat: &SeatId, session: Option<String>, tokens: u64, window: Option<u64>) {
        self.reports.lock().expect("reported context map").insert(
            seat.clone(),
            Report {
                session,
                tokens,
                window,
            },
        );
    }

    fn get(&self, seat: &SeatId, session: &str) -> Option<Report> {
        self.reports
            .lock()
            .expect("reported context map")
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
}
