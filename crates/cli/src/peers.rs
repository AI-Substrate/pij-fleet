//! `pij-rs peers check` and `pij-rs peers new-key` (plan 164 ruling 7).
//!
//! `check` validates `<state-dir>/peers.toml` exactly as the daemon will at
//! boot (owner, mode, aliases, keys), then calls each peer's local roster with
//! that peer's key, so a pairing is proven from this end before anyone relies
//! on it. Keys are shown only as fingerprints.

use std::path::Path;
use std::time::Duration;

use pij_core::model::{Envelope, ErrorKind};
use pij_daemon::pairing::{self, PEERS_FILE};
use serde::Serialize;

/// How long `peers check` waits for one peer.
const PEER_CHECK_TIMEOUT: Duration = Duration::from_secs(5);

/// One peer's line in the check.
#[derive(Debug, Serialize)]
pub struct PeerCheck {
    /// The configured alias.
    pub alias: String,
    /// The configured base URL.
    pub url: String,
    /// First 8 hex characters of the key's SHA-256.
    pub fingerprint: String,
    /// `ok`, or why not.
    pub status: String,
    /// Seats the peer listed, when it answered.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seats: Option<usize>,
}

/// The whole check.
#[derive(Debug, Serialize)]
pub struct PeersReport {
    /// The file checked.
    pub file: String,
    /// This machine's alias, from the file.
    pub machine: Option<String>,
    /// One row per configured peer.
    pub peers: Vec<PeerCheck>,
}

const COMMAND: &str = "pij peers check";

/// Validate the file and reach every peer.
pub async fn check(state_dir: &Path, euid: u32) -> Envelope<PeersReport> {
    let file = state_dir.join(PEERS_FILE).display().to_string();
    let pairing = match pairing::load(state_dir, euid) {
        Ok(Some(pairing)) => pairing,
        Ok(None) => {
            return Envelope::refused(
                COMMAND,
                ErrorKind::NotFound,
                format!(
                    "{file} does not exist: no machine is paired, and the daemon listens on loopback only"
                ),
            );
        }
        Err(error) => return Envelope::refused(COMMAND, ErrorKind::Refused, error.to_string()),
    };
    // Direct only, like the daemon's own peer client: a proxy would see the key.
    let client = match reqwest::Client::builder()
        .no_proxy()
        .timeout(PEER_CHECK_TIMEOUT)
        .build()
    {
        Ok(client) => client,
        Err(error) => {
            return Envelope::refused(
                COMMAND,
                ErrorKind::Adapter,
                format!("could not build the peer client: {error}"),
            );
        }
    };
    let mut rows = Vec::with_capacity(pairing.peers.len());
    for peer in &pairing.peers {
        let (status, seats) = reach(&client, &peer.alias, &peer.url, &peer.key).await;
        rows.push(PeerCheck {
            alias: peer.alias.clone(),
            url: peer.url.clone(),
            fingerprint: pairing::fingerprint(&peer.key),
            status,
            seats,
        });
    }
    let failed: Vec<String> = rows
        .iter()
        .filter(|row| row.status != "ok")
        .map(|row| row.alias.clone())
        .collect();
    let report = PeersReport {
        file,
        machine: Some(pairing.machine),
        peers: rows,
    };
    if failed.is_empty() {
        return Envelope::ok(COMMAND, report);
    }
    let mut refusal = Envelope::refused(
        COMMAND,
        ErrorKind::Refused,
        format!("not reachable as paired: {}", failed.join(", ")),
    );
    refusal.data = Some(report);
    refusal
}

async fn reach(
    client: &reqwest::Client,
    alias: &str,
    url: &str,
    key: &str,
) -> (String, Option<usize>) {
    let response = match client
        .get(format!("{url}/v1/seats?scope=local"))
        .bearer_auth(key)
        .send()
        .await
    {
        Ok(response) => response,
        // reqwest's error names the URL, never the Authorization header.
        Err(error) => return (format!("unreachable: {error}"), None),
    };
    match response.status().as_u16() {
        200 => {}
        401 => {
            return (
                format!(
                    "key refused: the peer's peers.toml does not hold this key (fingerprint {}) for this machine",
                    pairing::fingerprint(key)
                ),
                None,
            );
        }
        status => return (format!("peer answered HTTP {status}"), None),
    }
    let body: serde_json::Value = match response.json().await {
        Ok(body) => body,
        Err(error) => return (format!("not a pij answer: {error}"), None),
    };
    // Only an authenticated pij roster envelope proves the pairing: a proxy, a
    // captive portal or another service can answer 200 too (review S5).
    let Some(seats) = body["data"]["seats"]
        .as_array()
        .filter(|_| body["ok"] == true && body["command"] == "pij seats" && body["v"].is_u64())
    else {
        return (
            "not a pij roster answer: something other than a pij daemon answered (a proxy or another service?)"
                .to_string(),
            None,
        );
    };
    if let Some(other) = seats
        .iter()
        .filter_map(|seat| seat["machine"].as_str())
        .find(|machine| *machine != alias)
    {
        return (
            format!(
                "alias mismatch: the peer calls itself `{other}`; make the two peers.toml files agree"
            ),
            Some(seats.len()),
        );
    }
    ("ok".to_string(), Some(seats.len()))
}

/// The human table for a check.
pub fn render(envelope: &Envelope<PeersReport>) -> String {
    let Some(report) = envelope.data.as_ref() else {
        return format!(
            "{}: FAILED — {}",
            envelope.command,
            envelope.meta.as_deref().unwrap_or("no reason given")
        );
    };
    let mut lines = vec![format!(
        "{} · this machine: {}",
        report.file,
        report.machine.as_deref().unwrap_or("?")
    )];
    for peer in &report.peers {
        lines.push(format!(
            "  {:<16} {:<32} key {}  {}",
            peer.alias, peer.url, peer.fingerprint, peer.status
        ));
    }
    if let Some(meta) = envelope.meta.as_deref() {
        lines.push(meta.to_string());
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use axum::routing::get;

    use super::reach;

    /// Review F05 follow-up: the error envelope and its human rendering for a
    /// malformed file carry no text from the file.
    #[tokio::test]
    async fn a_malformed_file_never_reaches_the_envelope() {
        use std::os::unix::fs::PermissionsExt as _;
        let key = pij_daemon::pairing::new_key().expect("key");
        let dir = pij_testkit::fresh_dir("pij-peers-check-malformed");
        let path = dir.join(pij_daemon::pairing::PEERS_FILE);
        std::fs::write(&path, format!("machine = \"m\"\npeer=[\n{key}\n]\n")).expect("write");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).expect("chmod");
        let uid = std::os::unix::fs::MetadataExt::uid(&std::fs::metadata(&path).expect("stat"));
        let envelope = super::check(&dir, uid).await;
        assert!(!envelope.ok);
        let json = serde_json::to_string(&envelope).expect("json");
        let human = super::render(&envelope);
        assert!(!json.contains(&key), "{json}");
        assert!(!human.contains(&key), "{human}");
    }

    /// Review S5: a 200 that is not an authenticated pij roster envelope (a
    /// proxy's `{}`, a captive portal) is not a reachable peer.
    #[tokio::test]
    async fn only_a_real_pij_roster_envelope_counts_as_reachable() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        let server = tokio::spawn(async move {
            axum::serve(
                listener,
                axum::Router::new().route("/v1/seats", get(|| async { "{}" })),
            )
            .await
            .expect("serve");
        });
        let (status, _) = reach(
            &reqwest::Client::new(),
            "laptop",
            &format!("http://{addr}"),
            "some-generated-test-key",
        )
        .await;
        assert_ne!(status, "ok");
        server.abort();
    }
}
