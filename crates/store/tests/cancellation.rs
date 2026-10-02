use std::sync::{Arc, mpsc};
use std::time::Duration;

use pij_core::model::{Event, Seq};
use pij_core::ports::Spine;
use pij_store::SqliteSpine;

#[tokio::test]
async fn cancelling_an_admitted_write_cannot_interrupt_commit() {
    for timeout in [false, true] {
        let pool = pij_store::open("").await.unwrap();
        let spine = Arc::new(SqliteSpine::new(pool.clone()));
        let entered = Arc::new(tokio::sync::Notify::new());
        let entered_hook = entered.clone();
        let (release, receive) = mpsc::channel();
        let mut receive = Some(receive);
        let mut connection = pool.acquire().await.unwrap();
        connection
            .lock_handle()
            .await
            .unwrap()
            .set_update_hook(move |_| {
                if let Some(receive) = receive.take() {
                    entered_hook.notify_one();
                    // Real SQLite is paused inside INSERT, after BEGIN. Timeout only
                    // prevents a failed test leaving its worker thread blocked.
                    let _ = receive.recv_timeout(Duration::from_secs(5));
                }
            });
        drop(connection);
        let writer = spine.clone();
        let (cancel, cancelled) = tokio::sync::oneshot::channel();
        let caller = tokio::spawn(async move {
            let write = writer.append(Event {
                seq: None,
                v: 1,
                at: 1,
                kind: "cancel-proof".into(),
                seat: None,
                payload: "{}".into(),
            });
            if timeout {
                tokio::pin!(write);
                tokio::select! {
                    result = &mut write => panic!("write completed before hook release: {result:?}"),
                    _ = cancelled => {}
                }
                assert!(
                    tokio::time::timeout(Duration::from_millis(1), write)
                        .await
                        .is_err()
                );
            } else {
                write.await.unwrap();
            }
        });
        tokio::time::timeout(Duration::from_secs(2), entered.notified())
            .await
            .unwrap();
        if timeout {
            cancel.send(()).unwrap();
            caller.await.unwrap();
        } else {
            caller.abort();
            assert!(caller.await.unwrap_err().is_cancelled());
        }
        release.send(()).unwrap();
        let events = tokio::time::timeout(Duration::from_secs(2), spine.tail(None, Seq(0)))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            events.len(),
            1,
            "admitted INSERT must commit after caller cancellation; timeout={timeout}"
        );
        assert_eq!(events[0].kind, "cancel-proof");
        assert_eq!(
            pij_store::schema_version(&pool).await.unwrap(),
            pij_store::SCHEMA_VERSION
        );
    }
}
