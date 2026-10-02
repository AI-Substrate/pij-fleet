//! u-report's RED WITNESS and its shape-parity suite (plan 114, ac-1144).
//!
//! The witness is deliberately NOT a compile error. Every test here compiles and
//! runs against the shipped `/v1/report` surface; before the handler existed they
//! failed because axum answered an absent route, which is a behavioural red at
//! the same altitude as the claim ac-1142 makes: *a card written via rs is
//! readable via rs*.
//!
//! Two properties of the surrounding system shape every test below, and both
//! were verified on tree rather than assumed:
//!
//! 1. `require_bearer` layers the WHOLE router (`http/mod.rs:214`), so auth runs
//!    BEFORE the route match. A keyless probe of `/v1/report` answers 401
//!    identically whether or not the handler exists — so a missing key and a
//!    missing route are indistinguishable from the client. Every test here sends
//!    the real key, and one test pins that a keyless call is 401 and therefore
//!    proves nothing about route presence.
//! 2. Wave 1's shim reads an empty-bodied 404/405 as ROUTE ABSENCE and falls back
//!    to legacy (`core/generation-routing.ts`, `classifyRsResponse`). A refusal of
//!    ours wearing that shape would become a SILENT RE-HOMING of the seat. So
//!    every refusal this handler emits carries a decodable envelope, and
//!    `refusals_are_never_classifiable_as_route_absence` pins it.

use std::net::SocketAddr;

use pij_core::anomalies::{
    AnomalyThresholds, AnomalyView, Detector, DoneFact, UnverifiedDoneDetector,
};
use pij_core::config::{AdapterChoice, Adapters, Config};
use pij_core::model::{Envelope, Harness, SeatDescriptor, SeatId, SemanticState, Seq};
use pij_core::report::{CARD_EVENT_KIND, CardRecord, ReportConfig, ReportService};
use pij_daemon::http::{HttpConfig, router_with_config};
use pij_testkit::FreshStore;

const KEY: &str = "test-local-key";
const SEAT: &str = "pij-report-witness";

/// A daemon on a fresh store with one registered seat, served over real HTTP.
///
/// Real adapters for registry and spine — the claim is DURABILITY ("cards are
/// durable and readable back"), and a fake spine would prove the handler calls a
/// method, not that a card survives. tmux and harness stay fake: a real tmux
/// adapter in a test taps the operator's live panes.
async fn daemon() -> (SocketAddr, tokio::task::JoinHandle<()>, FreshStore, Config) {
    let store = FreshStore::new();
    let config = Config {
        adapters: Adapters {
            registry: AdapterChoice::Real,
            spine: AdapterChoice::Real,
            queue: AdapterChoice::Real,
            liveness: AdapterChoice::Real,
            ..Adapters::default()
        },
        store_path: store.path(),
        ..Config::default()
    };
    let services = pij_daemon::build_services(
        &config,
        std::path::Path::new("/tmp/pij-test-pane-signals-report"),
    )
    .await
    .expect("services");
    services
        .registry
        .put(SeatDescriptor::new(SEAT, Harness::Claude, "/abs/tree"))
        .await
        .expect("seed the reporting seat");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    let router = router_with_config(
        services,
        HttpConfig {
            local_key: KEY.to_string(),
            peer_keys: Vec::new(),
            machine_alias: "workstation".to_string(),
        },
    );
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.expect("serve");
    });
    (addr, server, store, config)
}

/// POST one report exactly as the shim does: `{ "seat": …, "argv": [ … ] }`.
///
/// `argv` is `process.argv.slice(2)` verbatim — the shim passes what the caller
/// typed, untouched (`adapters/generation-router.ts`, `callRs`). `seat` is the
/// separate field this handler requires, because argv carries the message and
/// omits its author: `report now` is FIRST-PERSON and its subject is never typed.
async fn post_report(addr: SocketAddr, seat: Option<&str>, argv: &[&str]) -> reqwest::Response {
    let mut body = serde_json::json!({ "argv": argv });
    if let Some(seat) = seat {
        body["seat"] = serde_json::Value::String(seat.to_string());
    }
    reqwest::Client::new()
        .post(format!("http://{addr}/v1/report"))
        .bearer_auth(KEY)
        .json(&body)
        .send()
        .await
        .expect("post /v1/report")
}

/// Read the card back through core's own read path, from the store the daemon
/// actually wrote to — reopened, not the handle the handler held.
async fn read_card_back(config: &Config) -> Option<(String, String, bool)> {
    let services = pij_daemon::build_services(
        config,
        std::path::Path::new("/tmp/pij-test-pane-signals-report"),
    )
    .await
    .expect("reopen services");
    let reports = ReportService::new(
        services.registry.as_ref(),
        services.spine.as_ref(),
        || 0,
        ReportConfig::default(),
    );
    reports
        .card(&SeatId::from(SEAT))
        .await
        .expect("card read")
        .map(|status| (status.card.did, status.card.next, status.stale))
}

// ─── THE RED WITNESS ────────────────────────────────────────────────────────

/// ac-1142's core claim, at the altitude the AC states it: a card written
/// THROUGH rs is readable back FROM rs, durably.
///
/// Before the handler existed this failed on the first assertion — axum answered
/// 404 for an unrouted path — which is a behavioural red, not a compile error.
#[tokio::test]
async fn a_card_written_via_rs_is_readable_via_rs() {
    let (addr, server, _store, config) = daemon().await;

    let response = post_report(
        addr,
        Some(SEAT),
        &["report", "now", "shipped x", "review y"],
    )
    .await;
    assert_eq!(
        response.status(),
        reqwest::StatusCode::OK,
        "POST /v1/report must be served by rs"
    );
    let envelope: Envelope<serde_json::Value> = response.json().await.expect("envelope");
    assert!(envelope.ok, "a successful report reports success");

    // Reopened from disk: proves DURABILITY, not that a method was called.
    assert_eq!(
        read_card_back(&config).await,
        Some(("shipped x".to_string(), "review y".to_string(), false)),
        "the card must survive the write and read back with its own freshness"
    );

    server.abort();
}

// ─── SHAPE PARITY, each row citing the TS source it was derived from ────────

/// TS `normalizeReportText` caps did/next at 280 chars AFTER whitespace
/// collapsing — `.pi/extensions/pij/core/cli.ts:815` (`REPORT_TEXT_MAX_LENGTH`)
/// and `:824`. rs's own `CARD_LIMIT` (`crates/core/src/model.rs:293`) is the same
/// 280, and this test cites the CONSTANT rather than the literal, so a change to
/// either side breaks here instead of drifting silently.
#[tokio::test]
async fn did_and_next_are_capped_at_the_limit_core_defines() {
    let (addr, server, _store, config) = daemon().await;

    let at_limit = "a".repeat(pij_core::model::CARD_LIMIT);
    assert_eq!(
        post_report(addr, Some(SEAT), &["report", "now", &at_limit, "next"])
            .await
            .status(),
        reqwest::StatusCode::OK,
        "exactly at the limit is accepted — the boundary is inclusive on both sides"
    );

    let over_limit = "a".repeat(pij_core::model::CARD_LIMIT + 1);
    let refusal = post_report(addr, Some(SEAT), &["report", "now", &over_limit, "next"]).await;
    assert_eq!(refusal.status(), reqwest::StatusCode::BAD_REQUEST);
    let envelope: Envelope<serde_json::Value> = refusal.json().await.expect("envelope");
    assert!(!envelope.ok);
    let meta = envelope.meta.unwrap_or_default();
    assert!(
        meta.contains("280"),
        "the refusal must NAME the limit it enforced, not just refuse: {meta}"
    );

    // The over-limit attempt wrote nothing.
    assert_eq!(
        read_card_back(&config).await.map(|card| card.0),
        Some(at_limit),
        "a refused report must not overwrite the last good card"
    );

    server.abort();
}

/// Whitespace is COLLAPSED before the limit is applied — TS `core/cli.ts:821`
/// (`input.trim().replace(/\s+/g, " ")`), and rs `collapse_whitespace`
/// (`crates/core/src/report.rs`). A caller whose text is only long because of
/// runs of spaces is not refused by either generation.
#[tokio::test]
async fn whitespace_collapses_before_the_limit_is_applied() {
    let (addr, server, _store, config) = daemon().await;

    let padded = format!("  did{}text  ", " ".repeat(400));
    assert_eq!(
        post_report(
            addr,
            Some(SEAT),
            &["report", "now", &padded, " next  value "]
        )
        .await
        .status(),
        reqwest::StatusCode::OK,
        "400 spaces collapse to one; the payload is well under the limit"
    );
    assert_eq!(
        read_card_back(&config).await,
        Some(("did text".to_string(), "next value".to_string(), false))
    );

    server.abort();
}

/// TS refuses an EMPTY did/next by name — `core/cli.ts:822`, "report did must not
/// be empty". rs core deliberately PERMITS an empty card (`report.rs`: "Empty is
/// deliberately valid: a present empty card and no card are different facts").
///
/// Both are right at their own altitude, so the divergence is settled at the
/// BOUNDARY, not by overruling either one: the caller-facing shape matches TS,
/// and core's deliberate choice is left standing for any in-process caller.
#[tokio::test]
async fn an_empty_card_field_is_refused_by_name_as_ts_refuses_it() {
    let (addr, server, _store, _config) = daemon().await;

    for argv in [
        ["report", "now", "", "next"],
        ["report", "now", "did", "   "],
    ] {
        let refusal = post_report(addr, Some(SEAT), &argv).await;
        assert_eq!(refusal.status(), reqwest::StatusCode::BAD_REQUEST);
        let envelope: Envelope<serde_json::Value> = refusal.json().await.expect("envelope");
        assert!(
            envelope
                .meta
                .unwrap_or_default()
                .contains("must not be empty"),
            "the refusal must match the wording a caller already knows"
        );
    }

    server.abort();
}

/// TS refuses an embedded newline by name — `core/cli.ts:819`, "report did must
/// be one line" — where rs's `collapse_whitespace` would silently fold it into a
/// space. Silently differing is the outcome ac-1144 forbids most explicitly.
#[tokio::test]
async fn an_embedded_newline_is_refused_rather_than_silently_collapsed() {
    let (addr, server, _store, _config) = daemon().await;

    let refusal = post_report(
        addr,
        Some(SEAT),
        &["report", "now", "line one\nline two", "next"],
    )
    .await;
    assert_eq!(refusal.status(), reqwest::StatusCode::BAD_REQUEST);
    let envelope: Envelope<serde_json::Value> = refusal.json().await.expect("envelope");
    assert!(
        envelope
            .meta
            .unwrap_or_default()
            .contains("must be one line"),
        "collapsing a newline into a space would change the caller's text without saying so"
    );

    server.abort();
}

/// `report blocked "<text>"` and `report question "<text>"` set the semantic
/// state AND retain their note — TS `core/cli.ts:1689-1706`, rs
/// `ReportService::blocked`/`question`.
#[tokio::test]
async fn blocked_and_question_set_the_state_and_retain_the_note() {
    let (addr, server, _store, config) = daemon().await;

    for (leaf, expected) in [
        ("blocked", SemanticState::Blocked),
        ("question", SemanticState::Question),
    ] {
        assert_eq!(
            post_report(addr, Some(SEAT), &["report", leaf, "waiting on the ruling"])
                .await
                .status(),
            reqwest::StatusCode::OK,
        );
        let services = pij_daemon::build_services(
            &config,
            std::path::Path::new("/tmp/pij-test-pane-signals-report"),
        )
        .await
        .expect("reopen");
        let descriptor = services
            .registry
            .get(&SeatId::from(SEAT))
            .await
            .expect("registry read")
            .expect("seat");
        assert_eq!(descriptor.semantic_state, Some(expected));
        let reports = ReportService::new(
            services.registry.as_ref(),
            services.spine.as_ref(),
            || 0,
            ReportConfig::default(),
        );
        let record = reports
            .latest_state_record(&SeatId::from(SEAT))
            .await
            .expect("state record")
            .expect("a record");
        assert_eq!(record.note.as_deref(), Some("waiting on the ruling"));
    }

    server.abort();
}

/// TS caps the blocked/question note at 200 — `core/cli.ts:816`
/// (`REPORT_NOTE_MAX_LENGTH`) and `:841` — a DIFFERENT limit from the 280 that
/// governs did/next. rs core applies no note limit at all, so the parity is
/// enforced here. The two limits differ, and a test that only exercised 280
/// would pass with the note limit deleted.
#[tokio::test]
async fn the_note_limit_is_200_and_is_not_the_card_limit() {
    let (addr, server, _store, _config) = daemon().await;

    let note = "n".repeat(201);
    let refusal = post_report(addr, Some(SEAT), &["report", "blocked", &note]).await;
    assert_eq!(refusal.status(), reqwest::StatusCode::BAD_REQUEST);
    let meta = refusal
        .json::<Envelope<serde_json::Value>>()
        .await
        .expect("envelope")
        .meta
        .unwrap_or_default();
    assert!(
        meta.contains("200"),
        "the refusal must name the note limit: {meta}"
    );

    // The discriminating arm: 201 chars is under the 280 CARD limit, so a handler
    // that reused the card limit for notes would accept this and pass every test
    // above. Only this one separates the two.
    assert!(note.chars().count() < pij_core::model::CARD_LIMIT);

    server.abort();
}

/// `report clear` removes the declaration — TS `core/cli.ts:1746`, rs
/// `ReportService::clear`.
#[tokio::test]
async fn clear_removes_the_declared_state() {
    let (addr, server, _store, config) = daemon().await;

    post_report(addr, Some(SEAT), &["report", "blocked", "on a peer"]).await;
    assert_eq!(
        post_report(addr, Some(SEAT), &["report", "clear"])
            .await
            .status(),
        reqwest::StatusCode::OK
    );
    let services = pij_daemon::build_services(
        &config,
        std::path::Path::new("/tmp/pij-test-pane-signals-report"),
    )
    .await
    .expect("reopen");
    assert_eq!(
        services
            .registry
            .get(&SeatId::from(SEAT))
            .await
            .expect("read")
            .expect("seat")
            .semantic_state,
        None
    );

    server.abort();
}

/// `report state <state>` accepts every word BOTH generations share — TS
/// `SEMANTIC_STATES` (`core/types.ts:110`) intersected with rs `SemanticState`
/// (`crates/core/src/model.rs:104`).
#[tokio::test]
async fn report_state_accepts_every_word_the_two_generations_share() {
    let (addr, server, _store, config) = daemon().await;

    for (word, expected) in [
        ("ready", SemanticState::Ready),
        ("waiting", SemanticState::Waiting),
        ("hold", SemanticState::Hold),
        ("blocked", SemanticState::Blocked),
        ("question", SemanticState::Question),
        ("done", SemanticState::Done),
    ] {
        assert_eq!(
            post_report(addr, Some(SEAT), &["report", "state", word])
                .await
                .status(),
            reqwest::StatusCode::OK,
            "'{word}' is in both vocabularies and must be honoured"
        );
        let services = pij_daemon::build_services(
            &config,
            std::path::Path::new("/tmp/pij-test-pane-signals-report"),
        )
        .await
        .expect("reopen");
        assert_eq!(
            services
                .registry
                .get(&SeatId::from(SEAT))
                .await
                .expect("read")
                .expect("seat")
                .semantic_state,
            Some(expected)
        );
    }

    server.abort();
}

#[tokio::test]
async fn report_state_accepts_failed_and_cancelled_and_preserves_them_after_reopen() {
    let (addr, server, _store, config) = daemon().await;
    for word in ["failed", "cancelled"] {
        let response = post_report(addr, Some(SEAT), &["report", "state", word]).await;
        assert_eq!(response.status(), reqwest::StatusCode::OK, "{word}");
        let envelope: Envelope<serde_json::Value> = response.json().await.expect("report envelope");
        assert!(envelope.ok);

        let response = post_state(addr, SEAT).await;
        let envelope: Envelope<serde_json::Value> = response.json().await.expect("state envelope");
        assert_eq!(envelope.data.expect("state")["semanticState"], word);

        let services = pij_daemon::build_services(
            &config,
            std::path::Path::new("/tmp/pij-test-pane-signals-report"),
        )
        .await
        .expect("reopen");
        let seat = services
            .registry
            .get(&SeatId::from(SEAT))
            .await
            .expect("read")
            .expect("seat");
        assert_eq!(serde_json::to_value(seat).unwrap()["semantic_state"], word);
    }
    server.abort();
}

// ─── REFUSAL BY NAME — the shapes rs cannot honour ──────────────────────────

/// Unsupported scoping refuses instead of silently writing a different subject.
#[tokio::test]
async fn unsupported_report_scopes_refuse_by_name() {
    let (addr, server, _store, _config) = daemon().await;

    for argv in [
        vec!["report", "now", "did", "next", "--project", "some-slug"],
        vec!["report", "now", "did", "next", "--for", "pij-other"],
    ] {
        let flag = argv
            .iter()
            .find(|token| token.starts_with("--"))
            .expect("a flag under test");
        let refusal = post_report(addr, Some(SEAT), &argv).await;
        assert_eq!(
            refusal.status(),
            reqwest::StatusCode::BAD_REQUEST,
            "{flag} must refuse, never be silently dropped"
        );
        let meta = refusal
            .json::<Envelope<serde_json::Value>>()
            .await
            .expect("envelope")
            .meta
            .unwrap_or_default();
        assert!(
            meta.contains(flag),
            "the refusal must name the flag it cannot honour: {meta}"
        );
    }

    server.abort();
}

/// Verification exists, but an unknown target must still return a named,
/// decodable refusal rather than being mistaken for route absence.
#[tokio::test]
async fn verify_refuses_unknown_target_with_decodable_evidence() {
    let (addr, server, _store, _config) = daemon().await;

    let refusal = post_report(addr, Some(SEAT), &["report", "verify", "nd-1"]).await;
    assert_eq!(refusal.status(), reqwest::StatusCode::BAD_REQUEST);
    let body = refusal.json::<serde_json::Value>().await.expect("envelope");
    assert_eq!(body["details"]["code"], "E-RS-NO-SEAT");
    assert_eq!(body["details"]["seat"], "nd-1");

    server.abort();
}

/// A seat rs does not hold is refused, never written for.
///
/// TS does exactly this — `resolveReportingSelf` answers `E-NOID` "reporting seat
/// 'x' is not registered" (`core/cli.ts:1997`) — and here it is also the
/// split-brain guard: `ReportService::now` appends to the spine WITHOUT
/// consulting the registry, so without this check a card for a legacy-resident
/// seat would land in the rs spine under a seat id rs has never heard of, with
/// every surface reporting success.
#[tokio::test]
async fn a_seat_rs_does_not_hold_is_refused_and_no_card_is_written() {
    let (addr, server, _store, config) = daemon().await;

    let refusal = post_report(
        addr,
        Some("pij-lives-in-legacy"),
        &["report", "now", "a", "b"],
    )
    .await;
    assert_eq!(refusal.status(), reqwest::StatusCode::BAD_REQUEST);
    let meta = refusal
        .json::<Envelope<serde_json::Value>>()
        .await
        .expect("envelope")
        .meta
        .unwrap_or_default();
    assert!(
        meta.contains("pij-lives-in-legacy"),
        "the refusal must name the seat it does not hold: {meta}"
    );

    let services = pij_daemon::build_services(
        &config,
        std::path::Path::new("/tmp/pij-test-pane-signals-report"),
    )
    .await
    .expect("reopen");
    let events = services
        .spine
        .tail(Some(&SeatId::from("pij-lives-in-legacy")), Seq(0))
        .await
        .expect("tail");
    assert!(
        events.iter().all(|event| event.kind != CARD_EVENT_KIND),
        "no card may be written for a seat rs does not hold"
    );

    server.abort();
}

/// argv carries the message and omits its author, so the seat is a separate
/// field — and its ABSENCE is a named refusal rather than a guess.
#[tokio::test]
async fn a_report_with_no_seat_is_refused_by_name_never_attributed_by_guess() {
    let (addr, server, _store, _config) = daemon().await;

    let refusal = post_report(addr, None, &["report", "now", "a", "b"]).await;
    assert_eq!(refusal.status(), reqwest::StatusCode::BAD_REQUEST);
    let meta = refusal
        .json::<Envelope<serde_json::Value>>()
        .await
        .expect("envelope")
        .meta
        .unwrap_or_default();
    assert!(meta.contains("seat"), "{meta}");

    server.abort();
}

// ─── THE TWO SYSTEM PROPERTIES, PINNED ──────────────────────────────────────

/// THE BINDING RULE: no refusal of ours may wear route-absence's clothes.
///
/// Wave 1's shim classifies "404 or 405 with a body that does not decode as a pij
/// envelope" as `legacy-rs-not-implemented` and falls back to legacy. A refusal
/// shaped like that would silently re-home the seat. This replays that exact
/// classification over every refusal the handler can emit.
///
/// It is non-vacuous by construction: it asserts the two facts the shim actually
/// branches on — the status is not a bare 404/405, AND the body decodes as an
/// envelope carrying `ok: false` — so a handler that started answering a bare 404
/// for any of these fails here and only here.
#[tokio::test]
async fn refusals_are_never_classifiable_as_route_absence() {
    let (addr, server, _store, _config) = daemon().await;

    let refusals: Vec<Vec<&str>> = vec![
        vec!["report", "verify", "nd-1"],
        vec!["report", "now", "did", "next", "--project", "slug"],
        vec!["report", "now", "", "next"],
        vec!["report", "now", "did\nnext", "x"],
        vec!["report", "state", "not-a-state"],
        vec!["report", "now", "only-one-positional"],
        vec!["report", "nonsense"],
    ];

    for argv in refusals {
        let response = post_report(addr, Some(SEAT), &argv).await;
        let status = response.status();
        let body = response.text().await.expect("body");

        // The shim's test 1: an empty-bodied 404/405 means "route absent".
        let looks_absent = (status == reqwest::StatusCode::NOT_FOUND
            || status == reqwest::StatusCode::METHOD_NOT_ALLOWED)
            && serde_json::from_str::<Envelope<serde_json::Value>>(&body).is_err();
        assert!(
            !looks_absent,
            "{argv:?} refused in a shape the shim reads as route-absence — it would fall back to legacy and re-home the seat"
        );

        // The shim's test 2: the body must be something an rs handler wrote.
        let envelope: Envelope<serde_json::Value> =
            serde_json::from_str(&body).unwrap_or_else(|error| {
                panic!("{argv:?} refusal body must decode as a pij envelope ({error}): {body}")
            });
        assert!(!envelope.ok, "{argv:?} must refuse, not succeed");
        assert!(
            envelope.error.is_some(),
            "{argv:?} refusal must carry a branchable ErrorKind, not prose alone"
        );
    }

    server.abort();
}

/// Auth runs ABOVE the route match (`http/mod.rs:214`), so a keyless probe
/// answers 401 whether or not `/v1/report` exists.
///
/// Pinned because it is the trap this unit was warned about and the reason no
/// test here concludes anything from a keyless call: during development a 401 on
/// a brand-new endpoint reads exactly like a missing handler.
#[tokio::test]
async fn a_keyless_probe_is_401_and_therefore_proves_nothing_about_the_route() {
    let (addr, server, _store, _config) = daemon().await;

    let keyless = reqwest::Client::new()
        .post(format!("http://{addr}/v1/report"))
        .json(&serde_json::json!({ "seat": SEAT, "argv": ["report", "now", "a", "b"] }))
        .send()
        .await
        .expect("keyless post");
    assert_eq!(keyless.status(), reqwest::StatusCode::UNAUTHORIZED);

    // The same 401 answers a path that certainly does not exist — which is the
    // whole point: status alone cannot tell a missing key from a missing route.
    let absent = reqwest::Client::new()
        .post(format!("http://{addr}/v1/definitely-not-a-route"))
        .send()
        .await
        .expect("keyless post to nothing");
    assert_eq!(absent.status(), reqwest::StatusCode::UNAUTHORIZED);

    server.abort();
}

/// The staleness threshold core already specifies is the one a card is judged by
/// — `ReportConfig::stale_after_ms` defaults to 10 minutes
/// (`crates/core/src/report.rs`), and equality with the threshold is FRESH.
///
/// Written through the HTTP surface and judged by core's reader, so the daemon
/// cannot acquire a second opinion about staleness.
#[tokio::test]
async fn staleness_is_judged_by_the_threshold_core_specifies() {
    let (addr, server, _store, config) = daemon().await;

    post_report(addr, Some(SEAT), &["report", "now", "did", "next"]).await;

    let services = pij_daemon::build_services(
        &config,
        std::path::Path::new("/tmp/pij-test-pane-signals-report"),
    )
    .await
    .expect("reopen");
    let written_at = services
        .spine
        .tail(Some(&SeatId::from(SEAT)), Seq(0))
        .await
        .expect("tail")
        .into_iter()
        .rev()
        .find(|event| event.kind == CARD_EVENT_KIND)
        .expect("a card event")
        .at;

    let threshold = ReportConfig::default().stale_after_ms;
    for (clock, expected_stale, why) in [
        (
            written_at + threshold,
            false,
            "equality with the threshold is fresh",
        ),
        (written_at + threshold + 1, true, "strictly older is stale"),
    ] {
        let reports = ReportService::new(
            services.registry.as_ref(),
            services.spine.as_ref(),
            move || clock,
            ReportConfig::default(),
        );
        let status = reports
            .card(&SeatId::from(SEAT))
            .await
            .expect("card")
            .expect("a card");
        assert_eq!(status.stale, expected_stale, "{why}");
    }

    server.abort();
}

/// The card the handler wrote carries the fields core defines, decodable as
/// core's own `CardRecord` — the durable payload, not a shape the handler
/// invented beside it.
#[tokio::test]
async fn the_durable_payload_is_cores_card_record() {
    let (addr, server, _store, config) = daemon().await;

    post_report(
        addr,
        Some(SEAT),
        &["report", "now", "did text", "next text"],
    )
    .await;

    let services = pij_daemon::build_services(
        &config,
        std::path::Path::new("/tmp/pij-test-pane-signals-report"),
    )
    .await
    .expect("reopen");
    let event = services
        .spine
        .tail(Some(&SeatId::from(SEAT)), Seq(0))
        .await
        .expect("tail")
        .into_iter()
        .rev()
        .find(|event| event.kind == CARD_EVENT_KIND)
        .expect("a card event");
    let record: CardRecord = serde_json::from_str(&event.payload).expect("core's CardRecord");
    assert_eq!(record.did, "did text");
    assert_eq!(record.next, "next text");
    assert_eq!(event.seat, Some(SeatId::from(SEAT)));

    server.abort();
}

/// The seat may arrive as the forwarded CALLER CONTEXT instead of an explicit
/// field — the shape the shim sends, since `report` is first-person and its
/// subject is never typed, so the caller forwards what only it can see.
///
/// Both spellings are exercised because the key crosses a runtime boundary by
/// NAME: TypeScript writes it, Rust reads it, and nothing but agreement between
/// two files keeps them matching. A rename on either side would present as a
/// green suite and a fleet that cannot report.
#[tokio::test]
async fn the_seat_may_arrive_as_forwarded_caller_context() {
    let (addr, server, _store, config) = daemon().await;

    for (key, did) in [
        ("PIJ_SESSION_ID", "via env spelling"),
        ("pij_session_id", "via rust spelling"),
    ] {
        let response = reqwest::Client::new()
            .post(format!("http://{addr}/v1/report"))
            .bearer_auth(KEY)
            .json(&serde_json::json!({
                "caller": { key: SEAT },
                "argv": ["report", "now", did, "next"],
            }))
            .send()
            .await
            .expect("post");
        assert_eq!(
            response.status(),
            reqwest::StatusCode::OK,
            "spelling '{key}'"
        );
        assert_eq!(
            read_card_back(&config).await.map(|card| card.0),
            Some(did.to_string()),
            "the card must be attributed to the forwarded seat"
        );
    }

    // A forwarded seat is a CLAIM, not an identity: one rs does not hold is
    // refused exactly as an explicit one is.
    let refusal = reqwest::Client::new()
        .post(format!("http://{addr}/v1/report"))
        .bearer_auth(KEY)
        .json(&serde_json::json!({
            "caller": { "PIJ_SESSION_ID": "pij-not-in-this-store" },
            "argv": ["report", "now", "a", "b"],
        }))
        .send()
        .await
        .expect("post");
    assert_eq!(refusal.status(), reqwest::StatusCode::BAD_REQUEST);

    server.abort();
}

/// The answer carries the human sentence the TypeScript CLI prints today, so the
/// routed path can render one without re-deriving the wording from fields — a
/// second copy of the sentence on the far side of the wire is a second thing to
/// drift.
#[tokio::test]
async fn the_receipt_carries_the_line_the_typescript_cli_prints() {
    let (addr, server, _store, _config) = daemon().await;

    let payload: Envelope<serde_json::Value> = post_report(
        addr,
        Some(SEAT),
        &["report", "now", "did text", "next text"],
    )
    .await
    .json()
    .await
    .expect("envelope");
    let line = payload.data.expect("data")["line"]
        .as_str()
        .expect("a line")
        .to_string();
    assert!(line.starts_with(&format!("reported by {SEAT}: ")), "{line}");
    assert!(
        line.contains("\"did text\" \u{2192} \"next text\""),
        "{line}"
    );
    assert!(line.contains("(spine "), "{line}");

    server.abort();
}

// ─── ac-1142's READ SURFACE (u-readback) ────────────────────────────────────
//
// `a_card_written_via_rs_is_readable_via_rs` above proves the card is DURABLE:
// it reopens the store and reads through core's own path. That is the write
// half. ac-1142 makes a second, separate claim — "`pij state <id>` returns it" —
// and it names the surface an OPERATOR uses. A card that only core can read back
// is not a card the operator can read back.
//
// Nothing here re-proves durability; these read the card through the shipped
// `/v1/state` route, which is the only thing the AC's second clause is about.

async fn post_state(addr: SocketAddr, seat: &str) -> reqwest::Response {
    reqwest::Client::new()
        .post(format!("http://{addr}/v1/state"))
        .bearer_auth(KEY)
        .json(&serde_json::json!({ "id": seat }))
        .send()
        .await
        .expect("post /v1/state")
}

/// RED WITNESS for the readback half of ac-1142.
///
/// Chronologically red on the composed tree: `report now` reaches rs and the
/// card lands in the spine (witnessed live in sqlite `spine_events`), and
/// `/v1/state` answers 200 with a good descriptor projection — but that
/// projection has no card in it, so the operator-facing half of the AC is
/// unmet. This fails on the card assertions, not on a missing route and not on a
/// compile error.
#[tokio::test]
async fn a_card_written_via_rs_is_read_back_by_state() {
    let (addr, server, _store, _config) = daemon().await;

    let written = post_report(
        addr,
        Some(SEAT),
        &["report", "now", "landed the readback", "await composition"],
    )
    .await;
    assert_eq!(written.status(), reqwest::StatusCode::OK);

    let response = post_state(addr, SEAT).await;
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let envelope: Envelope<serde_json::Value> = response.json().await.expect("state envelope");
    let card = envelope.data.expect("state data");

    // TS spells the card's two halves `statusPrev` (what was done) and
    // `statusNext` (what is next) — `.pi/extensions/pij/core/cli.ts:5752-5753`
    // (`node show`) and `:2878-2879` (`list`). Same names here, so a consumer
    // that already parses either surface parses this one unchanged.
    assert_eq!(card["statusPrev"], "landed the readback");
    assert_eq!(card["statusNext"], "await composition");
    // `statusAt` (cli.ts:5754) and `statusSeq` (cli.ts:5755): when the card was
    // written, and the spine sequence that carries it.
    assert!(
        card["statusAt"].as_u64().is_some_and(|at| at > 0),
        "a card that was written has a time it was written at"
    );
    assert!(
        card["statusSeq"].as_u64().is_some(),
        "the card's spine sequence is what makes it citable"
    );
    // Freshness is computed ON READ, as core defines it — not stored.
    assert_eq!(card["statusStale"], false);

    server.abort();
}

/// A seat that has never reported is DISTINGUISHABLE from one whose card failed
/// to load — and from one carrying an empty card.
///
/// `null` here is a real answer: "this seat exists and has never written a
/// card". It is the one case where a null is honest, because rs DID look and
/// there genuinely is none — unlike the `unsupported` fields, where rs cannot
/// look at all.
#[tokio::test]
async fn a_seat_that_never_reported_reads_back_a_null_card_not_an_empty_one() {
    let (addr, server, _store, _config) = daemon().await;

    let response = post_state(addr, SEAT).await;
    let envelope: Envelope<serde_json::Value> = response.json().await.expect("state envelope");
    let card = envelope.data.expect("state data");

    assert!(card["statusPrev"].is_null());
    assert!(card["statusNext"].is_null());
    assert!(card["statusAt"].is_null());
    assert!(card["statusSeq"].is_null());
    // Not `false`: staleness of a card that does not exist is not "fresh".
    assert!(
        card["statusStale"].is_null(),
        "a seat with no card is not a seat with a FRESH card"
    );
    // And the keys are PRESENT, so a consumer can tell "no card" from "this
    // build does not report cards" without guessing.
    for key in [
        "statusPrev",
        "statusNext",
        "statusAt",
        "statusSeq",
        "statusStale",
    ] {
        assert!(card.get(key).is_some(), "{key} must be present as null");
    }

    server.abort();
}

/// The declared semantic state and its note ride back too — `report blocked`
/// and `report question` set both, and an operator reading `state` is asking
/// exactly the question they answer.
///
/// TS surfaces the note beside the state on the same card
/// (`.pi/extensions/pij/core/cli.ts:5794` renders the report line).
#[tokio::test]
async fn state_reads_back_the_declared_state_and_its_note() {
    let (addr, server, _store, _config) = daemon().await;

    let declared = post_report(
        addr,
        Some(SEAT),
        &["report", "blocked", "waiting on the seam unit"],
    )
    .await;
    assert_eq!(declared.status(), reqwest::StatusCode::OK);

    let response = post_state(addr, SEAT).await;
    let envelope: Envelope<serde_json::Value> = response.json().await.expect("state envelope");
    let card = envelope.data.expect("state data");

    assert_eq!(card["semanticState"], "blocked");
    assert_eq!(card["stateNote"], "waiting on the seam unit");

    server.abort();
}

// ─── THE PANE-DERIVED SUBJECT (plan 117) ────────────────────────────────────

/// A hand-started seat forwards a PANE and no session id. `report` must name it.
///
/// THE RED THIS PINS, observed live on 2026-09-01 against the running daemon at
/// 127.0.0.1:7461 for seat `pij-dominant-vicuna` (pane `%255`, a live rs row):
/// two POSTs, byte-identical caller context — one pane, no session id — answered
/// differently by the same daemon in the same instant.
///
///   POST /v1/whoami {"caller":{"tmuxPane":"%255"}} -> 200, `pij-dominant-vicuna`
///   POST /v1/report {"caller":{"tmuxPane":"%255"}} -> 400, "no reporting seat
///                                                     was supplied"
///
/// The cause is not a routing fault and not a forwarding fault: the shim sent
/// everything it had, and the pane it sent identifies the seat unambiguously in
/// rs's OWN roster. `whoami`/`state`/`phonehome` resolve through the shared
/// identity ladder (`identity::resolve_seat`), which takes a pane as its
/// STRICTEST evidence; `report` had its own private resolution that read an
/// asserted id and nothing else. So `report` was the one routed verb with no
/// derivation path — it could only be ASSERTED. `PIJ_SESSION_ID` is unset on
/// every hand-started seat, which is the normal case on this fleet and not an
/// edge one, so the whole report family was refused for every rs-resident seat a
/// human started by hand — precisely the population that owes status cards.
///
/// This test asserts BOTH halves on purpose. The `whoami` half is the control:
/// without it, a future change that broke pane resolution everywhere would leave
/// this test failing for a reason it does not name, and a one-ended assertion
/// cannot tell "report ignores the pane" from "the pane never identified
/// anything". The two calls must agree, and the test says so in those terms.
#[tokio::test]
async fn a_pane_only_caller_is_named_by_report_exactly_as_it_is_by_whoami() {
    let (addr, server, _store, config) = daemon().await;

    // Seed a seat that owns a pane — the hand-started shape. Fresh id so this
    // test never leans on SEAT's paneless row.
    const PANED: &str = "pij-report-pane-witness";
    const PANE: &str = "%255";
    let services = pij_daemon::build_services(
        &config,
        std::path::Path::new("/tmp/pij-test-pane-signals-report"),
    )
    .await
    .expect("services");
    let mut descriptor = SeatDescriptor::new(PANED, Harness::Claude, "/abs/tree");
    descriptor.pane = Some(PANE.to_string());
    services
        .registry
        .put(descriptor)
        .await
        .expect("seed the paned seat");

    let caller = serde_json::json!({ "tmuxPane": PANE });

    // CONTROL: the ladder resolves this exact caller today.
    let whoami: Envelope<serde_json::Value> = reqwest::Client::new()
        .post(format!("http://{addr}/v1/whoami"))
        .bearer_auth(KEY)
        .json(&serde_json::json!({ "caller": caller, "argv": ["whoami"] }))
        .send()
        .await
        .expect("post /v1/whoami")
        .json()
        .await
        .expect("whoami envelope");
    assert_eq!(
        whoami.data.expect("whoami data")["id"],
        PANED,
        "control: the pane must identify the seat through the shared ladder"
    );

    // THE CLAIM: report is handed the same evidence and must reach the same seat.
    let response = reqwest::Client::new()
        .post(format!("http://{addr}/v1/report"))
        .bearer_auth(KEY)
        .json(&serde_json::json!({
            "caller": caller,
            "argv": ["report", "now", "derived from the pane", "next"],
        }))
        .send()
        .await
        .expect("post /v1/report");
    assert_eq!(
        response.status(),
        reqwest::StatusCode::OK,
        "report must derive its subject from the pane the caller forwarded, as \
         whoami already does with the very same caller context"
    );

    let payload: Envelope<serde_json::Value> = response.json().await.expect("report envelope");
    assert_eq!(
        payload.data.expect("report data")["seat"],
        PANED,
        "the card must be attributed to the seat the pane names"
    );

    server.abort();
}

/// An asserted id that CONTRADICTS the observable pane is refused — both of them.
///
/// The pane-derived fix must not become a wider door. The ladder's rule is that
/// an asserted id never outranks an observable one, and that a disagreement is
/// refused rather than resolved in either direction; a card is the one artifact
/// where attributing to the wrong seat is worse than not writing it. This is the
/// guard that stops "derive from the pane" being read as "prefer whichever
/// identity answers", so it is pinned rather than left to the ladder's own
/// tests: the ladder could keep this property while `report` stopped calling it,
/// and nothing here would notice.
#[tokio::test]
async fn report_refuses_when_the_asserted_seat_and_the_pane_disagree() {
    let (addr, server, _store, config) = daemon().await;

    const PANED: &str = "pij-report-pane-owner";
    const PANE: &str = "%909";
    let services = pij_daemon::build_services(
        &config,
        std::path::Path::new("/tmp/pij-test-pane-signals-report"),
    )
    .await
    .expect("services");
    let mut descriptor = SeatDescriptor::new(PANED, Harness::Claude, "/abs/tree");
    descriptor.pane = Some(PANE.to_string());
    services
        .registry
        .put(descriptor)
        .await
        .expect("seed the paned seat");

    // SEAT exists in this store too, so this is a live-vs-live disagreement and
    // not merely an unknown id.
    let response = reqwest::Client::new()
        .post(format!("http://{addr}/v1/report"))
        .bearer_auth(KEY)
        .json(&serde_json::json!({
            "seat": SEAT,
            "caller": { "tmuxPane": PANE },
            "argv": ["report", "now", "should not land", "anywhere"],
        }))
        .send()
        .await
        .expect("post /v1/report");
    assert_eq!(
        response.status(),
        reqwest::StatusCode::BAD_REQUEST,
        "a claim contradicting an observable pane must refuse, never pick a side"
    );

    // And the refusal is REAL: neither candidate got a card.
    assert_eq!(
        read_card_back(&config).await,
        None,
        "the refused report must not have been written to the asserted seat"
    );

    server.abort();
}

#[tokio::test]
async fn explicit_report_claim_cannot_hide_a_conflicting_caller_session() {
    let (addr, server, _store, config) = daemon().await;
    let services = pij_daemon::build_services(
        &config,
        std::path::Path::new("/tmp/pij-test-pane-signals-report"),
    )
    .await
    .expect("services");
    let mut other = SeatDescriptor::new("pij-other-reporter", Harness::Claude, "/abs/tree");
    other.pane = Some("%910".to_string());
    services
        .registry
        .put(other)
        .await
        .expect("second live actor");
    for caller in [
        serde_json::json!({"PIJ_SESSION_ID": SEAT}),
        serde_json::json!({"PIJ_SESSION_ID": SEAT, "TMUX_PANE": "%910"}),
    ] {
        let response = reqwest::Client::new()
            .post(format!("http://{addr}/v1/report"))
            .bearer_auth(KEY)
            .json(&serde_json::json!({
                "seat": "pij-other-reporter", "caller": caller,
                "argv": ["report", "now", "must not land", "anywhere"],
            }))
            .send()
            .await
            .expect("report");
        assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
        let refusal: serde_json::Value = response.json().await.expect("refusal envelope");
        assert_eq!(refusal["ok"], false);
        let reason = refusal["meta"].as_str().expect("reason");
        assert!(reason.contains(SEAT) && reason.contains("pij-other-reporter"));
    }
    let events = services
        .spine
        .tail(None, pij_core::model::Seq(0))
        .await
        .expect("history");
    assert!(events.iter().all(|event| event.kind != "report.now"));
    server.abort();
}

async fn governance_request(addr: SocketAddr, family: &str, argv: &[&str]) -> serde_json::Value {
    let response = reqwest::Client::new()
        .post(format!("http://{addr}/v1/{family}"))
        .bearer_auth(KEY)
        .json(&serde_json::json!({"caller":{"PIJ_SESSION_ID":SEAT},"argv":argv}))
        .send()
        .await
        .expect("governance request");
    let status = response.status();
    let body: serde_json::Value = response.json().await.expect("governance envelope");
    assert_eq!(status, reqwest::StatusCode::OK, "{body}");
    body["data"].clone()
}

#[tokio::test]
async fn assignment_reports_survive_reopen_and_reach_node_and_state_readers() {
    let (addr, server, _store, config) = daemon().await;
    let task = governance_request(addr, "task", &["task", "set", SEAT, "prove reports"]).await;
    let id = task["task"]["id"].as_str().expect("task id");
    for (leaf, value, expected_state, note) in [
        ("state", "waiting", "waiting", None),
        (
            "blocked",
            "waiting on owner",
            "blocked",
            Some("waiting on owner"),
        ),
        (
            "question",
            "which target?",
            "question",
            Some("which target?"),
        ),
    ] {
        let response = post_report(
            addr,
            Some(SEAT),
            &[
                "report",
                leaf,
                value,
                "--assignment",
                id,
                "--refs",
                "x, y,,",
            ],
        )
        .await;
        let status = response.status();
        let receipt: serde_json::Value = response.json().await.expect("receipt");
        assert_eq!(status, reqwest::StatusCode::OK, "{receipt}");
        assert_eq!(receipt["data"]["assignment_id"], id);
        assert_eq!(receipt["data"]["refs"], serde_json::json!(["x", "y"]));
        let services =
            pij_daemon::build_services(&config, std::path::Path::new("/unused-report-assignment"))
                .await
                .expect("reopen");
        let reports = ReportService::new(
            services.registry.as_ref(),
            services.spine.as_ref(),
            || 0,
            ReportConfig::default(),
        );
        let record = serde_json::to_value(
            reports
                .latest_state_record(&SeatId::from(SEAT))
                .await
                .expect("read persisted state")
                .expect("state"),
        )
        .expect("serialize");
        assert_eq!(record["assignment_id"], id);
        assert_eq!(record["refs"], serde_json::json!(["x", "y"]));
        assert_eq!(record["state"], expected_state);
        assert_eq!(record["note"], serde_json::json!(note));
        let node = governance_request(addr, "node", &["node", "show", SEAT]).await;
        assert_eq!(node["node"]["state"], record);
        let state: serde_json::Value = post_state(addr, SEAT).await.json().await.expect("state");
        assert_eq!(state["data"]["assignment_id"], id);
        assert_eq!(state["data"]["refs"], serde_json::json!(["x", "y"]));
        assert_eq!(state["data"]["stateNote"], serde_json::json!(note));
        if leaf == "question" {
            assert_eq!(receipt["data"]["decision"]["question"], value);
        }
    }
    let refs_only = post_report(
        addr,
        Some(SEAT),
        &["report", "state", "done", "--refs", "proof"],
    )
    .await;
    assert_eq!(refs_only.status(), reqwest::StatusCode::OK);
    let node = governance_request(addr, "node", &["node", "show", SEAT]).await;
    assert!(node["node"]["state"]["assignment_id"].is_null());
    assert_eq!(node["node"]["state"]["refs"], serde_json::json!(["proof"]));
    assert!(
        node["node"]["assignments"][0]["closed_at"].is_null(),
        "done does not close a task"
    );
    server.abort();
}

#[tokio::test]
async fn scoped_report_clear_removes_only_its_assignment_declaration() {
    let (addr, server, _store, _config) = daemon().await;
    let task =
        governance_request(addr, "task", &["task", "set", SEAT, "clear scoped blocker"]).await;
    let id = task["task"]["id"].as_str().expect("task id");
    let declared = post_report(
        addr,
        Some(SEAT),
        &["report", "state", "blocked", "--assignment", id],
    )
    .await;
    assert_eq!(declared.status(), reqwest::StatusCode::OK);
    let cleared = post_report(addr, Some(SEAT), &["report", "clear"]).await;
    assert_eq!(cleared.status(), reqwest::StatusCode::OK);
    let card: serde_json::Value = post_state(addr, SEAT).await.json().await.unwrap();
    assert!(card["data"]["semanticState"].is_null());
    assert_eq!(
        card["data"]["badge"], "blocked",
        "unscoped clear leaves task testimony intact"
    );

    let cleared = post_report(addr, Some(SEAT), &["report", "clear", "--assignment", id]).await;
    let status = cleared.status();
    let receipt: serde_json::Value = cleared.json().await.unwrap();
    assert_eq!(status, reqwest::StatusCode::OK, "{receipt}");
    let card: serde_json::Value = post_state(addr, SEAT).await.json().await.unwrap();
    assert_eq!(card["data"]["badge"], "idle");
    let node = governance_request(addr, "node", &["node", "show", SEAT]).await;
    assert!(
        node["node"]["assignments"][0]["closed_at"].is_null(),
        "clear is not task closure"
    );
    server.abort();
    let _ = server.await;
}

#[tokio::test]
async fn assignment_refusals_name_owner_and_leave_state_and_questions_untouched() {
    let (addr, server, _store, config) = daemon().await;
    let services =
        pij_daemon::build_services(&config, std::path::Path::new("/unused-report-assignment"))
            .await
            .expect("reopen");
    services
        .registry
        .put(SeatDescriptor::new("other", Harness::Omp, "/abs/tree"))
        .await
        .expect("other seat");
    let task = governance_request(addr, "task", &["task", "set", "other", "other work"]).await;
    let id = task["task"]["id"].as_str().expect("task id");
    let before = services.spine.tail(None, Seq(0)).await.expect("before");
    for (assignment, code) in [
        (id, "E-RS-ASSIGNMENT-NOT-YOURS"),
        ("missing", "E-RS-ASSIGNMENT-UNKNOWN"),
    ] {
        for argv in [
            vec!["report", "state", "done", "--assignment", assignment],
            vec!["report", "blocked", "blocked", "--assignment", assignment],
            vec!["report", "question", "why?", "--assignment", assignment],
            vec!["report", "clear", "--assignment", assignment],
        ] {
            let response = post_report(addr, Some(SEAT), &argv).await;
            let status = response.status();
            let body: serde_json::Value = response.json().await.expect("refusal");
            assert_eq!(status, reqwest::StatusCode::BAD_REQUEST, "{body}");
            assert_eq!(body["details"]["code"], code, "{body}");
            if assignment == id {
                assert!(body["meta"].as_str().unwrap().contains("other"), "{body}");
            }
        }
    }
    assert_eq!(
        services.spine.tail(None, Seq(0)).await.expect("after"),
        before
    );
    server.abort();
}

#[tokio::test]
async fn assignment_done_is_verified_only_by_current_parent_for_matching_task() {
    let (addr, server, _store, config) = daemon().await;
    let services =
        pij_daemon::build_services(&config, std::path::Path::new("/unused-report-assignment"))
            .await
            .expect("reopen");
    services
        .registry
        .put(SeatDescriptor::new("parent", Harness::Omp, "/abs/tree"))
        .await
        .expect("parent");
    let mut seat = services
        .registry
        .get(&SeatId::from(SEAT))
        .await
        .expect("read")
        .expect("seat");
    seat.parent = Some(SeatId::from("parent"));
    services.registry.put(seat).await.expect("record parent");
    let task = governance_request(addr, "task", &["task", "set", SEAT, "done control"]).await;
    let id = task["task"]["id"].as_str().expect("task id");
    let declared: serde_json::Value = post_report(
        addr,
        Some(SEAT),
        &["report", "state", "done", "--assignment", id],
    )
    .await
    .json()
    .await
    .expect("done receipt");
    assert_eq!(declared["ok"], true, "{declared}");
    let self_verify: serde_json::Value = post_report(
        addr,
        Some(SEAT),
        &["report", "verify", SEAT, "--assignment", id],
    )
    .await
    .json()
    .await
    .expect("self refusal");
    assert_eq!(self_verify["details"]["code"], "E-RS-OWNERSHIP");
    let other_task = governance_request(addr, "task", &["task", "set", SEAT, "other done"]).await;
    let other_id = other_task["task"]["id"].as_str().expect("other task id");
    for later_assignment in [None, Some(other_id)] {
        // Repeating A ensures both readers select its latest done, not its first.
        let latest: serde_json::Value = post_report(
            addr,
            Some(SEAT),
            &["report", "state", "done", "--assignment", id],
        )
        .await
        .json()
        .await
        .expect("latest A");
        assert_eq!(latest["ok"], true, "{latest}");
        let mut later_argv = vec!["report", "state", "done"];
        if let Some(assignment) = later_assignment {
            later_argv.extend(["--assignment", assignment]);
        }
        let later: serde_json::Value = post_report(addr, Some(SEAT), &later_argv)
            .await
            .json()
            .await
            .expect("later unrelated done");
        assert_eq!(later["ok"], true, "{later}");
        let before = governance_request(addr, "anomalies", &["anomalies"]).await;
        let row = before["anomalies"]
            .as_array()
            .expect("rows")
            .iter()
            .find(|row| row["kind"] == "unverified-done" && row["assignmentId"] == id)
            .expect("A awaits verification");
        assert_eq!(row["evidence"][0], latest["data"]["seq"]);

        // Run the detector's actual advice for the fact selected by HTTP, not a
        // separately written verify command that could drift from remediation.
        let facts = [DoneFact {
            seat: SeatId::from(row["nodeId"].as_str().expect("node")),
            assignment_id: Some(
                row["assignmentId"]
                    .as_str()
                    .expect("assignment")
                    .to_string(),
            ),
            done_seq: Seq(row["evidence"][0].as_u64().expect("done seq")),
            verified_seq: None,
            verified_done_seq: None,
        }];
        let advice = UnverifiedDoneDetector.scan(&AnomalyView {
            now_ms: 0,
            thresholds: AnomalyThresholds::default(),
            seats: &[],
            cards: &[],
            activity: &[],
            dispatches: &[],
            done: &facts,
            dispositions: &[],
            decisions: &[],
            dead: &[],
        });
        let argv: Vec<_> = advice[0]
            .remediation_line
            .split_whitespace()
            .skip(1)
            .collect();
        let verified: serde_json::Value = post_report(addr, Some("parent"), &argv)
            .await
            .json()
            .await
            .expect("verify via remediation");
        assert_eq!(verified["ok"], true, "{verified}");
        assert_eq!(verified["data"]["done_seq"], latest["data"]["seq"]);
        assert_eq!(verified["data"]["assignment_id"], id);
        let after = governance_request(addr, "anomalies", &["anomalies"]).await;
        let rows = after["anomalies"].as_array().expect("rows");
        assert!(
            !rows
                .iter()
                .any(|row| { row["kind"] == "unverified-done" && row["assignmentId"] == id }),
            "A's remediation must clear A: {after}"
        );
        assert!(
            rows.iter().any(|row| {
                row["kind"] == "unverified-done" && row["evidence"][0] == later["data"]["seq"]
            }),
            "unrelated done still awaits verification: {after}"
        );

        let unscoped: serde_json::Value =
            post_report(addr, Some("parent"), &["report", "verify", SEAT])
                .await
                .json()
                .await
                .expect("unscoped verify");
        assert_eq!(unscoped["ok"], true, "{unscoped}");
        assert_eq!(unscoped["data"]["done_seq"], later["data"]["seq"]);
    }
    let wrong: serde_json::Value = post_report(
        addr,
        Some("parent"),
        &["report", "verify", SEAT, "--assignment", "different"],
    )
    .await
    .json()
    .await
    .expect("wrong task");
    assert_eq!(wrong["details"]["code"], "E-RS-DONE-ASSIGNMENT");
    server.abort();
}
