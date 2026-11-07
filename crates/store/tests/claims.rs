//! Number claims and deliveries against a real store (plan 196, bp-0001,
//! bp-0002, bp-0004's store half).

mod support;

use pij_core::model::{Harness, SeatDescriptor, SeatId};
use pij_core::ports::Registry;
use pij_store::{ClaimKey, DeliveryRecord, ReleaseOutcome, SqliteOrchestration, SqliteRegistry};

fn key(number: u64) -> ClaimKey {
    ClaimKey {
        project: "pij".into(),
        kind: "plan".into(),
        number,
    }
}
use pij_testkit::FreshStore;

async fn store() -> (FreshStore, SqliteOrchestration, SqliteRegistry) {
    let fresh = FreshStore::new();
    let pool = pij_store::open(&fresh.path()).await.expect("open");
    let registry = SqliteRegistry::new(pool.clone(), support::publisher());
    for id in ["pij-a", "pij-b", "pij-prime"] {
        registry
            .put(SeatDescriptor::new(id, Harness::Omp, "/abs/tree"))
            .await
            .unwrap();
    }
    (fresh, SqliteOrchestration::new(pool), registry)
}

#[tokio::test]
async fn claims_take_the_next_number_after_claims_and_the_checkout_floor() {
    let (_fresh, store, _) = store().await;
    let a = SeatId::from("pij-a");
    let first = store
        .claim_number("pij", "plan", &a, "claims", 196, 1)
        .await
        .unwrap();
    assert_eq!(first.number, 197, "one past the checkout floor");
    let second = store
        .claim_number("pij", "plan", &a, "next", 0, 2)
        .await
        .unwrap();
    assert_eq!(second.number, 198, "one past the highest claim");
    let lifted = store
        .claim_number("pij", "plan", &a, "after a merge", 250, 3)
        .await
        .unwrap();
    assert_eq!(lifted.number, 251, "the floor wins when it is higher");
    let other_kind = store
        .claim_number("pij", "migration", &a, "m", 33, 4)
        .await
        .unwrap();
    assert_eq!(other_kind.number, 34);
    let other_project = store
        .claim_number("osk", "plan", &a, "x", 0, 5)
        .await
        .unwrap();
    assert_eq!(other_project.number, 1, "projects never interact");
}

#[tokio::test]
async fn ten_concurrent_claims_are_distinct_and_consecutive() {
    let (_fresh, store, _) = store().await;
    let mut tasks = Vec::new();
    for index in 0..10u64 {
        let store = store.clone();
        tasks.push(tokio::spawn(async move {
            let seat = SeatId::from(if index % 2 == 0 { "pij-a" } else { "pij-b" });
            store
                .claim_number("pij", "prd", &seat, &format!("s{index}"), 2, index)
                .await
                .unwrap()
                .number
        }));
    }
    let mut numbers = Vec::new();
    for task in tasks {
        numbers.push(task.await.unwrap());
    }
    numbers.sort_unstable();
    assert_eq!(numbers, (3..13).collect::<Vec<u64>>());
    let other = store
        .claim_number("osk", "prd", &SeatId::from("pij-a"), "x", 0, 1)
        .await
        .unwrap();
    assert_eq!(other.number, 1);
}

#[tokio::test]
async fn claims_release_by_owner_or_prime_and_a_released_number_is_never_reissued() {
    let (_fresh, store, _) = store().await;
    let (a, b, prime) = (
        SeatId::from("pij-a"),
        SeatId::from("pij-b"),
        SeatId::from("pij-prime"),
    );
    let mine = store
        .claim_number("pij", "plan", &a, "x", 0, 1)
        .await
        .unwrap();
    let theirs = store
        .claim_number("pij", "plan", &b, "y", 0, 2)
        .await
        .unwrap();

    let refused = store
        .release_claim(&key(mine.number), &b, None, "mine now", 3)
        .await
        .unwrap();
    assert!(
        matches!(refused, ReleaseOutcome::NotOwner(_)),
        "{refused:?}"
    );
    let released = store
        .release_claim(&key(mine.number), &a, None, "abandoned", 4)
        .await
        .unwrap();
    let ReleaseOutcome::Released(claim) = released else {
        panic!("the owner releases: {released:?}");
    };
    assert_eq!(claim.release_reason.as_deref(), Some("abandoned"));
    assert!(matches!(
        store
            .release_claim(&key(mine.number), &a, None, "again", 5)
            .await
            .unwrap(),
        ReleaseOutcome::AlreadyReleased(_)
    ));
    assert_eq!(
        store
            .release_claim(&key(99), &a, None, "none", 5)
            .await
            .unwrap(),
        ReleaseOutcome::Unknown
    );
    assert!(matches!(
        store
            .release_claim(&key(theirs.number), &a, Some(&prime), "x", 6)
            .await
            .unwrap(),
        ReleaseOutcome::NotOwner(_)
    ));
    assert!(matches!(
        store
            .release_claim(&key(theirs.number), &prime, Some(&prime), "prime", 6)
            .await
            .unwrap(),
        ReleaseOutcome::Released(_)
    ));

    let next = store
        .claim_number("pij", "plan", &a, "z", 0, 7)
        .await
        .unwrap();
    assert_eq!(next.number, 3, "released numbers stay burned");
    let listed = store.claims("pij").await.unwrap();
    assert_eq!(
        listed
            .iter()
            .map(|claim| (claim.number, claim.released_at.is_some()))
            .collect::<Vec<_>>(),
        [(1, true), (2, true), (3, false)]
    );
}

#[tokio::test]
async fn tombstoning_the_claiming_seat_releases_its_open_claims() {
    let (_fresh, store, registry) = store().await;
    let (a, b) = (SeatId::from("pij-a"), SeatId::from("pij-b"));
    store
        .claim_number("pij", "plan", &a, "x", 0, 1)
        .await
        .unwrap();
    store
        .claim_number("pij", "migration", &a, "y", 0, 1)
        .await
        .unwrap();
    store
        .claim_number("pij", "plan", &b, "z", 0, 1)
        .await
        .unwrap();
    registry.tombstone(&a, "owner-close").await.unwrap();
    let claims = store.claims("pij").await.unwrap();
    for claim in &claims {
        if claim.seat == "pij-a" {
            assert_eq!(
                claim.release_reason.as_deref(),
                Some("seat-closed"),
                "{claim:?}"
            );
            assert!(claim.released_at.is_some());
        } else {
            assert_eq!(claim.released_at, None, "{claim:?}");
        }
    }
    assert_eq!(claims.len(), 3);
}

#[tokio::test]
async fn deliveries_record_once_release_named_claims_and_reach_the_pm_card() {
    let (_fresh, store, _) = store().await;
    let a = SeatId::from("pij-a");
    let plan = store
        .claim_number("pij", "plan", &a, "x", 195, 1)
        .await
        .unwrap();
    let record = DeliveryRecord {
        project: "pij".into(),
        pr: 130,
        merge_commit: "abc123".into(),
        recorded_by: "pij-prime".into(),
        pm: Some("pij-b".into()),
        addresses: vec![
            "docs/project/prds/001-pij-community.dd.json#requirements/req-20".into(),
            "docs/project/roadmaps/001-pij-community.dd.json#steps/s11".into(),
        ],
        releases: vec![("plan".into(), plan.number), ("plan".into(), 999)],
        at: 10,
    };
    let first = store.record_delivery_committed(&record).await.unwrap();
    assert_eq!(first.recorded.len(), 2);
    assert!(first.already.is_empty());
    assert_eq!(first.released.len(), 1);
    assert_eq!(
        first.released[0].release_reason.as_deref(),
        Some("delivered #130")
    );
    assert_eq!(first.events.len(), 1);
    assert_eq!(first.events[0].kind, "delivered");
    assert_eq!(
        first.events[0].seat.as_ref().map(SeatId::as_str),
        Some("pij-b")
    );
    assert!(first.events[0].seq.is_some());

    let again = store.record_delivery_committed(&record).await.unwrap();
    assert!(again.recorded.is_empty(), "idempotent");
    assert_eq!(again.already.len(), 2);
    assert!(again.released.is_empty());
    assert!(again.events.is_empty(), "nothing new, no event");
    assert_eq!(again.earlier_pm.as_deref(), Some("pij-b"));

    for pr in [131, 132, 133] {
        let mut later = record.clone();
        later.pr = pr;
        later.at = 10 + pr;
        later.releases.clear();
        store.record_delivery_committed(&later).await.unwrap();
    }
    let card = store.deliveries_for_pm("pij-b", 3).await.unwrap();
    assert_eq!(
        card.iter().map(|note| note.pr).collect::<Vec<_>>(),
        [133, 132, 131]
    );
    assert_eq!(card[0].addresses.len(), 2);
    assert!(
        store
            .deliveries_for_pm("pij-a", 3)
            .await
            .unwrap()
            .is_empty()
    );
    let address = "docs/project/prds/001-pij-community.dd.json#requirements/req-20";
    assert_eq!(
        store.delivered_prs("pij", address).await.unwrap(),
        [130, 131, 132, 133],
        "every PR recorded for the address, oldest first"
    );
    assert!(
        store
            .delivered_prs("osk", address)
            .await
            .unwrap()
            .is_empty()
    );
}

/// Performance review of #131: both card reads walk an index in index order,
/// bounded, and never build a temporary b-tree to group or sort the PM's
/// history.
#[tokio::test]
async fn the_pm_card_reads_walk_their_indexes_without_sorting_the_history() {
    let (fresh, _store, _) = store().await;
    let pool = pij_store::open(&fresh.path()).await.expect("open");
    let step = sqlx::query(pij_store::claims::PM_DELIVERY_STEP_SQL)
        .bind("pij-b")
        .bind(i64::MAX)
        .bind(3_i64);
    let addresses = sqlx::query(pij_store::claims::DELIVERY_ADDRESSES_SQL)
        .bind("pij")
        .bind(143_i64);
    for (query, index) in [
        (step, "COVERING INDEX deliveries_by_pm"),
        (addresses, "sqlite_autoindex_deliveries_1"),
    ] {
        let plan: Vec<String> = query_plan(&pool, query).await;
        assert!(plan.iter().any(|step| step.contains(index)), "{plan:?}");
        assert!(
            !plan.iter().any(|step| step.contains("TEMP B-TREE")),
            "no sort or grouping: {plan:?}"
        );
    }
}

async fn query_plan<'a>(
    pool: &sqlx::SqlitePool,
    query: sqlx::query::Query<'a, sqlx::Sqlite, sqlx::sqlite::SqliteArguments<'a>>,
) -> Vec<String> {
    use sqlx::{Execute, Row};

    let mut query = query;
    let sql = format!("EXPLAIN QUERY PLAN {}", query.sql());
    let arguments = query.take_arguments().expect("arguments").expect("bound");
    sqlx::query_with(&sql, arguments)
        .fetch_all(pool)
        .await
        .expect("plan")
        .iter()
        .map(|row| row.get::<String, _>("detail"))
        .collect()
}

/// Performance review of #131 at 93ca2b1: a newest PR with more addresses than
/// any row cap still shows whole, and the two older PRs still show.
#[tokio::test]
async fn a_large_newest_delivery_never_hides_an_address_or_an_older_pr() {
    let (_fresh, store, _) = store().await;
    let delivery = |pr: u64, addresses: usize, at: u64| DeliveryRecord {
        project: "pij".into(),
        pr,
        merge_commit: format!("m{pr}"),
        recorded_by: "pij-prime".into(),
        pm: Some("pij-b".into()),
        addresses: (0..addresses)
            .map(|n| format!("docs/project/prds/001-pij-community.dd.json#requirements/req-{n:03}"))
            .collect(),
        releases: Vec::new(),
        at,
    };
    for (pr, addresses, at) in [(140, 1, 10), (141, 2, 20), (142, 3, 30), (143, 60, 40)] {
        store
            .record_delivery_committed(&delivery(pr, addresses, at))
            .await
            .unwrap();
    }
    let card = store.deliveries_for_pm("pij-b", 3).await.unwrap();
    assert_eq!(
        card.iter()
            .map(|note| (note.pr, note.addresses.len(), note.recorded_at))
            .collect::<Vec<_>>(),
        [(143, 60, 40), (142, 3, 30), (141, 2, 20)]
    );
    assert_eq!(card[0].merge_commit, "m143");
    assert!(card[0].addresses.is_sorted());
    assert_eq!(card[0].addresses, delivery(143, 60, 40).addresses);
}

/// #131 N002: a PR recorded again later by another seat shows that seat with
/// that time, not the first address's recorder with the newer time.
#[tokio::test]
async fn a_later_recording_by_another_seat_names_that_seat_with_its_time() {
    let (_fresh, store, _) = store().await;
    let recording = |address: &str, by: &str, at: u64| DeliveryRecord {
        project: "pij".into(),
        pr: 150,
        merge_commit: "m150".into(),
        recorded_by: by.into(),
        pm: Some("pij-b".into()),
        addresses: vec![format!(
            "docs/project/prds/001-pij-community.dd.json#requirements/{address}"
        )],
        releases: Vec::new(),
        at,
    };
    for (address, by, at) in [("req-20", "pij-first", 10), ("req-21", "pij-later", 20)] {
        store
            .record_delivery_committed(&recording(address, by, at))
            .await
            .unwrap();
    }
    let card = store.deliveries_for_pm("pij-b", 3).await.unwrap();
    assert_eq!(
        card.iter()
            .map(|note| (
                note.pr,
                note.addresses.len(),
                note.recorded_at,
                note.recorded_by.as_str()
            ))
            .collect::<Vec<_>>(),
        [(150, 2, 20, "pij-later")]
    );
}
