//! Native governance commands forward argv to the daemon's sole parser.

use pij_core::model::{Envelope, ErrorKind};
use serde_json::{Value, json};

use crate::{CallerContext, DaemonClient};

/// Forward a governance family, its arguments excluding the family token, and
/// caller evidence. All leaf/flag semantics and identity checks live in rs.
pub async fn governance(
    client: &DaemonClient,
    family: &str,
    args: &[String],
    caller: &CallerContext,
) -> Envelope<Value> {
    let path = match family {
        "project" => "/v1/project",
        "stream" => "/v1/stream",
        "fence" => "/v1/fence",
        "dispatch" => "/v1/dispatch",
        "ack" => "/v1/ack",
        "canary" => "/v1/canary",
        "attest" => "/v1/attest",
        "task" => "/v1/task",
        "node" => "/v1/node",
        "orchestration" => "/v1/orchestration",
        "spine" => "/v1/spine",
        _ => {
            let mut error = Envelope::refused(
                format!("pij {family}"),
                ErrorKind::Refused,
                format!("E-RS-UNPORTED {family}: not a governance family"),
            );
            error.details = Some(json!({"code": "E-RS-UNPORTED", "verb": family}));
            return error;
        }
    };
    let argv: Vec<&str> = std::iter::once(family)
        .chain(args.iter().map(String::as_str))
        .collect();
    client
        .post(
            &format!("pij {family}"),
            path,
            &json!({"argv": argv, "caller": caller}),
        )
        .await
}

#[cfg(test)]
mod tests {
    use axum::extract::{Json, State};
    use axum::http::HeaderMap;
    use axum::routing::post;
    use pij_core::model::Envelope;
    use pij_testkit::fresh_dir;
    use serde_json::Value;
    use std::sync::Arc;

    use super::governance;
    use crate::{CallerContext, DaemonClient};

    struct StateDir(std::path::PathBuf);

    impl Drop for StateDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    async fn echo_contract(
        State(fixtures): State<Arc<Value>>,
        headers: HeaderMap,
        Json(request): Json<Value>,
    ) -> String {
        assert_eq!(headers["authorization"], "Bearer governance-cli-key");
        let case = fixtures["routes"]
            .as_array()
            .expect("routes")
            .iter()
            .filter(|route| route["owner"] == "u3")
            .flat_map(|route| route["cases"].as_array().expect("cases"))
            .find(|case| case["request"]["argv"] == request["argv"])
            .expect("argv forwarded untouched, family attached once");
        let expected_caller: CallerContext =
            serde_json::from_value(case["request"]["caller"].clone()).expect("canonical caller");
        assert_eq!(
            request["caller"],
            serde_json::to_value(expected_caller).expect("caller wire")
        );
        let response: Envelope<Value> =
            serde_json::from_value(case["response"].clone()).expect("envelope");
        serde_json::to_string(&response).expect("same envelope serializer")
    }

    #[tokio::test]
    async fn governance_forwards_all_canonical_argv_and_preserves_full_envelope_bytes() {
        let fixtures: Value = serde_json::from_str(include_str!(
            "../../testkit/fixtures/golden/api/governance-routes.json"
        ))
        .expect("contracts");
        let mut router = axum::Router::new();
        for route in fixtures["routes"]
            .as_array()
            .expect("routes")
            .iter()
            .filter(|route| route["owner"] == "u3")
        {
            router = router.route(route["path"].as_str().expect("path"), post(echo_contract));
        }
        let router = router.with_state(Arc::new(fixtures.clone()));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("address");
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.expect("serve");
        });
        let state_dir = StateDir(fresh_dir("pij-governance-cli"));
        let dir = &state_dir.0;
        std::fs::write(dir.join("daemon.key"), "governance-cli-key").expect("test-only key");
        let client = DaemonClient::new(dir, &addr.to_string()).expect("client");
        for case in fixtures["routes"]
            .as_array()
            .expect("routes")
            .iter()
            .filter(|route| route["owner"] == "u3")
            .flat_map(|route| route["cases"].as_array().expect("cases"))
        {
            let argv: Vec<String> =
                serde_json::from_value(case["request"]["argv"].clone()).expect("argv");
            let caller: CallerContext =
                serde_json::from_value(case["request"]["caller"].clone()).expect("caller");
            let actual = governance(&client, &argv[0], &argv[1..], &caller).await;
            let expected: Envelope<Value> =
                serde_json::from_value(case["response"].clone()).expect("response");
            assert_eq!(
                serde_json::to_vec(&actual).expect("actual"),
                serde_json::to_vec(&expected).expect("expected"),
                "{}",
                case["id"]
            );
        }
        server.abort();
    }
}
