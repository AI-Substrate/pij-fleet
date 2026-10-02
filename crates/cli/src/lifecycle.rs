//! Native lifecycle leaves forward to the daemon's single semantic parser.

use pij_core::model::Envelope;
use serde_json::{Value, json};

use crate::{CallerContext, DaemonClient};

impl DaemonClient {
    /// Close a seat using daemon-derived ownership; retain the complete envelope.
    pub async fn close(&self, args: &[String], caller: &CallerContext) -> Envelope<Value> {
        let argv: Vec<&str> = std::iter::once("close")
            .chain(args.iter().map(String::as_str))
            .collect();
        self.post(
            "pij close",
            "/v1/close",
            &json!({"argv":argv,"caller":caller}),
        )
        .await
    }

    /// Reconcile stale records without interpreting liveness or flags client-side.
    pub async fn reap(&self, args: &[String], caller: &CallerContext) -> Envelope<Value> {
        let argv: Vec<&str> = std::iter::once("reap")
            .chain(args.iter().map(String::as_str))
            .collect();
        self.post(
            "pij reap",
            "/v1/reap",
            &json!({"argv":argv,"caller":caller}),
        )
        .await
    }
}
