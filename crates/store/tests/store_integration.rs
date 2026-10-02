//! Tier-3 exemplar: the store, against a REAL SQLite database.
//!
//! No fake here on purpose. Tiers 1 and 2 prove logic and contracts; this tier
//! exists to prove the things only a real database can be wrong about — the
//! migration actually ran, WAL actually took, sequence numbers actually come
//! from AUTOINCREMENT, and the SAME contract the fake passes is passed by SQL.
//!
//! Every test takes a `FreshStore`: entropy-named, destroyed on drop, never
//! shared. A suite that shares a database is a suite that can pass in the wrong
//! order and fail alone.

mod support;

use pij_core::error::PijError;
use pij_core::model::{
    Event, Harness, ProcIdentity, SeatDescriptor, SeatId, SemanticState, Seq, SystemState,
};
use pij_core::ports::{Registry, SeatFilter, Spine};
use pij_store::{SqliteRegistry, SqliteSpine};
use pij_testkit::FreshStore;
use pij_testkit::contract::{registry_contract, spine_contract};
use sqlx::{Column, Row};

fn native_copilot(id: &str) -> SeatDescriptor {
    let mut seat = SeatDescriptor::new(id, Harness::Copilot, "/abs/tree");
    seat.proc = Some(ProcIdentity {
        pid: 13700,
        proc_start: 20260905120000,
    });
    seat.harness_session = Some("00000000-0000-4000-8000-000000000137".to_string());
    seat.native_extension_delivery = true;
    seat
}

#[tokio::test]
async fn native_delivery_migration_preserves_schema_12_rows_and_history_without_attestation() {
    let fresh = FreshStore::new();
    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(fresh.path())
        .create_if_missing(true);
    let legacy_pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .expect("open historical store");
    let schema_12 = sqlx::migrate::Migrator {
        migrations: std::borrow::Cow::Owned(
            pij_store::migrate::MIGRATIONS
                .iter()
                .filter(|migration| migration.version <= 12)
                .cloned()
                .collect(),
        ),
        ..sqlx::migrate::Migrator::DEFAULT
    };
    schema_12
        .run(&legacy_pool)
        .await
        .expect("apply the actual historical migrations");
    // Version 12's daemon maintained this cache after running the ledger.
    sqlx::query("PRAGMA user_version = 12")
        .execute(&legacy_pool)
        .await
        .expect("record schema-12 daemon version");
    sqlx::raw_sql(
        "INSERT INTO seats (id, harness, harness_session, pane, pid, proc_start, folder, state, \
         semantic_state, role, parent, relay, tombstoned_at, tombstone_reason, seq, spawn_id, \
         model, provider, effort, cross_session_inbound_accept, rpc_port_intent) VALUES \
         ('legacy-native', 'copilot', '00000000-0000-4000-8000-000000000137', '%137', \
          13700, 20260905120000, '/abs/tree', 'working', 'question', 'worker', 'prime', 0, \
          NULL, NULL, 7, 'spawn-137', 'native-model', 'provider-137', 'high', 1, 47781); \
         INSERT INTO seats (id, harness, harness_session, pid, proc_start, folder, state, \
         tombstoned_at, tombstone_reason, seq, rpc_port_intent) VALUES \
         ('legacy-retired', 'copilot', '00000000-0000-4000-8000-000000000138', \
          13800, 20260905120001, '/retired', 'idle', 50, 'process exited', 8, NULL); \
         INSERT INTO seats (id, harness, folder, state, seq, rpc_port_intent) VALUES \
         ('legacy-omp', 'omp', '/other', 'idle', 9, 51948); \
         INSERT INTO spine_events (seq, v, at, kind, seat, payload) VALUES \
         (7, 1, 10, 'seat.put', 'legacy-native', '{\"rpc_port_intent\":47781}'); \
         INSERT INTO descriptor_merge_history \
         (id, survivor_id, alias_id, survivor_json, alias_json, mismatched_fields, verified_at) \
         VALUES (3, 'legacy-native', 'legacy-retired', '{\"rpc_port_intent\":47781}', \
                 '{\"rpc_port_intent\":null}', '[\"folder\"]', 20); \
         INSERT INTO delivered_messages (seq, recipient, msg_id, origin, delivered_at) VALUES \
         (5, 'legacy-native', 'historic-message', 'injected-to-transport', 30);",
    )
    .execute(&legacy_pool)
    .await
    .expect("seed historical rows and evidence");
    legacy_pool.close().await;

    let mut live = native_copilot("legacy-native");
    live.native_extension_delivery = false;
    live.pane = Some("%137".to_string());
    live.state = SystemState::Working;
    live.semantic_state = Some(SemanticState::Question);
    live.role = Some("worker".to_string());
    live.parent = Some(SeatId::from("prime"));
    live.spawn_id = Some("spawn-137".to_string());
    live.model = Some("native-model".to_string());
    live.provider = Some("provider-137".to_string());
    live.effort = Some("high".to_string());
    live.cross_session_inbound_accept = Some(true);
    let mut retired = SeatDescriptor::new("legacy-retired", Harness::Copilot, "/retired");
    retired.harness_session = Some("00000000-0000-4000-8000-000000000138".to_string());
    retired.proc = Some(ProcIdentity {
        pid: 13800,
        proc_start: 20260905120001,
    });
    retired.tombstoned_at = Some(50);
    retired.tombstone_reason = Some("process exited".to_string());
    let expected = [
        live,
        retired,
        SeatDescriptor::new("legacy-omp", Harness::Omp, "/other"),
    ];

    for _ in 0..2 {
        let pool = pij_store::open(&fresh.path())
            .await
            .expect("upgrade or reopen current schema");
        // Exercise every pooled connection's first seat projection after boot.
        // A version read alone cannot detect sqlx column metadata cached from
        // the pre-migration schema on a sibling connection.
        let mut connections = Vec::new();
        for _ in 0..pool.options().get_max_connections() {
            connections.push(pool.acquire().await.expect("acquire migrated connection"));
        }
        for connection in &mut connections {
            let row = sqlx::query("SELECT * FROM seats WHERE id='legacy-native'")
                .fetch_one(&mut **connection)
                .await
                .expect("read first migrated projection");
            assert_eq!(
                row.try_get::<i64, _>("native_extension_delivery")
                    .expect("every connection exposes native capability"),
                0
            );
            assert!(
                row.columns()
                    .iter()
                    .all(|column| column.name() != "rpc_port_intent"),
                "every connection uses current column metadata"
            );
        }
        drop(connections);
        assert_eq!(
            pij_store::schema_version(&pool).await.expect("version"),
            pij_store::SCHEMA_VERSION
        );
        let registry = SqliteRegistry::new(pool.clone(), support::publisher());
        for seat in &expected {
            assert_eq!(
                registry.get(&seat.id).await.expect("read migrated seat"),
                Some(seat.clone())
            );
        }
        let obsolete: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM pragma_table_info('seats') WHERE name='rpc_port_intent'",
        )
        .fetch_one(&pool)
        .await
        .expect("inspect migrated schema");
        assert_eq!(obsolete, 0, "obsolete launch intent has no live column");
        let seqs: Vec<(String, i64)> = sqlx::query_as("SELECT id, seq FROM seats ORDER BY seq")
            .fetch_all(&pool)
            .await
            .expect("read retained row sequences");
        assert_eq!(
            seqs,
            vec![
                ("legacy-native".to_string(), 7),
                ("legacy-retired".to_string(), 8),
                ("legacy-omp".to_string(), 9)
            ]
        );
        let event: (i64, i64, i64, String, String, String) =
            sqlx::query_as("SELECT seq, v, at, kind, seat, payload FROM spine_events")
                .fetch_one(&pool)
                .await
                .expect("read preserved event");
        assert_eq!(
            event,
            (
                7,
                1,
                10,
                "seat.put".to_string(),
                "legacy-native".to_string(),
                "{\"rpc_port_intent\":47781}".to_string()
            )
        );
        let history: (i64, String, String, String, String, String, i64) = sqlx::query_as(
            "SELECT id, survivor_id, alias_id, survivor_json, alias_json, mismatched_fields, verified_at \
             FROM descriptor_merge_history",
        ).fetch_one(&pool).await.expect("read preserved merge evidence");
        assert_eq!(
            history,
            (
                3,
                "legacy-native".to_string(),
                "legacy-retired".to_string(),
                "{\"rpc_port_intent\":47781}".to_string(),
                "{\"rpc_port_intent\":null}".to_string(),
                "[\"folder\"]".to_string(),
                20
            )
        );
        let delivered: (i64, String, String, String, i64) = sqlx::query_as(
            "SELECT seq, recipient, msg_id, origin, delivered_at FROM delivered_messages",
        )
        .fetch_one(&pool)
        .await
        .expect("read preserved delivery evidence");
        assert_eq!(
            delivered,
            (
                5,
                "legacy-native".to_string(),
                "historic-message".to_string(),
                "injected-to-transport".to_string(),
                30
            )
        );
        drop(registry);
        pool.close().await;
    }

    let pool = pij_store::open(&fresh.path())
        .await
        .expect("open upgraded store");
    sqlx::query("INSERT INTO seats (id, harness, folder, state, seq) VALUES ('default', 'pi', '/new', 'idle', 10)")
        .execute(&pool).await.expect("insert without native capability column");
    let default: i64 =
        sqlx::query_scalar("SELECT native_extension_delivery FROM seats WHERE id='default'")
            .fetch_one(&pool)
            .await
            .expect("read SQL default");
    assert_eq!(default, 0);
}

#[tokio::test]
async fn the_daemon_self_migrates_a_brand_new_store_at_boot() {
    // R1: automatic, no second command. A schema that needs a human step is a
    // schema that will be stale on somebody's machine.
    let fresh = FreshStore::new();
    assert!(!fresh.exists(), "nothing exists before open");

    let pool = pij_store::open(&fresh.path())
        .await
        .expect("open must migrate");

    assert!(fresh.exists(), "opening the store creates it");
    assert_eq!(
        pij_store::schema_version(&pool).await.expect("version"),
        pij_store::SCHEMA_VERSION
    );
    pij_store::require_current_schema(&pool)
        .await
        .expect("a freshly migrated store is current");
}

#[tokio::test]
async fn wal_is_actually_on_which_the_migration_file_could_not_have_done() {
    // The trap this asserts against: `PRAGMA journal_mode` is a no-op inside a
    // transaction, and sqlx runs each migration in one — so setting WAL in
    // 0001.sql would LOOK right and silently leave a rollback journal.
    let fresh = FreshStore::new();
    let pool = pij_store::open(&fresh.path()).await.expect("open");

    assert_eq!(
        pij_store::migrate::journal_mode(&pool)
            .await
            .expect("journal mode")
            .to_lowercase(),
        "wal",
        "the journal mode must be WAL on disk, not merely requested"
    );
}

#[tokio::test]
async fn re_opening_an_existing_store_is_idempotent_and_keeps_the_data() {
    let fresh = FreshStore::new();
    let seat = SeatDescriptor::new("pij-persisted", Harness::Claude, "/abs/work");

    {
        let pool = pij_store::open(&fresh.path()).await.expect("open");
        SqliteRegistry::new(pool, support::publisher())
            .put(seat.clone())
            .await
            .expect("put");
    }

    let pool = pij_store::open(&fresh.path()).await.expect("re-open");
    let registry = SqliteRegistry::new(pool, support::publisher());
    assert_eq!(
        registry.get(&seat.id).await.expect("get"),
        Some(seat),
        "a second boot must migrate to a no-op and leave the rows alone"
    );
}

#[tokio::test]
async fn a_store_from_the_future_is_refused_in_the_direction_that_matters() {
    // Skew is directional. Forward is a migration; BACKWARD is a refusal,
    // because a binary cannot know what a schema it has never seen means, and a
    // half-understood schema corrupts quietly. Never limp.
    let fresh = FreshStore::new();
    let pool = pij_store::open(&fresh.path()).await.expect("open");
    sqlx::query("PRAGMA user_version = 99")
        .execute(&pool)
        .await
        .expect("bump the schema version");

    let error = pij_store::require_current_schema(&pool)
        .await
        .expect_err("a newer schema must be refused");

    match error {
        PijError::StoreSchemaStale {
            found,
            expected,
            ref fix,
        } => {
            assert_eq!((found, expected), (99, pij_store::SCHEMA_VERSION));
            assert!(
                fix.contains("upgrade"),
                "the refusal must name the direction-specific fix: {fix}"
            );
        }
        other => panic!("wrong error: {other:?}"),
    }

    // And the refusal survives a re-open: `open` will not silently "fix" a
    // future schema by running migrations over it.
    let error = pij_store::open(&fresh.path())
        .await
        .expect_err("open must refuse a future store too");
    assert!(matches!(error, PijError::StoreSchemaStale { .. }));
}

#[tokio::test]
async fn an_older_store_is_migrated_forward_rather_than_refused() {
    let fresh = FreshStore::new();
    let pool = pij_store::open(&fresh.path()).await.expect("open");
    sqlx::query("PRAGMA user_version = 0")
        .execute(&pool)
        .await
        .expect("pretend the store predates this binary");

    let error = pij_store::require_current_schema(&pool)
        .await
        .expect_err("a command must refuse a stale schema rather than write to it");
    match error {
        PijError::StoreSchemaStale { ref fix, .. } => assert!(
            fix.contains("restart the daemon"),
            "backward skew tells the caller to let the daemon migrate: {fix}"
        ),
        other => panic!("wrong error: {other:?}"),
    }
    drop(pool);

    // ...and the daemon's own boot does exactly that, without a human step.
    let pool = pij_store::open(&fresh.path())
        .await
        .expect("boot migrates forward");
    pij_store::require_current_schema(&pool)
        .await
        .expect("current after boot");
}

#[tokio::test]
async fn consent_backfill_upgrades_only_live_unknown_claude_rows_and_is_idempotent() {
    let fresh = FreshStore::new();
    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(fresh.path())
        .create_if_missing(true);
    let legacy_pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .expect("open legacy store");
    let legacy_migrator = sqlx::migrate::Migrator {
        migrations: std::borrow::Cow::Owned(
            pij_store::migrate::MIGRATIONS
                .iter()
                .filter(|migration| migration.version <= 9)
                .cloned()
                .collect(),
        ),
        ..sqlx::migrate::Migrator::DEFAULT
    };
    legacy_migrator
        .run(&legacy_pool)
        .await
        .expect("migrate fixture to schema 9");
    sqlx::raw_sql(
        "INSERT INTO seats \
         (id, harness, folder, state, relay, tombstoned_at, seq, cross_session_inbound_accept) \
         VALUES \
         ('pij-claude-live', 'claude', '/live', 'idle', 0, NULL, 1, NULL), \
         ('pij-claude-refused', 'claude', '/refused', 'idle', 0, NULL, 2, 0), \
         ('pij-claude-tombstone', 'claude', '/dead', 'idle', 0, 1, 3, NULL), \
         ('pij-omp-live', 'omp', '/omp', 'idle', 0, NULL, 4, NULL)",
    )
    .execute(&legacy_pool)
    .await
    .expect("seed schema-9 consent states");
    drop(legacy_pool);

    let migrated_pool = pij_store::open(&fresh.path())
        .await
        .expect("migrate schema 9 to current");
    let rows: Vec<(String, Option<i64>)> =
        sqlx::query_as("SELECT id, cross_session_inbound_accept FROM seats ORDER BY id")
            .fetch_all(&migrated_pool)
            .await
            .expect("read backfilled consent states");
    assert_eq!(
        rows,
        vec![
            ("pij-claude-live".to_string(), Some(1)),
            ("pij-claude-refused".to_string(), Some(0)),
            ("pij-claude-tombstone".to_string(), None),
            ("pij-omp-live".to_string(), None),
        ],
        "backfill changes only non-tombstoned Claude rows whose consent is unknown"
    );
    assert_eq!(
        pij_store::schema_version(&migrated_pool)
            .await
            .expect("schema version"),
        pij_store::SCHEMA_VERSION,
        "opening the store applies the backfill and every later migration"
    );

    let backfill = pij_store::migrate::MIGRATIONS
        .iter()
        .find(|migration| migration.version == 10)
        .expect("consent backfill migration");
    let repeated = sqlx::raw_sql(&backfill.sql)
        .execute(&migrated_pool)
        .await
        .expect("repeat the exact backfill SQL");
    assert_eq!(
        repeated.rows_affected(),
        0,
        "running the backfill SQL twice changes zero additional rows"
    );
}

#[tokio::test]
async fn harness_session_migration_round_trips_up_and_down() {
    let fresh = FreshStore::new();
    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(fresh.path())
        .create_if_missing(true);
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .expect("open schema-10 store");
    let schema_10 = sqlx::migrate::Migrator {
        migrations: std::borrow::Cow::Owned(
            pij_store::migrate::MIGRATIONS
                .iter()
                .filter(|migration| migration.version <= 10)
                .cloned()
                .collect(),
        ),
        ..sqlx::migrate::Migrator::DEFAULT
    };
    schema_10
        .run(&pool)
        .await
        .expect("migrate fixture to schema 10");
    sqlx::query(
        "INSERT INTO seats (id, harness, folder, state, relay, seq) \
         VALUES ('pij-schema-10', 'omp', '/abs/tree', 'idle', 0, 1)",
    )
    .execute(&pool)
    .await
    .expect("seed schema-10 seat");

    let schema_11 = sqlx::migrate::Migrator {
        migrations: std::borrow::Cow::Owned(
            pij_store::migrate::MIGRATIONS
                .iter()
                .filter(|migration| migration.version <= 11)
                .cloned()
                .collect(),
        ),
        ..sqlx::migrate::Migrator::DEFAULT
    };
    schema_11.run(&pool).await.expect("migrate schema 10 to 11");
    assert_eq!(pij_store::schema_version(&pool).await.expect("version"), 11);
    let session: Option<String> =
        sqlx::query_scalar("SELECT harness_session FROM seats WHERE id = 'pij-schema-10'")
            .fetch_one(&pool)
            .await
            .expect("new nullable column");
    assert_eq!(session, None, "migration invents no session for an old row");

    schema_11
        .undo(&pool, 10)
        .await
        .expect("revert schema 11 to 10");
    assert_eq!(pij_store::schema_version(&pool).await.expect("version"), 10);
    let column: Option<String> = sqlx::query_scalar(
        "SELECT name FROM pragma_table_info('seats') WHERE name = 'harness_session'",
    )
    .fetch_optional(&pool)
    .await
    .expect("inspect reverted schema")
    .flatten();
    assert_eq!(column, None, "down migration removes the added column");
    let retained: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM seats WHERE id = 'pij-schema-10'")
        .fetch_one(&pool)
        .await
        .expect("seat survives down migration");
    assert_eq!(retained, 1);
}

#[tokio::test]
async fn the_sqlite_registry_honours_the_same_contract_the_fake_does() {
    // The point of the shared suite: this is the identical function
    // `contract_over_fakes.rs` runs. If SQL and the fake ever disagree, exactly
    // one assertion fires and names the promise that broke.
    let fresh = FreshStore::new();
    let pool = pij_store::open(&fresh.path()).await.expect("open");
    registry_contract(&SqliteRegistry::new(pool, support::publisher())).await;
}

#[tokio::test]
async fn the_sqlite_spine_honours_the_same_contract_the_fake_does() {
    let fresh = FreshStore::new();
    let pool = pij_store::open(&fresh.path()).await.expect("open");
    spine_contract(&SqliteSpine::new(pool)).await;
}

#[tokio::test]
async fn a_seat_round_trips_with_every_optional_fact_intact() {
    let fresh = FreshStore::new();
    let pool = pij_store::open(&fresh.path()).await.expect("open");
    let registry = SqliteRegistry::new(pool, support::publisher());

    let mut seat = native_copilot("pij-full");
    seat.pane = Some("%255".to_string());
    seat.state = SystemState::Working;
    seat.semantic_state = Some(SemanticState::Question);
    seat.role = Some("pm".to_string());
    seat.parent = Some(SeatId::from("pij-prime"));
    seat.relay = true;

    registry.put(seat.clone()).await.expect("put");
    assert_eq!(registry.get(&seat.id).await.expect("get"), Some(seat));
}

#[tokio::test]
async fn native_capability_defaults_closed_upserts_and_tombstone_clears_it() {
    let fresh = FreshStore::new();
    let pool = pij_store::open(&fresh.path()).await.expect("open");
    let registry = SqliteRegistry::new(pool, support::publisher());
    let default = SeatDescriptor::new("pij-native-upsert", Harness::Copilot, "/abs/tree");
    assert!(
        !default.native_extension_delivery,
        "absence is the closed default"
    );
    registry
        .put(default.clone())
        .await
        .expect("insert unattested seat");
    assert_eq!(
        registry.get(&default.id).await.expect("get default"),
        Some(default)
    );

    let mut seat = native_copilot("pij-native-upsert");
    registry
        .put(seat.clone())
        .await
        .expect("attest current incarnation");
    assert_eq!(
        registry.get(&seat.id).await.expect("get attested"),
        Some(seat.clone())
    );

    seat.native_extension_delivery = false;
    registry
        .put(seat.clone())
        .await
        .expect("withdraw capability");
    assert_eq!(
        registry.get(&seat.id).await.expect("get withdrawn"),
        Some(seat.clone())
    );
    seat.native_extension_delivery = true;
    registry
        .put(seat.clone())
        .await
        .expect("reattest current incarnation");

    registry
        .tombstone(&seat.id, "process exited")
        .await
        .expect("tombstone");
    let tombstoned = registry
        .get(&seat.id)
        .await
        .expect("read tombstone")
        .expect("descriptor remains as post-mortem");
    assert!(tombstoned.tombstoned_at.is_some());
    assert!(!tombstoned.native_extension_delivery);
    assert_eq!(tombstoned.proc, seat.proc, "retain process history");
    assert_eq!(
        tombstoned.harness_session, seat.harness_session,
        "retain session history"
    );
}

#[tokio::test]
async fn incarnation_replacement_never_inherits_native_capability() {
    let fresh = FreshStore::new();
    let pool = pij_store::open(&fresh.path()).await.expect("open");
    let registry = SqliteRegistry::new(pool, support::publisher());
    for change in ["pid", "proc_start", "native_session", "revive"] {
        let mut seat = native_copilot(change);
        registry
            .put(seat.clone())
            .await
            .expect("seed attested incarnation");
        match change {
            "pid" => seat.proc.as_mut().expect("process").pid += 1,
            "proc_start" => seat.proc.as_mut().expect("process").proc_start += 1,
            "native_session" => {
                seat.harness_session = Some("00000000-0000-4000-8000-000000000138".to_string());
            }
            "revive" => {
                registry
                    .tombstone(&seat.id, "process exited")
                    .await
                    .expect("tombstone");
            }
            _ => unreachable!(),
        }
        // Registration supplies the current incarnation's explicit capability;
        // storage must never coalesce false with the previous row's true.
        seat.native_extension_delivery = false;
        registry
            .put(seat.clone())
            .await
            .expect("replace incarnation without attestation");
        assert_eq!(
            registry.get(&seat.id).await.expect("get replacement"),
            Some(seat.clone())
        );
        seat.native_extension_delivery = true;
        registry
            .put(seat.clone())
            .await
            .expect("attest replacement incarnation");
        assert_eq!(
            registry.get(&seat.id).await.expect("get reattested"),
            Some(seat)
        );
    }
    let mut seat = native_copilot("explicit-replacement");
    registry
        .put(seat.clone())
        .await
        .expect("seed original incarnation");
    seat.proc.as_mut().expect("process").proc_start += 1;
    seat.harness_session = Some("00000000-0000-4000-8000-000000000139".to_string());
    registry
        .put(seat.clone())
        .await
        .expect("persist explicitly attested replacement in one put");
    assert_eq!(
        registry
            .get(&seat.id)
            .await
            .expect("get explicit replacement"),
        Some(seat)
    );
}

#[tokio::test]
async fn native_capability_requires_a_live_copilot_process_and_native_session() {
    let fresh = FreshStore::new();
    let pool = pij_store::open(&fresh.path()).await.expect("open");
    let registry = SqliteRegistry::new(pool.clone(), support::publisher());
    for invalid in [
        "harness",
        "process",
        "pid",
        "proc_start",
        "session",
        "empty_session",
        "tombstone",
    ] {
        let mut seat = native_copilot(invalid);
        match invalid {
            "harness" => seat.harness = Harness::Omp,
            "process" => seat.proc = None,
            "pid" => seat.proc.as_mut().expect("process").pid = 0,
            "proc_start" => seat.proc.as_mut().expect("process").proc_start = 0,
            "session" => seat.harness_session = None,
            "empty_session" => seat.harness_session = Some(String::new()),
            "tombstone" => seat.tombstoned_at = Some(1),
            _ => unreachable!(),
        }
        let id = seat.id.clone();
        let error = registry
            .put(seat)
            .await
            .expect_err("invalid attestation must be refused");
        assert!(
            error.to_string().contains("native_extension_delivery"),
            "{error}"
        );
        assert_eq!(registry.get(&id).await.expect("get refused seat"), None);
    }
    let events: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM spine_events")
        .fetch_one(&pool)
        .await
        .expect("read failed puts");
    assert_eq!(
        events, 0,
        "invalid capability rolls back the paired spine event"
    );
}

#[tokio::test]
async fn native_capability_schema_refuses_non_boolean_values() {
    let fresh = FreshStore::new();
    let pool = pij_store::open(&fresh.path()).await.expect("open");
    let registry = SqliteRegistry::new(pool.clone(), support::publisher());
    registry
        .put(native_copilot("pij-native-range"))
        .await
        .expect("seed attested seat");

    for invalid in [-1_i64, 2, 65_536, i64::MAX] {
        let result = sqlx::query("UPDATE seats SET native_extension_delivery=?2 WHERE id=?1")
            .bind("pij-native-range")
            .bind(invalid)
            .execute(&pool)
            .await;
        assert!(
            result.is_err(),
            "migration CHECK must refuse native_extension_delivery={invalid}"
        );
    }
}

#[tokio::test]
async fn corrupt_native_capability_is_a_named_read_error_never_attestation() {
    let fresh = FreshStore::new();
    let pool = pij_store::open(&fresh.path()).await.expect("open");
    let registry = SqliteRegistry::new(pool.clone(), support::publisher());
    registry
        .put(native_copilot("pij-native-corrupt"))
        .await
        .expect("seed attested seat");
    let mut connection = pool
        .acquire()
        .await
        .expect("acquire corrupt-store connection");
    sqlx::query("PRAGMA ignore_check_constraints = ON")
        .execute(&mut *connection)
        .await
        .expect("model an already-corrupt store");

    for invalid in [-1_i64, 2, 65_536, i64::MAX] {
        sqlx::query("UPDATE seats SET native_extension_delivery=?2 WHERE id=?1")
            .bind("pij-native-corrupt")
            .bind(invalid)
            .execute(&mut *connection)
            .await
            .expect("plant corrupt flag");
        for error in [
            registry
                .get(&SeatId::from("pij-native-corrupt"))
                .await
                .expect_err("corrupt flag must not become attestation"),
            registry
                .list(SeatFilter::default())
                .await
                .expect_err("list must refuse the same corrupt flag"),
        ] {
            assert!(
                error.to_string().contains("native_extension_delivery")
                    && error.to_string().contains(&invalid.to_string()),
                "error must name the corrupt field and value: {error}"
            );
        }
    }

    sqlx::query("UPDATE seats SET native_extension_delivery=1, harness_session=NULL WHERE id=?1")
        .bind("pij-native-corrupt")
        .execute(&mut *connection)
        .await
        .expect("plant capability without native identity");
    let error = registry
        .get(&SeatId::from("pij-native-corrupt"))
        .await
        .expect_err("a boolean alone is not native attestation");
    assert!(
        error.to_string().contains("native_extension_delivery"),
        "{error}"
    );
    sqlx::query("PRAGMA ignore_check_constraints = OFF")
        .execute(&mut *connection)
        .await
        .expect("restore connection constraints");
}

#[tokio::test]
async fn filters_narrow_and_absent_filter_fields_do_not() {
    let fresh = FreshStore::new();
    let pool = pij_store::open(&fresh.path()).await.expect("open");
    let registry = SqliteRegistry::new(pool, support::publisher());

    registry
        .put(SeatDescriptor::new("pij-a", Harness::Pi, "/tree/one"))
        .await
        .expect("put");
    let mut child = SeatDescriptor::new("pij-b", Harness::Claude, "/tree/two");
    child.parent = Some(SeatId::from("pij-a"));
    registry.put(child).await.expect("put");

    assert_eq!(
        registry
            .list(SeatFilter::default())
            .await
            .expect("list")
            .len(),
        2
    );
    assert_eq!(
        registry
            .list(SeatFilter {
                harness: Some(Harness::Claude),
                ..SeatFilter::default()
            })
            .await
            .expect("list")
            .len(),
        1
    );
    assert_eq!(
        registry
            .list(SeatFilter {
                parent: Some(SeatId::from("pij-a")),
                ..SeatFilter::default()
            })
            .await
            .expect("list")
            .len(),
        1
    );
    assert!(
        registry
            .list(SeatFilter {
                folder: Some(String::new()),
                ..SeatFilter::default()
            })
            .await
            .expect("list")
            .is_empty(),
        "an EMPTY filter value filters for empty — it is not the same as absent"
    );
}

#[tokio::test]
async fn a_half_written_process_identity_is_refused_rather_than_half_believed() {
    // A pid with no start time is not half an identity; it is an unusable one,
    // and believing it is the recycled-pid bug in a different costume.
    let fresh = FreshStore::new();
    let pool = pij_store::open(&fresh.path()).await.expect("open");
    sqlx::query(
        "INSERT INTO seats (id, harness, folder, state, pid, relay, seq) \
         VALUES ('pij-halfbound', 'pi', '/tmp', 'idle', 4242, 0, 1)",
    )
    .execute(&pool)
    .await
    .expect("plant a half-written row");

    let error = SqliteRegistry::new(pool, support::publisher())
        .get(&SeatId::from("pij-halfbound"))
        .await
        .expect_err("half an identity must be refused");
    assert!(
        error.to_string().contains("half a process identity"),
        "the refusal must say what is wrong with the row: {error}"
    );
}

#[tokio::test]
async fn the_spine_is_append_only_and_its_sequence_never_goes_backwards() {
    let fresh = FreshStore::new();
    let pool = pij_store::open(&fresh.path()).await.expect("open");
    let spine = SqliteSpine::new(pool);

    let event = |kind: &str, seat: Option<&str>| Event {
        seq: None,
        v: 1,
        at: 1_724_800_000_000,
        kind: kind.to_string(),
        seat: seat.map(SeatId::from),
        payload: "{}".to_string(),
    };

    let first = spine
        .append(event("report", Some("pij-a")))
        .await
        .expect("append");
    let second = spine
        .append(event("report", Some("pij-b")))
        .await
        .expect("append");
    let third = spine
        .append(event("receipt", Some("pij-a")))
        .await
        .expect("append");
    assert!(first < second && second < third, "seq is monotonic");

    // `tail(since)` means "everything after what I have seen", which is what
    // makes a resumable stream possible at all.
    let after_first = spine.tail(None, first).await.expect("tail");
    assert_eq!(after_first.len(), 2);

    let just_a = spine
        .tail(Some(&SeatId::from("pij-a")), Seq(0))
        .await
        .expect("tail");
    assert_eq!(just_a.len(), 2, "seat filter narrows: {just_a:?}");
    assert_eq!(just_a[0].kind, "report");
    assert_eq!(just_a[1].kind, "receipt");

    assert!(
        spine.tail(None, third).await.expect("tail").is_empty(),
        "a tail from the newest event is empty, not an error"
    );
}

#[tokio::test]
async fn each_test_gets_its_own_database() {
    // The property the whole tier depends on. Two FreshStores are two files, and
    // neither can see the other's rows.
    let one = FreshStore::new();
    let two = FreshStore::new();
    assert_ne!(one.path(), two.path());

    let pool_one = pij_store::open(&one.path()).await.expect("open");
    SqliteRegistry::new(pool_one, support::publisher())
        .put(SeatDescriptor::new("pij-only-in-one", Harness::Pi, "/tmp"))
        .await
        .expect("put");

    let pool_two = pij_store::open(&two.path()).await.expect("open");
    assert_eq!(
        SqliteRegistry::new(pool_two, support::publisher())
            .get(&SeatId::from("pij-only-in-one"))
            .await
            .expect("get"),
        None
    );
}

#[test]
fn a_dropped_fresh_store_takes_its_wal_sidecars_with_it() {
    // WAL leaves `-wal` and `-shm` beside the database; a cleanup that forgets
    // them leaves state behind for whatever reuses the name next.
    let path = {
        let fresh = FreshStore::new();
        let path = fresh.path();
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime.block_on(async {
            let pool = pij_store::open(&path).await.expect("open");
            SqliteRegistry::new(pool, support::publisher())
                .put(SeatDescriptor::new("pij-tmp", Harness::Pi, "/tmp"))
                .await
                .expect("put");
        });
        assert!(std::path::Path::new(&path).exists());
        path
    };

    for suffix in ["", "-wal", "-shm"] {
        assert!(
            !std::path::Path::new(&format!("{path}{suffix}")).exists(),
            "dropping FreshStore must remove {path}{suffix}"
        );
    }
}

#[tokio::test]
async fn bind_evidence_round_trips_and_absent_stays_absent() {
    // R5 items 13-15, and the class behind them: the registry row lacked facts
    // another surface already held. A row that ROUND-TRIPS them is the fix; a row
    // that invents them for an adopted seat would be the same defect mirrored.
    let fresh = FreshStore::new();
    let pool = pij_store::open(&fresh.path()).await.expect("open");
    let registry = SqliteRegistry::new(pool, support::publisher());

    let mut spawned = SeatDescriptor::new("pij-spawned", Harness::Omp, "/abs/tree");
    spawned.spawn_id = Some("s1787958708582-43870".to_string());
    spawned.model = Some("github-copilot/gpt-5.6-sol-fast".to_string());
    spawned.provider = Some("github-copilot".to_string());
    spawned.effort = Some("high".to_string());

    registry.put(spawned.clone()).await.expect("put");
    assert_eq!(
        registry.get(&spawned.id).await.expect("get"),
        Some(spawned),
        "every fact the spawn knew must be readable from the registry"
    );

    // A seat a human adopted never had a spawn id. Absent must stay absent.
    let adopted = SeatDescriptor::new("pij-adopted", Harness::Claude, "/abs/other");
    registry.put(adopted.clone()).await.expect("put");
    let read_back = registry
        .get(&adopted.id)
        .await
        .expect("get")
        .expect("present");
    assert_eq!(read_back.spawn_id, None);
    assert_eq!(read_back.model, None);
    assert_eq!(read_back, adopted);
}

#[tokio::test]
async fn an_older_store_gains_the_bind_evidence_columns_by_migrating_forward() {
    // Migration 0003 is additive over a store that already has seats in it: the
    // rows survive, and their new columns read absent rather than defaulting to
    // a value nobody chose.
    let fresh = FreshStore::new();
    {
        let pool = pij_store::open(&fresh.path()).await.expect("open");
        SqliteRegistry::new(pool, support::publisher())
            .put(SeatDescriptor::new("pij-legacy", Harness::Pi, "/abs/old"))
            .await
            .expect("put");
    }

    let pool = pij_store::open(&fresh.path())
        .await
        .expect("re-open migrates");
    assert_eq!(
        pij_store::schema_version(&pool).await.expect("version"),
        pij_store::SCHEMA_VERSION
    );
    let legacy = SqliteRegistry::new(pool, support::publisher())
        .get(&SeatId::from("pij-legacy"))
        .await
        .expect("get")
        .expect("the row survives the migration");
    assert_eq!(
        legacy.spawn_id, None,
        "a pre-0003 row has no bind evidence, not a fake one"
    );
    assert_eq!(legacy.folder, "/abs/old");
}
