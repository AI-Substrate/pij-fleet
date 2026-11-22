//! F282: a prime may link and close any seat, recorded parent or not, over
//! HTTP on a real store. Each act it takes from another seat's parent leaves
//! a `seat.prime-override` fact on the seat's history and one FYI to that
//! parent while it is live. A non-prime stays refused as before, and a close
//! never kills a process or pane.
use std::sync::Arc;

use pij_core::config::{AdapterChoice, Adapters, Config};
use pij_core::model::{Harness, SeatDescriptor, SeatId};
use pij_core::orchestration::{PrimeDesignation, PrimeState, Project};
use pij_daemon::Services;
use pij_daemon::http::{HttpConfig, router_with_config};
use pij_testkit::FreshStore;
use pij_testkit::fakes::{FakeLiveness, FakeTmux};
use serde_json::{Value, json};

struct Fixture {
    services: Services,
    tmux: Arc<FakeTmux>,
    addr: std::net::SocketAddr,
    server: tokio::task::JoinHandle<()>,
    _store: FreshStore,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}

/// The three kinds of prime, a parent with its children, and an outsider.
async fn fixture() -> Fixture {
    let store = FreshStore::new();
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
    let mut services = pij_daemon::build_services(
        &config,
        &std::path::Path::new(&store.path()).with_extension("signals"),
    )
    .await
    .expect("real store services");
    let tmux = Arc::new(FakeTmux::new());
    services.tmux = tmux.clone();
    services.liveness = Arc::new(FakeLiveness::new());
    let child = |id: &str| {
        let mut seat = SeatDescriptor::new(id, Harness::Claude, "/w");
        seat.parent = Some("pij-parent".into());
        seat.pane = Some(format!("%{}", id.len()));
        seat
    };
    for seat in [
        SeatDescriptor::new("pij-designated", Harness::Omp, "/w"),
        SeatDescriptor::new("pij-project-prime", Harness::Omp, "/w"),
        SeatDescriptor::new("pij-role-prime", Harness::Omp, "/w"),
        SeatDescriptor::new("pij-parent", Harness::Omp, "/w"),
        SeatDescriptor::new("pij-outsider", Harness::Omp, "/w"),
        child("pij-child-a"),
        child("pij-child-bb"),
        child("pij-child-ccc"),
        child("pij-child-dddd"),
    ] {
        services.registry.put(seat).await.expect("seat");
    }
    services
        .directory
        .store()
        .create_project(&Project {
            slug: "fleet".into(),
            description: None,
            repo: None,
            plan_path: None,
            prime_id: Some("pij-project-prime".into()),
            created_by: "pij-human".into(),
            created_at: 1,
        })
        .await
        .expect("project");
    services
        .watchdogs()
        .designate_prime(&PrimeDesignation {
            seat: "pij-designated".into(),
            designated_by: "pij-human".into(),
            designated_at: 1,
            state: PrimeState::Current,
        })
        .await
        .expect("designation");
    services
        .roles
        .assert_role(
            &"pij-role-prime".into(),
            &"pij-role-prime".into(),
            Some("prime".to_string()),
        )
        .await
        .expect("role prime");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = router_with_config(
        services.clone(),
        HttpConfig {
            auth: pij_daemon::http::AuthRing::local("prime-key".to_string()),
            machine_alias: "test".to_string(),
        },
    );
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    Fixture {
        services,
        tmux,
        addr,
        server,
        _store: store,
    }
}

impl Fixture {
    async fn post(&self, path: &str, body: &Value) -> (u16, Value) {
        let response = reqwest::Client::new()
            .post(format!("http://{}{path}", self.addr))
            .bearer_auth("prime-key")
            .json(body)
            .send()
            .await
            .unwrap();
        let status = response.status().as_u16();
        (status, response.json().await.expect("envelope"))
    }

    async fn link(&self, caller: &str, seat: &str) -> (u16, Value) {
        self.post(
            "/v1/link",
            &json!({"argv": ["link", seat, "--role", "worker"], "caller": {"PIJ_SESSION_ID": caller}}),
        )
        .await
    }

    async fn close(&self, caller: &str, seat: &str) -> (u16, Value) {
        self.post(
            "/v1/close",
            &json!({"seat": seat, "caller": {"PIJ_SESSION_ID": caller}}),
        )
        .await
    }

    async fn row(&self, id: &str) -> SeatDescriptor {
        self.services
            .registry
            .get(&id.into())
            .await
            .unwrap()
            .expect("row")
    }

    async fn override_fact(&self, seat: &str) -> Option<Value> {
        self.services
            .spine
            .latest_matching(&SeatId::from(seat), &[pij_store::PRIME_OVERRIDE_EVENT])
            .await
            .expect("spine")
            .map(|event| serde_json::from_str(&event.payload).expect("payload"))
    }

    async fn fyis(&self, seat: &str) -> u64 {
        self.services
            .delivery
            .pending_fyi_count(&seat.into())
            .await
            .expect("count")
    }
}

/// Non-primes, before and after: neither a link nor a close of another
/// seat's child, which stays as it was.
#[tokio::test]
async fn a_non_prime_is_refused_link_and_close_as_before() {
    let f = fixture().await;
    let (status, linked) = f.link("pij-outsider", "pij-child-a").await;
    assert_eq!(status, 403, "{linked}");
    assert_eq!(linked["details"]["code"], "E-RS-OWNERSHIP", "{linked}");
    let (status, closed) = f.close("pij-outsider", "pij-child-a").await;
    assert_eq!(status, 403, "{closed}");
    assert_eq!(closed["details"]["code"], "E-RS-OWNERSHIP", "{closed}");
    let child = f.row("pij-child-a").await;
    assert_eq!(child.parent, Some("pij-parent".into()));
    assert!(child.tombstoned_at.is_none());
    assert!(f.override_fact("pij-child-a").await.is_none());
    assert_eq!(f.fyis("pij-parent").await, 0);
}

/// Each kind of prime takes a seat from its live parent: the seat is its
/// child with the role, the fact names the prime and the prior parent, and
/// the prior parent holds one FYI per act.
#[tokio::test]
async fn every_kind_of_prime_links_a_seat_from_its_live_parent() {
    let f = fixture().await;
    for (n, (prime, seat)) in [
        ("pij-designated", "pij-child-a"),
        ("pij-project-prime", "pij-child-bb"),
        ("pij-role-prime", "pij-child-ccc"),
    ]
    .into_iter()
    .enumerate()
    {
        let (status, linked) = f.link(prime, seat).await;
        assert_eq!(status, 200, "{prime}: {linked}");
        assert_eq!(linked["data"]["taken_from"], "pij-parent", "{linked}");
        let row = f.row(seat).await;
        assert_eq!(row.parent, Some(prime.into()), "{prime}");
        let fact = f.override_fact(seat).await.expect("audit fact");
        assert_eq!(
            fact,
            json!({"operation": "link", "prime": prime, "seat": seat, "prior_parent": "pij-parent"})
        );
        assert_eq!(f.fyis("pij-parent").await, n as u64 + 1, "{prime}");
    }
    // Its own child again: no override, no new fact or notice.
    let (status, again) = f.link("pij-designated", "pij-child-a").await;
    assert_eq!(status, 200, "{again}");
    assert!(again["data"].get("taken_from").is_none(), "{again}");
    assert_eq!(f.fyis("pij-parent").await, 3);
}

/// A prime closes another seat's child: a tombstone, the fact, one FYI to
/// the prior parent, and no pane or process killed.
#[tokio::test]
async fn a_prime_closes_any_seat_and_kills_nothing() {
    let f = fixture().await;
    let (status, closed) = f.close("pij-project-prime", "pij-child-dddd").await;
    assert_eq!(status, 200, "{closed}");
    assert!(f.row("pij-child-dddd").await.tombstoned_at.is_some());
    assert_eq!(
        f.override_fact("pij-child-dddd").await.expect("audit fact"),
        json!({"operation": "close", "prime": "pij-project-prime", "seat": "pij-child-dddd", "prior_parent": "pij-parent"})
    );
    assert_eq!(f.fyis("pij-parent").await, 1);
    assert!(
        !f.tmux.calls().iter().any(|call| call.starts_with("kill")),
        "{:?}",
        f.tmux.calls()
    );
    // A repeat close records nothing new and tells no one again.
    let (status, again) = f.close("pij-project-prime", "pij-child-dddd").await;
    assert_eq!(status, 200, "{again}");
    assert_eq!(f.fyis("pij-parent").await, 1);
    // The parent closing its own child is no override.
    let (status, own) = f.close("pij-parent", "pij-child-a").await;
    assert_eq!(status, 200, "{own}");
    assert!(f.override_fact("pij-child-a").await.is_none());
}
