//! Plan 229: the spawn gate's decisions, the watch's notices and the status
//! client, against a stub status route on an ephemeral port.

use std::time::Duration;

use axum::Router;
use axum::routing::get;
use pij_core::model::{CACHE_PROXY_OPTED_OUT, CacheProxyRoute, Harness, SeatDescriptor};
use serde_json::{Value, json};

use super::{CacheProxy, NOT_LISTENING, Precheck, ProxyStatus, Reading, SeatSplit};
use super::{base_id, decide, precheck, transition};

const URL: &str = "http://127.0.0.1:7471";

/// The status route body exactly as s02 serves it (types from crane via
/// pm-004, 2026-11-18).
fn body(state: &str) -> Value {
    let ready = state == "ready";
    json!({
        "ready": ready,
        "state": state,
        "reason": if ready { "" } else { "quota exhausted until 2026-12-01" },
        "fix": if ready { "" } else { "wait for the reset or switch account: pij-rs cache-proxy account <login>" },
        "account": "octo",
        "upstream_host": "api.individual.githubcopilot.com",
        "quota": {"usable": ready, "reset": if ready { Value::Null } else { json!("2026-12-01") }},
        "models": {"claude-opus-5.5": {"max_prompt_tokens": 936000}},
        "counters": {"requests": 4, "refreshes": 0, "gh_launches": 1, "upstream_401": 0, "upstream_402": 0},
        "pid": 4242,
        "since": 1_763_400_000_000_u64,
    })
}

fn status(state: &str) -> Reading {
    Reading::Status(Box::new(
        serde_json::from_value::<ProxyStatus>(body(state)).expect("contract"),
    ))
}

#[test]
fn base_id_strips_only_a_trailing_1m() {
    assert_eq!(base_id("claude-opus-5.5-1m"), "claude-opus-5.5");
    assert_eq!(base_id("claude-opus-5.5"), "claude-opus-5.5");
    assert_eq!(base_id("claude-1m-preview"), "claude-1m-preview");
}

/// Rows 3, 13, 20: the checks that need no status read.
#[test]
fn precheck_settles_harness_opt_out_and_model_without_a_read() {
    for harness in [Harness::Omp, Harness::Claude, Harness::Codex, Harness::Pi] {
        assert_eq!(
            precheck(harness, Some("claude-opus-5.5"), false),
            Precheck::NotCopilot
        );
    }
    let Precheck::Plain(opted) = precheck(Harness::Copilot, Some("claude-opus-5.5"), true) else {
        panic!("opt-out is plain");
    };
    assert_eq!(opted.reason, CACHE_PROXY_OPTED_OUT);
    assert!(opted.opted_out());
    let Precheck::Plain(gpt) = precheck(Harness::Copilot, Some("gpt-6.1-sol"), false) else {
        panic!("a GPT model is plain");
    };
    assert_eq!(gpt.reason, "model gpt-6.1-sol is not Claude");
    let Precheck::Plain(unknown) = precheck(Harness::Copilot, None, false) else {
        panic!("no model is plain");
    };
    assert!(unknown.reason.contains("no --model"), "{}", unknown.reason);
    assert_eq!(
        precheck(Harness::Copilot, Some("claude-opus-5.5-1m"), false),
        Precheck::Ask {
            base_id: "claude-opus-5.5".to_string()
        }
    );
}

/// Rows 1, 4, 5, 6, 13 (off), 22: the decision from one reading.
#[test]
fn decide_is_proxied_only_when_ready_and_the_base_id_is_served() {
    let (seat, wiring) = decide(
        &Reading::Unreachable("connection refused".into()),
        "claude-opus-5.5",
        URL,
    );
    assert_eq!(
        (seat.state, seat.reason.as_str(), wiring),
        (CacheProxyRoute::Plain, "proxy not listening", None)
    );
    assert!(seat.fix.is_some());

    let (seat, wiring) = decide(&status("quota_exhausted"), "claude-opus-5.5", URL);
    assert_eq!(seat.state, CacheProxyRoute::Plain);
    assert!(seat.reason.contains("2026-12-01"), "{}", seat.reason);
    assert!(seat.fix.is_some_and(|fix| fix.contains("account")));
    assert!(wiring.is_none());

    let (seat, wiring) = decide(&status("off"), "claude-opus-5.5", URL);
    assert_eq!(seat.state, CacheProxyRoute::Plain);
    assert!(seat.reason.starts_with("proxy off"), "{}", seat.reason);
    assert!(wiring.is_none());

    let (seat, wiring) = decide(&status("ready"), "claude-sonnet-9", URL);
    assert_eq!(seat.reason, "model not served by Copilot API");
    assert!(wiring.is_none());

    let (seat, wiring) = decide(&status("ready"), "claude-opus-5.5", URL);
    assert_eq!(seat.state, CacheProxyRoute::Proxied);
    let wiring = wiring.expect("proxied wiring");
    assert_eq!(
        (
            wiring.base_url.as_str(),
            wiring.model_id.as_str(),
            wiring.max_prompt_tokens
        ),
        (URL, "claude-opus-5.5", 936_000)
    );
}

/// A ready flag disagreeing with its state is not ready.
#[test]
fn decide_needs_both_ready_and_the_ready_state() {
    let mut odd = body("ready");
    odd["ready"] = json!(false);
    let reading = Reading::Status(Box::new(serde_json::from_value(odd).expect("contract")));
    assert_eq!(
        decide(&reading, "claude-opus-5.5", URL).0.state,
        CacheProxyRoute::Plain
    );
}

fn seats() -> Vec<String> {
    vec!["pij-a".to_string(), "pij-b".to_string()]
}

/// pm-004 ruling: the first read is silent when ready or when nothing is
/// proxied; not ready with live proxied seats is reported.
#[test]
fn first_read_reports_only_a_problem_under_proxied_seats() {
    assert_eq!(transition(None, &status("ready"), URL, &seats()), None);
    assert_eq!(
        transition(None, &Reading::Unreachable("refused".into()), URL, &[]),
        None
    );
    let notice = transition(None, &Reading::Unreachable("refused".into()), URL, &seats())
        .expect("a dead sidecar under proxied seats is told at once");
    assert!(notice.contains("pij-a, pij-b"), "{notice}");
    assert!(notice.contains("pij-rs revive"), "{notice}");
    assert!(!notice.contains("(was"), "{notice}");
}

/// Rows 7 and 21 (F192): one notice per change, none while unchanged.
#[test]
fn each_change_is_one_notice_and_an_unchanged_state_is_none() {
    let readings = [
        status("ready"),
        status("ready"),
        Reading::Unreachable("refused".into()),
        Reading::Unreachable("refused".into()),
        Reading::Unreachable("no answer within 1 s".into()),
        status("ready"),
        status("ready"),
    ];
    let mut previous: Option<String> = None;
    let mut notices = Vec::new();
    for reading in &readings {
        if let Some(notice) = transition(previous.as_deref(), reading, URL, &seats()) {
            notices.push(notice);
        }
        previous = Some(reading.state().to_string());
    }
    assert_eq!(notices.len(), 2, "{notices:#?}");
    assert!(notices[0].contains("not listening") && notices[0].contains("(was ready)"));
    assert!(
        notices[1].contains("is ready") && notices[1].contains(&format!("(was {NOT_LISTENING})"))
    );
}

/// Crane's contract: `off` keeps serving existing seats, so its notice is
/// not a failure.
#[test]
fn off_is_told_as_new_spawns_plain_existing_seats_unaffected() {
    let notice = transition(Some("ready"), &status("off"), URL, &seats()).expect("a change");
    assert!(notice.contains("new Copilot spawns run plain"), "{notice}");
    assert!(notice.contains("unaffected"), "{notice}");
    assert!(!notice.contains("fail"), "{notice}");
}

/// Row 18: a Copilot seat with nothing recorded is plain; other harnesses
/// and tombstoned seats are not counted.
#[test]
fn seat_split_reads_a_missing_route_as_plain() {
    let mut proxied = SeatDescriptor::new("pij-p", Harness::Copilot, "/w");
    proxied.cache_proxy = Some(pij_core::model::SeatCacheProxy {
        state: CacheProxyRoute::Proxied,
        reason: "through".into(),
        fix: None,
    });
    let before = SeatDescriptor::new("pij-old", Harness::Copilot, "/w");
    let omp = SeatDescriptor::new("pij-omp", Harness::Omp, "/w");
    let mut gone = SeatDescriptor::new("pij-gone", Harness::Copilot, "/w");
    gone.tombstoned_at = Some(1);
    let split = SeatSplit::of(&[proxied, before.clone(), omp, gone]);
    assert_eq!(split.proxied, ["pij-p"]);
    assert_eq!(split.plain, ["pij-old"]);
    assert_eq!(
        before.cache_proxy_view().map(|view| view.reason),
        Some(pij_core::model::CACHE_PROXY_BEFORE_GATE.to_string())
    );
}

async fn stub(router: Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        axum::serve(listener, router).await.expect("serve");
    });
    format!("http://{addr}")
}

/// The client against a stub status route: ready wires the seat, a refused
/// port and a hang past 1 s are plain "proxy not listening".
#[tokio::test]
async fn gate_reads_the_status_route_and_fails_safe() {
    let ready = stub(Router::new().route(
        super::STATUS_PATH,
        get(|| async { axum::Json(body("ready")) }),
    ))
    .await;
    let proxy = CacheProxy::new(ready.clone());
    let (seat, wiring) = proxy
        .gate(Harness::Copilot, Some("claude-opus-5.5-1m"), false)
        .await;
    assert_eq!(seat.map(|seat| seat.state), Some(CacheProxyRoute::Proxied));
    assert_eq!(wiring.map(|wiring| wiring.base_url), Some(ready));
    assert_eq!(
        proxy
            .gate(Harness::Omp, Some("claude-opus-5.5"), false)
            .await,
        (None, None)
    );

    let closed = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let refused_url = format!("http://{}", closed.local_addr().expect("addr"));
    drop(closed);
    let (seat, wiring) = CacheProxy::new(refused_url)
        .gate(Harness::Copilot, Some("claude-opus-5.5"), false)
        .await;
    assert_eq!(
        seat.map(|seat| seat.reason),
        Some("proxy not listening".to_string())
    );
    assert!(wiring.is_none());

    let hung = stub(Router::new().route(
        super::STATUS_PATH,
        get(|| async {
            tokio::time::sleep(Duration::from_secs(5)).await;
            axum::Json(body("ready"))
        }),
    ))
    .await;
    let started = std::time::Instant::now();
    let reading = CacheProxy::new(hung).read().await;
    assert!(matches!(reading, Reading::Unreachable(_)), "{reading:?}");
    assert!(started.elapsed() < Duration::from_secs(3), "bounded at 1 s");
}

/// The watch records what `/health` shows and counts its notices.
#[tokio::test]
async fn observe_records_health_and_counts_notices() {
    let url = stub(Router::new().route(
        super::STATUS_PATH,
        get(|| async { axum::Json(body("ready")) }),
    ))
    .await;
    let proxy = CacheProxy::new(url);
    assert_eq!(proxy.observe(&[], 10).await, None);
    let health = proxy.health();
    assert_eq!(health.state.as_deref(), Some("ready"));
    assert_eq!(health.reachable, Some(true));
    assert_eq!(health.checked_at, Some(10));
    assert_eq!(
        health.status.map(|status| status.counters.gh_launches),
        Some(1)
    );
    assert_eq!(health.notices, 0);
}

/// The watch tick end to end on a real store: a sidecar that is down under
/// a live proxied seat is told once to each live prime (a project's prime
/// and the designated prime, once each; a tombstoned prime gets nothing),
/// and the next tick with nothing changed tells no one (F192).
#[tokio::test]
async fn a_tick_tells_each_live_prime_once_and_then_nothing() {
    use pij_core::config::{AdapterChoice, Adapters, Config};
    use pij_core::orchestration::{PrimeDesignation, PrimeState, Project};

    let store = pij_testkit::FreshStore::new();
    let config = Config {
        adapters: Adapters {
            registry: AdapterChoice::Real,
            spine: AdapterChoice::Real,
            queue: AdapterChoice::Real,
            ..Adapters::default()
        },
        store_path: store.path(),
        ..Config::default()
    };
    let mut services = crate::build_services(
        &config,
        &std::path::Path::new(&store.path()).with_extension("signals"),
    )
    .await
    .expect("real store services");
    let closed = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    services.cache_proxy = std::sync::Arc::new(CacheProxy::new(format!(
        "http://{}",
        closed.local_addr().expect("addr")
    )));
    drop(closed);

    let mut proxied = SeatDescriptor::new("pij-proxied", Harness::Copilot, "/w");
    proxied.cache_proxy = Some(pij_core::model::SeatCacheProxy {
        state: CacheProxyRoute::Proxied,
        reason: "through".into(),
        fix: None,
    });
    let mut gone = SeatDescriptor::new("pij-gone-prime", Harness::Claude, "/w");
    gone.tombstoned_at = Some(1);
    for seat in [
        proxied,
        SeatDescriptor::new("pij-prime", Harness::Claude, "/w"),
        SeatDescriptor::new("pij-other-prime", Harness::Claude, "/w"),
        gone,
    ] {
        services.registry.put(seat).await.expect("seat");
    }
    let orchestration = services.directory.store();
    for (slug, prime) in [
        ("alpha", "pij-prime"),
        ("beta", "pij-other-prime"),
        ("gamma", "pij-gone-prime"),
    ] {
        orchestration
            .create_project(&Project {
                slug: slug.into(),
                description: None,
                repo: None,
                plan_path: None,
                prime_id: Some(prime.into()),
                created_by: "pij-human".into(),
                created_at: 1,
            })
            .await
            .expect("project");
    }
    services
        .watchdogs()
        .designate_prime(&PrimeDesignation {
            seat: "pij-prime".into(),
            designated_by: "pij-human".into(),
            designated_at: 1,
            state: PrimeState::Current,
        })
        .await
        .expect("prime");

    let pending = |seat: &'static str| {
        let delivery = std::sync::Arc::clone(&services.delivery);
        async move {
            delivery
                .pending_fyi_count(&seat.into())
                .await
                .expect("count")
        }
    };
    super::tick(&services).await;
    assert_eq!(pending("pij-prime").await, 1, "the designated prime, once");
    assert_eq!(pending("pij-other-prime").await, 1);
    assert_eq!(pending("pij-gone-prime").await, 0);
    assert_eq!(services.cache_proxy.health().notices, 1);
    assert_eq!(services.cache_proxy.health().seats.proxied, ["pij-proxied"]);

    super::tick(&services).await;
    assert_eq!(pending("pij-prime").await, 1, "no repeat while unchanged");
    assert_eq!(pending("pij-other-prime").await, 1);
}
