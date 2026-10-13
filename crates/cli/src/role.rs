//! Native role leaf: forward to the same daemon parser as the generation shim.

use pij_core::model::Envelope;
use serde_json::{Value, json};

use crate::{CallerContext, DaemonClient};

impl DaemonClient {
    /// Forward role arguments (without the leading verb) and caller evidence.
    /// The original daemon envelope is returned intact, including refusals.
    pub async fn role(&self, args: &[String], caller: &CallerContext) -> Envelope<Value> {
        let argv: Vec<&str> = std::iter::once("role")
            .chain(args.iter().map(String::as_str))
            .collect();
        self.post(
            "pij role",
            "/v1/role",
            &json!({"argv":argv,"caller":caller}),
        )
        .await
    }

    /// Forward watchdog arguments (without the leading verb) and caller evidence.
    pub async fn watchdog(&self, args: &[String], caller: &CallerContext) -> Envelope<Value> {
        let argv: Vec<&str> = std::iter::once("watchdog")
            .chain(args.iter().map(String::as_str))
            .collect();
        self.post(
            "pij watchdog",
            "/v1/watchdog",
            &json!({"argv":argv,"caller":caller}),
        )
        .await
    }

    /// Forward link arguments (without the leading verb) and caller evidence.
    pub async fn link(&self, args: &[String], caller: &CallerContext) -> Envelope<Value> {
        let argv: Vec<&str> = std::iter::once("link")
            .chain(args.iter().map(String::as_str))
            .collect();
        self.post(
            "pij link",
            "/v1/link",
            &json!({"argv":argv,"caller":caller}),
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::extract::State;
    use axum::http::{HeaderMap, StatusCode};
    use axum::routing::post;
    use axum::{Json, Router};
    use tokio::sync::Mutex;

    use super::*;

    const ROUTES: &str = include_str!("../../testkit/fixtures/golden/api/governance-routes.json");

    #[derive(Clone)]
    struct FixtureState {
        seen: Arc<Mutex<Vec<Value>>>,
        response: Value,
        status: StatusCode,
    }

    async fn handle(
        State(state): State<FixtureState>,
        headers: HeaderMap,
        Json(body): Json<Value>,
    ) -> (StatusCode, Json<Value>) {
        assert_eq!(
            headers.get("authorization").expect("bearer"),
            "Bearer fixture-key"
        );
        state.seen.lock().await.push(body);
        (state.status, Json(state.response))
    }

    #[tokio::test]
    async fn native_role_forwards_u0_argv_and_preserves_success_and_refusal_envelopes() {
        let contract: Value = serde_json::from_str(ROUTES).expect("u0 routes");
        let case = &contract["routes"]
            .as_array()
            .expect("routes")
            .iter()
            .find(|route| route["path"] == "/v1/role")
            .expect("role route")["cases"][0];
        let caller: CallerContext =
            serde_json::from_value(case["shim_request"]["caller"].clone()).expect("caller");
        let args: Vec<String> = case["shim_request"]["argv"].as_array().expect("argv")[1..]
            .iter()
            .map(|arg| arg.as_str().expect("argument").to_string())
            .collect();
        for (status, expected) in [
            (StatusCode::OK, case["response"].clone()),
            (
                StatusCode::FORBIDDEN,
                contract["refusals"]["ownership"]["response"].clone(),
            ),
        ] {
            let state_dir = pij_testkit::fresh_dir("pij-role-client");
            std::fs::write(state_dir.join("daemon.key"), "fixture-key")
                .expect("private fixture key");
            let seen = Arc::new(Mutex::new(Vec::new()));
            let app = Router::new()
                .route("/v1/role", post(handle))
                .with_state(FixtureState {
                    seen: seen.clone(),
                    response: expected.clone(),
                    status,
                });
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("private listener");
            let address = listener.local_addr().expect("address").to_string();
            let server = tokio::spawn(async move {
                axum::serve(listener, app).await.expect("serve");
            });
            let client = DaemonClient::new(&state_dir, &address).expect("client");
            let response = client.role(&args, &caller).await;
            server.abort();
            std::fs::remove_dir_all(&state_dir).expect("remove owned client fixture directory");
            assert_eq!(serde_json::to_value(response).expect("wire"), expected);
            let requests = seen.lock().await;
            assert_eq!(requests.len(), 1);
            assert_eq!(requests[0]["argv"], case["shim_request"]["argv"]);
            // The shared CallerContext serializer may include absent allowlisted keys.
            let submitted: CallerContext =
                serde_json::from_value(requests[0]["caller"].clone()).expect("submitted caller");
            assert_eq!(submitted.pane, caller.pane);
            assert_eq!(submitted.cwd, caller.cwd);
            assert_eq!(submitted.session_id, caller.session_id);
        }
    }
}
