//! Native decision commands forward their original argv to the sole daemon parser.
use crate::{CallerContext, DaemonClient};
use pij_core::model::Envelope;
use serde_json::{Value, json};

/// Read questions without implementing independent filter semantics.
pub async fn decisions(
    client: &DaemonClient,
    args: &[String],
    caller: &CallerContext,
) -> Envelope<Value> {
    let argv: Vec<&str> = std::iter::once("decisions")
        .chain(args.iter().map(String::as_str))
        .collect();
    client
        .post(
            "pij decisions",
            "/v1/decisions",
            &json!({"argv":argv,"caller":caller}),
        )
        .await
}

/// Answer through the same identity, ownership and delivery path as the shim.
pub async fn answer(
    client: &DaemonClient,
    args: &[String],
    caller: &CallerContext,
) -> Envelope<Value> {
    let argv: Vec<&str> = std::iter::once("answer")
        .chain(args.iter().map(String::as_str))
        .collect();
    client
        .post(
            "pij answer",
            "/v1/answer",
            &json!({"argv":argv,"caller":caller}),
        )
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::extract::{Json, State};
    use axum::routing::post;
    use pij_testkit::fresh_dir;
    use std::sync::Arc;

    async fn echo(State(fixtures): State<Arc<Value>>, Json(request): Json<Value>) -> String {
        let case = fixtures["routes"]
            .as_array()
            .expect("routes")
            .iter()
            .filter(|route| route["owner"] == "u4")
            .flat_map(|route| route["cases"].as_array().expect("cases"))
            .find(|case| {
                case["request"]["argv"] == request["argv"]
                    || case["shim_request"]["argv"] == request["argv"]
            })
            .expect("canonical argv");
        let request_case = if case["request"]["argv"].is_array() {
            &case["request"]
        } else {
            &case["shim_request"]
        };
        let caller: CallerContext =
            serde_json::from_value(request_case["caller"].clone()).expect("caller");
        assert_eq!(
            request["caller"],
            serde_json::to_value(caller).expect("caller wire")
        );
        let response: Envelope<Value> =
            serde_json::from_value(case["response"].clone()).expect("response");
        serde_json::to_string(&response).expect("envelope")
    }

    #[tokio::test]
    async fn anomaly_decision_answer_commands_preserve_canonical_argv_and_envelopes() {
        let fixtures: Value = serde_json::from_str(include_str!(
            "../../testkit/fixtures/golden/api/governance-routes.json"
        ))
        .expect("contracts");
        let router = axum::Router::new()
            .route("/v1/anomalies", post(echo))
            .route("/v1/decisions", post(echo))
            .route("/v1/answer", post(echo))
            .with_state(Arc::new(fixtures.clone()));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("address");
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.expect("serve");
        });
        let dir = fresh_dir("pij-u4-cli");
        std::fs::write(dir.join("daemon.key"), "u4-cli-key").expect("fixture key");
        let client = DaemonClient::new(&dir, &addr.to_string()).expect("client");
        for case in fixtures["routes"]
            .as_array()
            .expect("routes")
            .iter()
            .filter(|route| route["owner"] == "u4")
            .flat_map(|route| route["cases"].as_array().expect("cases"))
        {
            let request = if case["shim_request"].is_object() {
                &case["shim_request"]
            } else {
                &case["request"]
            };
            let Some(argv) = request["argv"].as_array() else {
                continue;
            };
            let argv: Vec<String> = argv
                .iter()
                .map(|arg| arg.as_str().expect("arg").to_string())
                .collect();
            let caller: CallerContext =
                serde_json::from_value(request["caller"].clone()).expect("caller");
            let actual = match argv[0].as_str() {
                "anomalies" => crate::anomalies::anomalies(&client, &argv[1..], &caller).await,
                "decisions" => decisions(&client, &argv[1..], &caller).await,
                "answer" => answer(&client, &argv[1..], &caller).await,
                _ => continue,
            };
            let expected: Envelope<Value> =
                serde_json::from_value(case["response"].clone()).expect("response");
            assert_eq!(
                serde_json::to_vec(&actual).expect("actual"),
                serde_json::to_vec(&expected).expect("expected")
            );
        }
        server.abort();
        std::fs::remove_dir_all(dir).expect("remove owned fixture");
    }
}
