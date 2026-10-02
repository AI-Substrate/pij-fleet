use serde::Serialize;
use serde::de::DeserializeOwned;

use pij_core::error::{PijError, Result};
use pij_core::model::{Envelope, ErrorKind};
use pij_core::wire;

use super::{CursorResetDetail, StreamFrame};

/// A manually bootstrapped peer daemon.
#[derive(Clone, PartialEq, Eq)]
pub struct PeerEndpoint {
    /// Peer daemon base URL, such as `http://workstation.local:7463`.
    pub base_url: String,
    /// Persistent per-machine-pair bearer key configured on both machines.
    pub bearer_key: String,
}

/// POST one JSON request to a peer and decode its versioned envelope.
///
/// This primitive deliberately performs exactly one request. Retry, discovery,
/// peer selection, and long-lived federation workers belong to `federation`.
///
/// # Errors
/// Network failures, unreadable responses, and malformed or future envelopes.
pub async fn post_to_peer<Request, Response>(
    client: &reqwest::Client,
    peer: &PeerEndpoint,
    path: &str,
    request: &Request,
) -> Result<Envelope<Response>>
where
    Request: Serialize + ?Sized,
    Response: DeserializeOwned,
{
    let url = peer_url(peer, path);
    let response = client
        .post(&url)
        .bearer_auth(&peer.bearer_key)
        .json(request)
        .send()
        .await
        .map_err(|error| peer_error(&url, error))?;
    decode_peer_envelope(&url, response).await
}

/// GET one authenticated peer envelope.
///
/// # Errors
/// Network failures, unreadable responses, and malformed or future envelopes.
pub async fn get_from_peer<Response>(
    client: &reqwest::Client,
    peer: &PeerEndpoint,
    path: &str,
) -> Result<Envelope<Response>>
where
    Response: DeserializeOwned,
{
    let url = peer_url(peer, path);
    let response = client
        .get(&url)
        .bearer_auth(&peer.bearer_key)
        .send()
        .await
        .map_err(|error| peer_error(&url, error))?;
    decode_peer_envelope(&url, response).await
}

/// Open one authenticated peer NDJSON stream.
///
/// # Errors
/// Network failures or a non-success response.
pub async fn stream_from_peer(
    client: &reqwest::Client,
    peer: &PeerEndpoint,
    path: &str,
    query: &[(&str, String)],
) -> Result<PeerEventStream> {
    let url = peer_url(peer, path);
    let response = client
        .get(&url)
        .query(query)
        .bearer_auth(&peer.bearer_key)
        .send()
        .await
        .map_err(|error| peer_error(&url, error))?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|error| peer_error(&url, error))?;
        // Decode the peer's KIND and re-raise the typed refusal. Flattening every
        // non-2xx into an Adapter string forced the caller to pattern-match our
        // own prose, so a wording change would silently disable its recovery
        // (review F10). A peer too old to send a kind still produces the generic
        // Adapter error, which is exactly the mixed-version case the reviewer
        // named — and it now degrades to "retrying" rather than to silence.
        let refusal = serde_json::from_str::<Envelope<CursorResetDetail>>(&body).ok();
        if refusal.as_ref().and_then(|envelope| envelope.error) == Some(ErrorKind::CursorReset) {
            // Re-raise with the peer's OWN numbers. Fabricating zeroes gave an
            // operator the right diagnosis carrying evidence nobody measured
            // (review round 4) — the same defect as a receipt claiming an
            // unobserved delivery, one layer down in the diagnostics.
            let detail = refusal
                .and_then(|envelope| envelope.data)
                .unwrap_or(CursorResetDetail {
                    requested: 0,
                    newest: 0,
                });
            return Err(PijError::CursorBeyondSpine {
                requested: detail.requested,
                newest: detail.newest,
            });
        }
        return Err(PijError::Adapter {
            adapter: "daemon/http-peer".to_string(),
            message: format!("peer {url} refused event stream with {status}: {body}"),
        });
    }
    Ok(PeerEventStream {
        url,
        response,
        buffer: Vec::new(),
        line_number: 0,
        saw_hello: false,
        ended: false,
    })
}

/// Incremental reader for one peer's tagged NDJSON stream.
pub struct PeerEventStream {
    url: String,
    response: reqwest::Response,
    buffer: Vec<u8>,
    line_number: usize,
    saw_hello: bool,
    ended: bool,
}

impl PeerEventStream {
    /// Read the next complete frame, validating the Hello before any payload.
    ///
    /// # Errors
    /// Malformed UTF-8/JSON, version skew, or a dropped response body.
    pub async fn next_frame(&mut self) -> Result<Option<StreamFrame>> {
        loop {
            if let Some(line) = self.take_line()? {
                self.line_number += 1;
                if line.trim().is_empty() {
                    continue;
                }
                if !self.saw_hello {
                    self.read_hello(&line)?;
                    self.saw_hello = true;
                    continue;
                }
                let frame = serde_json::from_str(&line).map_err(|error| PijError::Adapter {
                    adapter: "daemon/http-peer".to_string(),
                    message: format!(
                        "peer {} line {} is not a stream frame: {error}",
                        self.url, self.line_number
                    ),
                })?;
                return Ok(Some(frame));
            }
            if self.ended {
                if !self.buffer.is_empty() {
                    self.buffer.push(b'\n');
                    continue;
                }
                if !self.saw_hello {
                    return Err(PijError::Adapter {
                        adapter: "daemon/http-peer".to_string(),
                        message: format!("peer {} ended before its Hello line", self.url),
                    });
                }
                return Ok(None);
            }
            match self.response.chunk().await {
                Ok(Some(chunk)) => self.buffer.extend_from_slice(&chunk),
                Ok(None) => self.ended = true,
                Err(error) => return Err(peer_error(&self.url, error)),
            }
        }
    }

    fn take_line(&mut self) -> Result<Option<String>> {
        let Some(newline) = self.buffer.iter().position(|byte| *byte == b'\n') else {
            return Ok(None);
        };
        let mut bytes: Vec<u8> = self.buffer.drain(..=newline).collect();
        bytes.pop();
        if bytes.last() == Some(&b'\r') {
            bytes.pop();
        }
        String::from_utf8(bytes)
            .map(Some)
            .map_err(|error| PijError::Adapter {
                adapter: "daemon/http-peer".to_string(),
                message: format!(
                    "peer {} line {} is not UTF-8: {error}",
                    self.url,
                    self.line_number + 1
                ),
            })
    }

    fn read_hello(&self, line: &str) -> Result<()> {
        #[derive(serde::Deserialize)]
        struct Hello {
            hello: bool,
            v: u32,
            build: String,
        }
        let hello: Hello = serde_json::from_str(line).map_err(|error| PijError::Adapter {
            adapter: "daemon/http-peer".to_string(),
            message: format!("peer {} did not begin with Hello: {error}", self.url),
        })?;
        if !hello.hello || hello.build.is_empty() {
            return Err(PijError::Adapter {
                adapter: "daemon/http-peer".to_string(),
                message: format!("peer {} sent an incomplete Hello", self.url),
            });
        }
        if hello.v != wire::EVENT_VERSION {
            return Err(PijError::Adapter {
                adapter: "daemon/http-peer".to_string(),
                message: format!(
                    "peer {} event stream v{} is unsupported; this build speaks v{}",
                    self.url,
                    hello.v,
                    wire::EVENT_VERSION
                ),
            });
        }
        Ok(())
    }
}

async fn decode_peer_envelope<Response>(
    url: &str,
    response: reqwest::Response,
) -> Result<Envelope<Response>>
where
    Response: DeserializeOwned,
{
    let body = response
        .text()
        .await
        .map_err(|error| peer_error(url, error))?;
    wire::decode_envelope(&body).map_err(|error| PijError::Adapter {
        adapter: "daemon/http-peer".to_string(),
        message: format!("peer {url} returned an unreadable envelope: {error}"),
    })
}

fn peer_url(peer: &PeerEndpoint, path: &str) -> String {
    format!(
        "{}/{}",
        peer.base_url.trim_end_matches('/'),
        path.trim_start_matches('/')
    )
}

fn peer_error(url: &str, error: reqwest::Error) -> PijError {
    PijError::Adapter {
        adapter: "daemon/http-peer".to_string(),
        message: format!("peer request to {url} failed: {error}"),
    }
}
