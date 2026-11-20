//! Requests served, by route (F222). The daemon spent ~32% of a core in its
//! SQLite workers, and nothing said which caller drove it: `pij-rs --json
//! ping` reports these beside `spawns`, so two readings a known interval apart
//! give requests a second per route, and the route names the caller.
//!
//! In-process counters only: no launch, no event, no store write, no log per
//! request. The table of route templates is built once, from the endpoint
//! inventory, when the router is built, and never changes: recording a
//! request is a lookup in that immutable table and one atomic increment,
//! with no lock, no allocation and nothing shared to wait on (P266-001). A
//! template outside the table and a path the router doesn't serve count under
//! one key, and so does a method outside the standard set, so a client can't
//! add keys. `GET /v1/seats` is also counted by query shape (which of its
//! known parameters were present), because one route serves the extension
//! roster, peer fan-in and `pij list` alike: a second increment, after one
//! scan of its query bytes. Nothing reads the counts to decide anything, so
//! they can't change behaviour.
//!
//! A snapshot reads each counter on its own, without stopping recording, so
//! it is not transactional: a request answered mid-read can show in one
//! count and not yet in another. `total` is the sum of the route counts it
//! read. Counters wrap at `u64::MAX`.
//!
//! F231: each route also sums its handler time, in microseconds, measured
//! once per request around the rest of the stack (two `Instant::now()`, one
//! or two relaxed atomic adds, no allocation). A long-poll's wait is held,
//! not active: the wait marks itself with a [`HeldSpan`], which adds its
//! length to a task-local the layer scopes around each request, so
//! `active_us` is the handler's wall time less what it spent waiting.
//! `active_us / (ok + refused)` is the route's cost per call.

use std::cell::Cell;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

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

/// The key for a request the router has no route for, or one whose template
/// is not in the table.
pub const UNMATCHED: &str = "(unmatched)";

/// The `GET /v1/seats` query parameters counted by shape, in bit order. The
/// last bit is `scope=local` itself, since a peer may send only that scope.
const SEAT_PARAMS: [&str; 6] = ["harness", "folder", "parent", "scope", "here", "sizes"];
const SCOPE_LOCAL_BIT: usize = SEAT_PARAMS.len();
const SEAT_SHAPES: usize = 1 << (SEAT_PARAMS.len() + 1);
const SEATS_PATH: &str = "/v1/seats";

/// One route's answers: success (2xx and 3xx) and refusal (4xx and 5xx),
/// and the time its handler spent on them (F231).
#[derive(Default)]
struct Outcomes {
    ok: AtomicU64,
    refused: AtomicU64,
    active_us: AtomicU64,
    held_us: AtomicU64,
}

impl Outcomes {
    fn record(&self, success: bool, spent: Spent) {
        let counter = if success { &self.ok } else { &self.refused };
        counter.fetch_add(1, Ordering::Relaxed);
        self.active_us.fetch_add(spent.active_us, Ordering::Relaxed);
        if spent.held_us != 0 {
            self.held_us.fetch_add(spent.held_us, Ordering::Relaxed);
        }
    }

    fn read(&self) -> RouteCount {
        RouteCount {
            ok: self.ok.load(Ordering::Relaxed),
            refused: self.refused.load(Ordering::Relaxed),
            active_us: self.active_us.load(Ordering::Relaxed),
            held_us: self.held_us.load(Ordering::Relaxed),
        }
    }
}

/// One request's handler time, split (F231).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Spent {
    active_us: u64,
    held_us: u64,
}

impl Spent {
    /// `wall` less what the request spent `held`, in microseconds.
    fn split(wall: std::time::Duration, held_us: u64) -> Self {
        let wall_us = u64::try_from(wall.as_micros()).unwrap_or(u64::MAX);
        Self {
            active_us: wall_us.saturating_sub(held_us),
            held_us: held_us.min(wall_us),
        }
    }
}

tokio::task_local! {
    /// Microseconds the current request has spent waiting (F231).
    static HELD_US: Cell<u64>;
}

/// A long-poll's wait: while it lives, the request is held, not working.
/// Its length is added to the request's held time when it drops, however
/// the wait ends. Outside a counted request it records nothing.
pub(crate) struct HeldSpan(Instant);

impl HeldSpan {
    pub(crate) fn start() -> Self {
        Self(Instant::now())
    }
}

impl Drop for HeldSpan {
    fn drop(&mut self) {
        let waited = u64::try_from(self.0.elapsed().as_micros()).unwrap_or(u64::MAX);
        let _ = HELD_US.try_with(|held| held.set(held.get().saturating_add(waited)));
    }
}

/// One path template's outcomes, per method (the last slot is any other).
#[derive(Default)]
struct PathCounts {
    by_method: [Outcomes; METHODS.len() + 1],
}

/// The daemon's request counts.
pub struct RouteCounts {
    /// Every route template, sorted for a binary search, each with its
    /// counters inline. Fixed when the router is built.
    paths: Box<[(&'static str, PathCounts)]>,
    unmatched: Outcomes,
    seat_shapes: [AtomicU64; SEAT_SHAPES],
}

/// One route's counts as `ping` reports them. `active_us` is the handler
/// time spent working and `held_us` the time a long-poll spent waiting, both
/// summed over every request, in microseconds (F231).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct RouteCount {
    pub ok: u64,
    pub refused: u64,
    pub active_us: u64,
    pub held_us: u64,
}

/// Every count, read in one pass. Routes never requested are left out.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RequestCounts {
    /// The sum of the route counts below, as read.
    pub total: u64,
    /// `"<METHOD> <path template>"`, e.g. `"GET /v1/seats"`.
    pub routes: BTreeMap<String, RouteCount>,
    /// `GET /v1/seats` by the query parameters present, joined with `&` in a
    /// fixed order, e.g. `"scope=local"` or `"here&sizes"`; `"(none)"` for a
    /// bare request.
    pub seats_queries: BTreeMap<String, u64>,
}

impl RouteCounts {
    /// The table for these route templates. Built once, when the router is.
    #[must_use]
    pub fn new(templates: impl IntoIterator<Item = &'static str>) -> Self {
        let mut templates: Vec<&'static str> = templates.into_iter().collect();
        templates.sort_unstable();
        templates.dedup();
        Self {
            paths: templates
                .into_iter()
                .map(|template| (template, PathCounts::default()))
                .collect(),
            unmatched: Outcomes::default(),
            seat_shapes: std::array::from_fn(|_| AtomicU64::new(0)),
        }
    }

    /// Count one answered request and its handler time. `seat_shape` is its
    /// [`seat_shape`] when it was a `GET /v1/seats`.
    fn record(
        &self,
        method: &Method,
        path: Option<&str>,
        seat_shape: Option<usize>,
        success: bool,
        spent: Spent,
    ) {
        let counts = path.and_then(|path| {
            self.paths
                .binary_search_by(|(template, _)| (*template).cmp(path))
                .ok()
                .map(|index| &self.paths[index].1)
        });
        let Some(counts) = counts else {
            self.unmatched.record(success, spent);
            return;
        };
        let slot = METHODS
            .iter()
            .position(|known| known == method)
            .unwrap_or(METHODS.len());
        counts.by_method[slot].record(success, spent);
        if let Some(shape) = seat_shape {
            self.seat_shapes[shape].fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Every count, each read on its own while recording goes on.
    #[must_use]
    pub fn snapshot(&self) -> RequestCounts {
        let mut routes = BTreeMap::new();
        let mut total: u64 = 0;
        let mut add = |key: String, count: RouteCount| {
            if count.ok != 0 || count.refused != 0 {
                total = total.wrapping_add(count.ok).wrapping_add(count.refused);
                routes.insert(key, count);
            }
        };
        for (path, counts) in &self.paths {
            for (slot, outcomes) in counts.by_method.iter().enumerate() {
                let count = outcomes.read();
                if count.ok != 0 || count.refused != 0 {
                    let method = METHODS.get(slot).map_or(OTHER_METHOD, Method::as_str);
                    add(format!("{method} {path}"), count);
                }
            }
        }
        add(UNMATCHED.to_string(), self.unmatched.read());
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
/// answered, with the time the rest of the stack took, less any time it
/// spent held in a long-poll wait (F231).
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
            .is_some_and(|path| path.as_str() == SEATS_PATH))
    .then(|| seat_shape(request.uri().query()));
    let started = Instant::now();
    let (response, held_us) = HELD_US
        .scope(Cell::new(0), async {
            let response = next.run(request).await;
            (response, HELD_US.with(Cell::get))
        })
        .await;
    let spent = Spent::split(started.elapsed(), held_us);
    let success = response.status().is_success() || response.status().is_redirection();
    counts.record(
        &method,
        path.as_ref().map(MatchedPath::as_str),
        shape,
        success,
        spent,
    );
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table() -> RouteCounts {
        RouteCounts::new(["/v1/whoami", SEATS_PATH, "/health"])
    }

    /// A count with no handler time recorded.
    fn count(ok: u64, refused: u64) -> RouteCount {
        RouteCount {
            ok,
            refused,
            active_us: 0,
            held_us: 0,
        }
    }

    const NONE: Spent = Spent {
        active_us: 0,
        held_us: 0,
    };

    #[test]
    fn counts_by_route_method_and_outcome_and_leaves_unrequested_routes_out() {
        let counts = table();
        counts.record(&Method::POST, Some("/v1/whoami"), None, true, NONE);
        counts.record(&Method::POST, Some("/v1/whoami"), None, true, NONE);
        counts.record(&Method::POST, Some("/v1/whoami"), None, false, NONE);
        counts.record(&Method::GET, Some("/v1/whoami"), None, true, NONE);
        counts.record(&Method::TRACE, Some("/v1/whoami"), None, false, NONE);
        counts.record(&Method::GET, None, None, false, NONE);
        let snapshot = counts.snapshot();
        assert_eq!(snapshot.routes.get("POST /v1/whoami"), Some(&count(2, 1)));
        assert_eq!(snapshot.routes.get("GET /v1/whoami"), Some(&count(1, 0)));
        assert_eq!(snapshot.routes.get("OTHER /v1/whoami"), Some(&count(0, 1)));
        assert_eq!(snapshot.routes.get(UNMATCHED), Some(&count(0, 1)));
        assert_eq!(snapshot.routes.len(), 4, "{snapshot:?}");
        assert_eq!(snapshot.total, 6);
        assert!(snapshot.seats_queries.is_empty());
    }

    /// P266-001: the table is fixed when built. A template it doesn't hold
    /// never becomes a key: it counts as unmatched.
    #[test]
    fn a_template_outside_the_table_counts_as_unmatched_and_adds_no_key() {
        let counts = table();
        counts.record(
            &Method::GET,
            Some("/v1/not-in-the-inventory"),
            None,
            true,
            NONE,
        );
        let snapshot = counts.snapshot();
        assert_eq!(
            snapshot.routes.keys().collect::<Vec<_>>(),
            vec![UNMATCHED],
            "{snapshot:?}"
        );
        assert_eq!(snapshot.routes[UNMATCHED], count(1, 0));
    }

    /// P266-001: snapshots taken while requests are recorded on other
    /// threads neither block nor lose them: every recorded request is in
    /// the last snapshot.
    #[test]
    fn snapshots_during_recording_lose_nothing() {
        let counts = Arc::new(table());
        let recorders: Vec<_> = (0..4)
            .map(|_| {
                let counts = Arc::clone(&counts);
                std::thread::spawn(move || {
                    for _ in 0..10_000 {
                        counts.record(&Method::GET, Some(SEATS_PATH), Some(0), true, NONE);
                    }
                })
            })
            .collect();
        let mut snapshots = 0;
        while recorders.iter().any(|recorder| !recorder.is_finished()) {
            let _ = counts.snapshot();
            snapshots += 1;
        }
        for recorder in recorders {
            recorder.join().expect("recorder");
        }
        let snapshot = counts.snapshot();
        assert!(snapshots > 0);
        assert_eq!(snapshot.total, 40_000);
        assert_eq!(snapshot.seats_queries["(none)"], 40_000);
    }

    #[test]
    fn seats_reads_are_counted_by_query_shape_and_unknown_parameters_add_no_keys() {
        let counts = table();
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
                Some(SEATS_PATH),
                Some(seat_shape(query)),
                true,
                NONE,
            );
        }
        counts.record(&Method::POST, Some(SEATS_PATH), None, true, NONE);
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
        assert_eq!(snapshot.routes.get("GET /v1/seats"), Some(&count(7, 0)));
    }

    /// F231: through the real layer, N requests to a route raise its count by
    /// N and its active time by at least their work; a long-poll's wait is
    /// held time, not active, however it ends.
    #[tokio::test]
    async fn handler_time_is_summed_per_route_and_a_long_polls_wait_is_held_not_active() {
        use std::time::Duration;
        const WORK: Duration = Duration::from_millis(4);
        const WAIT: Duration = Duration::from_millis(80);
        let counts = Arc::new(RouteCounts::new(["/work", "/poll"]));
        let app = axum::Router::new()
            .route(
                "/work",
                axum::routing::get(|| async {
                    std::thread::sleep(WORK);
                    "worked"
                }),
            )
            .route(
                "/poll",
                axum::routing::get(|| async {
                    {
                        let _held = HeldSpan::start();
                        tokio::time::sleep(WAIT).await;
                    }
                    std::thread::sleep(WORK);
                    "polled"
                }),
            )
            .layer(axum::middleware::from_fn_with_state(
                Arc::clone(&counts),
                count_request,
            ));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = reqwest::Client::new();
        for path in ["/work", "/work", "/work", "/poll"] {
            let answer = client
                .get(format!("http://{addr}{path}"))
                .send()
                .await
                .unwrap();
            assert!(answer.status().is_success());
        }
        server.abort();
        let snapshot = counts.snapshot();
        let us = |duration: Duration| u64::try_from(duration.as_micros()).unwrap();
        let work = snapshot.routes["GET /work"];
        assert_eq!((work.ok, work.refused, work.held_us), (3, 0, 0), "{work:?}");
        assert!(work.active_us >= 3 * us(WORK), "{work:?}");
        let poll = snapshot.routes["GET /poll"];
        assert_eq!(poll.ok, 1, "{poll:?}");
        assert!(poll.held_us >= us(WAIT), "{poll:?}");
        assert!(
            poll.active_us >= us(WORK) && poll.active_us < us(WAIT),
            "the wait is not active time: {poll:?}"
        );
    }
}
