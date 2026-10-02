//! Native anomaly command: semantics remain owned by the daemon parser.
use crate::{CallerContext, DaemonClient};
use pij_core::model::Envelope;
use serde_json::{Value, json};

/// Forward the exact filter argv and caller evidence, preserving the envelope.
pub async fn anomalies(
    client: &DaemonClient,
    args: &[String],
    caller: &CallerContext,
) -> Envelope<Value> {
    let argv: Vec<&str> = std::iter::once("anomalies")
        .chain(args.iter().map(String::as_str))
        .collect();
    client
        .post(
            "pij anomalies",
            "/v1/anomalies",
            &json!({"argv":argv,"caller":caller}),
        )
        .await
}
