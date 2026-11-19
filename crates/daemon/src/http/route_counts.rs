//! Requests served, by route (F222). The daemon spent ~32% of a core in its
//! SQLite workers, and nothing said which caller drove it: `pij-rs --json
//! ping` reports these beside `spawns`, so two readings a known interval apart
//! give requests a second per route, and the route names the caller.
//!
//! In-process counters only: no launch, no event, no store write, no log per
//! request. Bounded by construction: a route is the router's own path
//! template (`MatchedPath`), never the request path, so a client can't add
//! keys; a path the router doesn't serve counts under one key, and so does a
//! method outside the standard set. `GET /v1/seats` is also counted by query
//! shape (which of its known parameters were present), because one route
//! serves the extension roster, peer fan-in and `pij list` alike. Nothing
//! reads the counts to decide anything, so they can't change behaviour.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use axum::extract::{MatchedPath, Request, State};
use axum::http::Method;
use axum::middleware::Next;
use axum::response::Response;
use serde::Serialize;

/// The methods counted by name; any other counts under [`OTHER_METHOD`].
const METHODS: [Method; 7] = [
    Method::GET,
    Method::POST,
    Method::PUT,
    Method::DELETE,
    Method::PATCH,
    Method::HEAD,
    Method::OPTIONS,
];

/// The key for a method outside [`METHODS`].
const OTHER_METHOD: &str = "OTHER";

/// The key for a request the router has no route for.
pub const UNMATCHED: &str = "(unmatched)";

/// The `GET /v1/seats` query parameters counted by shape, in bit order. The
/// last bit is `scope=local` itself, since a peer may send only that scope.
const SEAT_PARAMS: [&str; 6] = ["harness", "folder", "parent", "scope", "here", "sizes"];
const SCOPE_LOCAL_BIT: usize = SEAT_PARAMS.len();
const SEAT_SHAPES: usize = 1 << (SEAT_PARAMS.len() + 1);

/// One route's answers: success (2xx and 3xx) and refusal (4xx and 5xx).
#[derive(Default)]
struct Outcomes {
    ok: AtomicU64,
    refused: AtomicU64,
}

impl Outcomes {
    fn record(&self, success: bool) {
        let counter = if success { &self.ok } else { &self.refused };
        counter.fetch_add(1, Ordering::Relaxed);
    }
}

/// One path template's outcomes, per method (the last slot is any other).
#[derive(Default)]
struct PathCounts {
    by_method: [Outcomes; METHODS.len() + 1],
}

/// The daemon's request counts.
pub struct RouteCounts {
    /// Keyed by the router's path template; a key is allocated once, on the
    /// template's first request.
    paths: Mutex<BTreeMap<String, Arc<PathCounts>>>,
    unmatched: Outcomes,
    seat_shapes: [AtomicU64; SEAT_SHAPES],
}

impl Default for RouteCounts {
    fn default() -> Self {
        Self {
            paths: Mutex::new(BTreeMap::new()),
            unmatched: Outcomes::default(),
            seat_shapes: std::array::from_fn(|_| AtomicU64::new(0)),
        }
    }
}

/// One route's counts as `ping` reports them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct RouteCount {
    pub ok: u64,
    pub refused: u64,
}

/// Every count, read in one pass. Routes never requested are left out.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RequestCounts {
    pub total: u64,
    /// `"<METHOD> <path template>"`, e.g. `"GET /v1/seats"`.
    pub routes: BTreeMap<String, RouteCount>,
    /// `GET /v1/seats` by the query parameters present, joined with `&` in a
    /// fixed order, e.g. `"scope=local"` or `"here&sizes"`; `"(none)"` for a
    /// bare request.
    pub seats_queries: BTreeMap<String, u64>,
}

impl RouteCounts {
    /// Count one answered request. `seat_shape` is its [`seat_shape`] when it
    /// was a `GET /v1/seats`.
    fn record(
        &self,
        method: &Method,
        path: Option<&str>,
        seat_shape: Option<usize>,
        success: bool,
    ) {
        let Some(path) = path else {
            self.unmatched.record(success);
            return;
        };
        let slot = METHODS
            .iter()
            .position(|known| known == method)
            .unwrap_or(METHODS.len());
        let counts = {
            let mut paths = self
                .paths
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            match paths.get(path) {
                Some(counts) => Arc::clone(counts),
                None => Arc::clone(paths.entry(path.to_string()).or_default()),
            }
        };
        counts.by_method[slot].record(success);
        if let Some(shape) = seat_shape {
            self.seat_shapes[shape].fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Every count, read in one pass.
    #[must_use]
    pub fn snapshot(&self) -> RequestCounts {
        let mut routes = BTreeMap::new();
        let mut total = 0;
        let mut add = |key: String, outcomes: &Outcomes| {
            let count = RouteCount {
                ok: outcomes.ok.load(Ordering::Relaxed),
                refused: outcomes.refused.load(Ordering::Relaxed),
            };
            if count.ok + count.refused > 0 {
                total += count.ok + count.refused;
                routes.insert(key, count);
            }
        };
        {
            let paths = self
                .paths
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            for (path, counts) in paths.iter() {
                for (slot, outcomes) in counts.by_method.iter().enumerate() {
                    let method = METHODS.get(slot).map_or(OTHER_METHOD, Method::as_str);
                    add(format!("{method} {path}"), outcomes);
                }
            }
        }
        add(UNMATCHED.to_string(), &self.unmatched);
        let seats_queries = self
            .seat_shapes
            .iter()
            .enumerate()
            .filter_map(|(shape, count)| {
                let count = count.load(Ordering::Relaxed);
                (count > 0).then(|| (shape_name(shape), count))
            })
            .collect();
        RequestCounts {
            total,
            routes,
            seats_queries,
        }
    }
}

/// The query shape of a `GET /v1/seats`: one bit per known parameter
/// present, plus one for `scope=local`. Unknown parameters are ignored.
fn seat_shape(query: Option<&str>) -> usize {
    let mut shape = 0;
    for pair in query.unwrap_or_default().split('&') {
        let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
        if let Some(bit) = SEAT_PARAMS.iter().position(|known| *known == name) {
            shape |= 1 << bit;
            if name == "scope" && value == "local" {
                shape |= 1 << SCOPE_LOCAL_BIT;
            }
        }
    }
    shape
}

fn shape_name(shape: usize) -> String {
    let names: Vec<&str> = SEAT_PARAMS
        .iter()
        .enumerate()
        .filter(|(bit, _)| shape & (1 << bit) != 0)
        .map(|(_, name)| {
            if *name == "scope" && shape & (1 << SCOPE_LOCAL_BIT) != 0 {
                "scope=local"
            } else {
                name
            }
        })
        .collect();
    if names.is_empty() {
        "(none)".to_string()
    } else {
        names.join("&")
    }
}

/// The outermost layer: count each request under its route once it is
/// answered.
pub(crate) async fn count_request(
    State(counts): State<Arc<RouteCounts>>,
    request: Request,
    next: Next,
) -> Response {
    let method = request.method().clone();
    // MatchedPath holds its template in an Arc: cloning it copies no bytes.
    let path = request.extensions().get::<MatchedPath>().cloned();
    let shape = (method == Method::GET
        && path
            .as_ref()
            .is_some_and(|path| path.as_str() == "/v1/seats"))
    .then(|| seat_shape(request.uri().query()));
    let response = next.run(request).await;
    let success = response.status().is_success() || response.status().is_redirection();
    counts.record(
        &method,
        path.as_ref().map(MatchedPath::as_str),
        shape,
        success,
    );
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_by_route_method_and_outcome_and_leaves_unrequested_routes_out() {
        let counts = RouteCounts::default();
        counts.record(&Method::POST, Some("/v1/whoami"), None, true);
        counts.record(&Method::POST, Some("/v1/whoami"), None, true);
        counts.record(&Method::POST, Some("/v1/whoami"), None, false);
        counts.record(&Method::GET, Some("/v1/whoami"), None, true);
        counts.record(&Method::TRACE, Some("/v1/whoami"), None, false);
        counts.record(&Method::GET, None, None, false);
        let snapshot = counts.snapshot();
        assert_eq!(
            snapshot.routes.get("POST /v1/whoami"),
            Some(&RouteCount { ok: 2, refused: 1 })
        );
        assert_eq!(
            snapshot.routes.get("GET /v1/whoami"),
            Some(&RouteCount { ok: 1, refused: 0 })
        );
        assert_eq!(
            snapshot.routes.get("OTHER /v1/whoami"),
            Some(&RouteCount { ok: 0, refused: 1 })
        );
        assert_eq!(
            snapshot.routes.get(UNMATCHED),
            Some(&RouteCount { ok: 0, refused: 1 })
        );
        assert_eq!(snapshot.routes.len(), 4, "{snapshot:?}");
        assert_eq!(snapshot.total, 6);
        assert!(snapshot.seats_queries.is_empty());
    }

    #[test]
    fn seats_reads_are_counted_by_query_shape_and_unknown_parameters_add_no_keys() {
        let counts = RouteCounts::default();
        for query in [
            None,
            Some("scope=local"),
            Some("scope=local&junk=1"),
            Some("scope=remote"),
            Some("sizes=true&here=%2Ftmp"),
            Some("here=%2Fa&sizes=1"),
            Some("x=1&y=2"),
        ] {
            counts.record(
                &Method::GET,
                Some("/v1/seats"),
                Some(seat_shape(query)),
                true,
            );
        }
        counts.record(&Method::POST, Some("/v1/seats"), None, true);
        let snapshot = counts.snapshot();
        let expected: BTreeMap<String, u64> = [
            ("(none)", 2),
            ("scope=local", 2),
            ("scope", 1),
            ("here&sizes", 2),
        ]
        .into_iter()
        .map(|(shape, count)| (shape.to_string(), count))
        .collect();
        assert_eq!(snapshot.seats_queries, expected);
        assert_eq!(
            snapshot.routes.get("GET /v1/seats"),
            Some(&RouteCount { ok: 7, refused: 0 })
        );
    }
}
