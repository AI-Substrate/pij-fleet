//! A delivery waiting behind a non-blank composer costs no process launch
//! (plan 184, AGENTS.md § Resource cost).
//!
//! Before plan 184 every attempt reserved the pane first, `tmux.atomically
//! reserve staged submit` plus `tmux.restore pane input and release ownership`:
//! two launches per held seat per drain tick, forever (pij-fleet#86 measured
//! 0.8/s for two stuck seats). The gate now looks through the control client
//! and reserves only a composer that is submittable.
//!
//! ONE test on purpose: it reads the process-wide launch counter.

use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use pij_core::delivery::DeliveryDeferralReason;
use pij_core::ports::TmuxPort;
use pij_core::spawns::snapshot;
use pij_harnesses::{InteractionGate, StagedSubmission};
use pij_testkit::tmux::PrivateTmux;
use pij_tmux::{TmuxAdapter, tap_sink_path};

#[tokio::test]
async fn a_held_delivery_waits_without_launching_anything() {
    let server = PrivateTmux::start();
    // A Claude-shaped composer holding a human draft, caret inside it.
    let window = Command::new("tmux")
        .args(["-L", server.name(), "new-window", "-d", "-t", "test", "-n", "seat"])
        .arg(
            "printf '────────────────\\n❯ parked human draft\\n────────────────\\033[2;10H'; exec sleep 600",
        )
        .env_remove("TMUX")
        .output()
        .expect("tmux new-window");
    assert!(window.status.success(), "{window:?}");
    let tap_root = std::env::temp_dir().join(format!("pij-held-launches-{}", std::process::id()));
    std::fs::create_dir_all(&tap_root).expect("tap root");
    let tmux = Arc::new(TmuxAdapter::for_server(server.name(), &tap_root));
    let pane = tmux
        .list_panes()
        .await
        .expect("list private panes")
        .into_iter()
        .find(|pane| pane.window == "seat")
        .expect("seat pane")
        .id;
    tmux.attach_pane_tap(&pane, &tap_sink_path(&tap_root, &pane))
        .await
        .expect("tap the seat");
    let gate = InteractionGate::with_typing_grace(
        Arc::clone(&tmux) as Arc<dyn TmuxPort>,
        Duration::from_secs(60),
        0,
    );
    // Warm-up: the control client attaches once; the draft renders.
    let mut held = None;
    for _ in 0..50 {
        held = Some(gate.submit_with_owned_input(&pane, "/compact").await);
        if matches!(
            held,
            Some(Ok(StagedSubmission::Deferred {
                reason: DeliveryDeferralReason::ComposerBusy,
                ..
            }))
        ) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        matches!(
            held,
            Some(Ok(StagedSubmission::Deferred {
                reason: DeliveryDeferralReason::ComposerBusy,
                ..
            }))
        ),
        "the draft must read as composer-busy: {held:?}"
    );

    let before = snapshot();
    for _ in 0..20 {
        assert!(matches!(
            gate.submit_with_owned_input(&pane, "/compact").await,
            Ok(StagedSubmission::Deferred {
                reason: DeliveryDeferralReason::ComposerBusy,
                ..
            })
        ));
    }
    let after = snapshot();

    let launched: Vec<(String, u64)> = after
        .by_op
        .iter()
        .map(|(op, count)| {
            (
                op.clone(),
                count - before.by_op.get(op).copied().unwrap_or(0),
            )
        })
        .filter(|(_, delta)| *delta > 0)
        .collect();
    assert!(
        launched.is_empty(),
        "20 held attempts launched {launched:?}"
    );
    let _ = std::fs::remove_dir_all(tap_root);
}
