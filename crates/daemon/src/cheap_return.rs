//! Plan 225b: `revive --cheap` relaunches a cold Claude seat on Sonnet and
//! compacts it there, then puts the seat back on its own model.
//!
//! The switch back happens inside the running session: restarting the
//! process mid-compaction would kill the compaction. Measured on Claude Code
//! 2.1.321 (the plan's step-1 gate): a `/model <id>` typed while `/compact`
//! runs opens a "Switch model?" dialog at once; confirming it applies the
//! switch, and the compaction finishes undisturbed. The same `/model` also
//! rewrites the user's default `"model"` in every new session's
//! `settings.json`, so each Claude home's value is saved first and put back
//! straight after.
//!
//! Cost per use, never per tick: one live event subscription until the compact
//! starts (bounded by [`Timing::start_wait`]), one 5 s wait, one typed
//! `/model`, one pane read, at most one Enter, one settings read and write per
//! Claude home, one registry write and one audit event. No loop, poll or timer
//! beyond those waits.

use std::path::PathBuf;
use std::time::Duration;

use pij_core::events::EventFilter;
use pij_core::model::{Event, SeatId};
use pij_core::wire;
use pij_harnesses::{ModelRestore, StagedSubmission, restore_claude_models, save_claude_models};
use serde_json::{Value, json};
use tokio_stream::StreamExt;

use crate::Services;
use crate::events::Subscription;

/// The dialog Claude Code shows for a `/model` on a cached conversation.
pub const SWITCH_MODAL: &str = "Switch model?";

/// The audit event: what the switch back did, settings included.
pub const RETURN_KIND: &str = "revive.model-returned";

/// The waits, injectable so tests run them on a paused clock.
#[derive(Clone, Copy, Debug)]
pub struct Timing {
    /// After the compaction starts (the `/compact` was typed), before `/model`.
    pub after_start: Duration,
    /// After `/model`, before the one pane read for the dialog.
    pub modal: Duration,
    /// After the switch, before settings are put back (Claude writes them).
    pub settle: Duration,
    /// How long the compact may take to start: its hold limit is 7 minutes.
    pub start_wait: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            after_start: Duration::from_secs(5),
            modal: Duration::from_millis(1_500),
            settle: Duration::from_secs(2),
            start_wait: Duration::from_secs(15 * 60),
        }
    }
}

/// One pending switch back.
#[derive(Clone, Debug)]
pub struct Return {
    /// The seat revived with `--cheap`.
    pub seat: SeatId,
    /// The msg_id the caller's compact control goes under.
    pub compact_msg_id: String,
    /// The seat's recorded model before `--cheap`: never assumed to be Opus.
    pub model: String,
    /// The Claude homes whose `"model"` is saved and put back.
    pub homes: Vec<PathBuf>,
}

/// Subscribe now, before the caller can send the compact, then follow it.
pub fn arm(services: &Services, pending: Return, timing: Timing) -> tokio::task::JoinHandle<()> {
    let events = services
        .event_bus
        .subscribe_live(EventFilter::kinds(["delivery.outcome"]).for_seat(pending.seat.clone()));
    let services = services.clone();
    tokio::spawn(async move { follow(&services, events, &pending, timing).await })
}

async fn follow(services: &Services, mut events: Subscription, pending: &Return, timing: Timing) {
    let deadline = tokio::time::Instant::now() + timing.start_wait;
    let started = loop {
        match tokio::time::timeout_at(deadline, events.next()).await {
            Err(_) | Ok(None) => break false,
            Ok(Some(event)) => {
                if let Some(started) = compact_started(&event, &pending.compact_msg_id) {
                    break started;
                }
            }
        }
    };
    let outcome = if started {
        tokio::time::sleep(timing.after_start).await;
        switch_back(services, pending, timing).await
    } else {
        Err("the compaction never started, so the seat stays on Sonnet".to_string())
    };
    audit(services, pending, &outcome).await;
}

/// Whether `event` settles the compact: `Some(true)` typed into the pane (the
/// compaction starts), `Some(false)` refused or failed, `None` not yet.
fn compact_started(event: &Event, msg_id: &str) -> Option<bool> {
    let payload: Value = serde_json::from_str(&event.payload).ok()?;
    if payload["msg_id"].as_str() != Some(msg_id) {
        return None;
    }
    match payload["outcome"]["outcome"].as_str()? {
        "delivered" => Some(true),
        "queued" | "held" => None,
        _ => Some(false),
    }
}

/// What the switch back did.
#[derive(Debug)]
pub struct Switched {
    /// `/model` was typed into the pane.
    pub typed: bool,
    /// The "Switch model?" dialog showed and Enter was pressed.
    pub confirmed: bool,
    /// The switch was seen to apply: Claude wrote the model into a settings
    /// file whose saved value was something else. Only then is the seat's
    /// recorded model set to it (review of #215, F002).
    pub applied: bool,
    /// `/model` was typed but no settings file could show whether it applied:
    /// each home's default already was the model (or there was no home). The
    /// seat is on that model or still on Sonnet, so its recorded model becomes
    /// unknown rather than either (review of #215 round 2, S215-002).
    pub unconfirmed: bool,
    /// The seat's recorded model before the switch: the last one confirmed.
    pub last_confirmed: Option<String>,
    /// Why it did not apply, when it didn't.
    pub why: Option<String>,
    /// Each Claude home's `"model"`: found after the switch, and left.
    pub settings: Vec<ModelRestore>,
}

/// A model id pij will type into a pane: the closed grammar of Claude's ids
/// (`claude-opus-5-5`, `claude-opus-5-5[1m]`), never free text.
#[must_use]
pub fn is_model_id(model: &str) -> bool {
    (1..=80).contains(&model.len())
        && model.as_bytes()[0].is_ascii_alphanumeric()
        && model
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b".-_[]".contains(&byte))
}

/// Claude Code's "Switch model?" dialog, matched by its title and its
/// confirming choice, never a bare substring.
fn shows_switch_dialog(screen: &str) -> bool {
    let lines: Vec<&str> = screen.lines().map(str::trim).collect();
    lines.contains(&SWITCH_MODAL)
        && lines.iter().any(|line| {
            line.trim_start_matches('❯')
                .trim()
                .starts_with("1. Yes, switch to")
        })
}

/// Type `/model <recorded model>`, confirm the dialog only if it shows, and
/// put each home's settings back on every path. The seat's model is recorded
/// only when the switch is seen to apply.
///
/// # Errors
/// Before anything is typed: the seat is gone or has no pane, the model id is
/// not one pij types, or the settings snapshot failed.
pub async fn switch_back(
    services: &Services,
    pending: &Return,
    timing: Timing,
) -> Result<Switched, String> {
    if !is_model_id(&pending.model) {
        return Err("the recorded model is not a model id pij types".to_string());
    }
    let seat = services
        .registry
        .get(&pending.seat)
        .await
        .map_err(|error| error.to_string())?
        .filter(|seat| seat.tombstoned_at.is_none())
        .ok_or("the seat is no longer live")?;
    let pane = seat.pane.clone().ok_or("the seat has no pane")?;
    let saved = save_claude_models(&pending.homes)
        .map_err(|why| format!("settings snapshot failed, so nothing was switched: {why}"))?;
    let mut switched = Switched {
        typed: false,
        confirmed: false,
        applied: false,
        unconfirmed: false,
        last_confirmed: seat.model.clone(),
        why: None,
        settings: Vec::new(),
    };
    let attempt = async {
        match services
            .interaction
            .submit_with_owned_input(&pane, &format!("/model {}", pending.model))
            .await
        {
            Ok(StagedSubmission::Submitted) => switched.typed = true,
            Ok(_) => return Err("/model was held at the composer and not typed".to_string()),
            Err(error) => return Err(format!("/model was not typed: {error}")),
        }
        tokio::time::sleep(timing.modal).await;
        // One read. A pane that can't be read, or shows no dialog, presses
        // nothing: pij answers only the dialog it opened.
        if let Ok(screen) = services.tmux.capture(&pane, 40).await
            && shows_switch_dialog(&screen)
        {
            services
                .tmux
                .press_enter(&pane)
                .await
                .map_err(|error| format!("the dialog was not confirmed: {error}"))?;
            switched.confirmed = true;
        }
        tokio::time::sleep(timing.settle).await;
        Ok(())
    };
    let attempted = attempt.await;
    // Every path, failures included, puts the settings back.
    switched.settings = restore_claude_models(&saved);
    let target = Some(pending.model.as_str());
    let witness = saved.iter().zip(&switched.settings).any(|(before, after)| {
        before.model().as_deref() != target && after.found.as_deref() == target
    });
    // A home whose default already was the model can't show the switch: it
    // reads the same whether or not it applied.
    let blind = saved.is_empty()
        || saved
            .iter()
            .any(|before| before.model().as_deref() == target);
    switched.why = match attempted {
        Err(why) if !switched.typed => Some(why),
        _ if witness => None,
        Err(why) if !blind => Some(why),
        Ok(()) if !blind => Some(
            "the switch was not seen to apply (no settings file took the model; the dialog may have come late), so the seat stays on Sonnet"
                .to_string(),
        ),
        attempted => {
            switched.unconfirmed = true;
            let failed = attempted.err().map(|why| format!("{why}; ")).unwrap_or_default();
            Some(format!(
                "{failed}the switch could not be confirmed (the default model already was {}, so settings can't show it), so the seat's model is recorded as unknown: it is on {} or still on Sonnet",
                pending.model, pending.model
            ))
        }
    };
    if let Some(error) = switched
        .settings
        .iter()
        .find_map(|restore| restore.error.clone())
    {
        eprintln!(
            "pij-rs revive --cheap: settings not put back for {}: {error}",
            pending.seat
        );
    }
    if switched.why.is_none() || switched.unconfirmed {
        let mut seat = services
            .registry
            .get(&pending.seat)
            .await
            .map_err(|error| error.to_string())?
            .ok_or("the seat is gone")?;
        seat.model = if switched.unconfirmed {
            None
        } else {
            Some(pending.model.clone())
        };
        services
            .registry
            .put(seat)
            .await
            .map_err(|error| error.to_string())?;
        switched.applied = !switched.unconfirmed;
    }
    Ok(switched)
}

async fn audit(services: &Services, pending: &Return, outcome: &Result<Switched, String>) {
    let settings = |restored: &[ModelRestore]| {
        restored
            .iter()
            .map(|restore| {
                json!({
                    "home": restore.home,
                    "found": restore.found,
                    "left": restore.left,
                    "exact": restore.exact,
                    "error": restore.error,
                })
            })
            .collect::<Vec<_>>()
    };
    let payload = match outcome {
        Ok(switched) => json!({
            "model": pending.model,
            "compact_msg_id": pending.compact_msg_id,
            "returned": switched.applied,
            "unconfirmed": switched.unconfirmed,
            "last_confirmed_model": switched.last_confirmed,
            "typed": switched.typed,
            "confirmed": switched.confirmed,
            "why": switched.why,
            "settings": settings(&switched.settings),
            "settings_restored": switched.settings.iter().all(|restore| restore.error.is_none()),
        }),
        Err(why) => json!({
            "model": pending.model,
            "compact_msg_id": pending.compact_msg_id,
            "returned": false,
            "typed": false,
            "why": why,
        }),
    };
    let Ok(at) = crate::http::system_time_ms() else {
        return;
    };
    let event = Event {
        seq: None,
        v: wire::EVENT_VERSION,
        at,
        kind: RETURN_KIND.to_string(),
        seat: Some(pending.seat.clone()),
        payload: payload.to_string(),
    };
    if let Err(error) = services.event_bus.publish(event).await {
        eprintln!(
            "pij-rs revive --cheap: {RETURN_KIND} for {}: {error}",
            pending.seat
        );
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use pij_core::config::Config;
    use pij_core::model::{Harness, ProcIdentity, SeatDescriptor};
    use pij_testkit::FreshStore;
    use pij_testkit::fakes::{FakeRegistry, FakeSpine, FakeTmux};

    use super::*;

    const SEAT: &str = "pij-cheap";
    const PANE: &str = "%cheap";

    async fn rig(store: &FreshStore) -> (Services, Arc<FakeTmux>) {
        let config = Config {
            store_path: store.path(),
            ..Default::default()
        };
        let mut services =
            crate::build_services(&config, std::path::Path::new("/tmp/pij-cheap-taps"))
                .await
                .expect("services");
        // Pure in-memory registry and spine: a store write on a paused clock
        // reads as an idle runtime, which jumps time to the next deadline.
        services.registry = Arc::new(FakeRegistry::new());
        services.event_bus =
            Arc::new(crate::events::EventBus::new(Arc::new(FakeSpine::new()), 64).unwrap());
        let tmux = Arc::new(FakeTmux::new());
        tmux.arrange_clear_composer(PANE);
        services.tmux = tmux.clone();
        services.interaction = Arc::new(pij_harnesses::InteractionGate::new(tmux.clone()));
        let mut seat = SeatDescriptor::new(SEAT, Harness::Claude, "/tmp");
        seat.pane = Some(PANE.to_string());
        seat.proc = Some(ProcIdentity {
            pid: 4343,
            proc_start: 1,
        });
        seat.model = Some("claude-sonnet-5-5".to_string());
        services.registry.put(seat).await.unwrap();
        (services, tmux)
    }

    async fn outcome(services: &Services, msg_id: &str, outcome: &str) {
        services
            .event_bus
            .publish(Event {
                seq: None,
                v: wire::EVENT_VERSION,
                at: 1,
                kind: "delivery.outcome".to_string(),
                seat: Some(SeatId::from(SEAT)),
                payload: json!({"msg_id": msg_id, "outcome": {"outcome": outcome}}).to_string(),
            })
            .await
            .unwrap();
    }

    fn typed_model(tmux: &FakeTmux) -> bool {
        tmux.calls()
            .iter()
            .any(|call| call.contains("/model claude-fable-5"))
    }

    async fn run_for(duration: Duration) {
        let end = tokio::time::Instant::now() + duration;
        while tokio::time::Instant::now() < end {
            tokio::time::advance(Duration::from_millis(100)).await;
            tokio::task::yield_now().await;
        }
    }

    /// Plan 225b: `--cheap` puts the seat back on its own model (here not
    /// Opus) 5 s after its compaction STARTS, confirms the "Switch model?"
    /// dialog only when it shows, leaves the user's default model as it was
    /// (every other byte too), and records the seat's model again.
    #[tokio::test]
    async fn the_switch_back_comes_five_seconds_after_the_compaction_starts() {
        let store = FreshStore::new();
        let (services, tmux) = rig(&store).await;
        // Built on the real clock (the store's pool times out on a paused one).
        tokio::time::pause();
        let home = std::env::temp_dir().join(format!("pij-cheap-home-{}", std::process::id()));
        std::fs::create_dir_all(&home).unwrap();
        let before = "{\n  \"theme\": \"dark\",\n  \"model\": \"sonnet\"\n}\n";
        std::fs::write(home.join("settings.json"), before).unwrap();
        let task = arm(
            &services,
            Return {
                seat: SeatId::from(SEAT),
                compact_msg_id: "m-compact".to_string(),
                model: "claude-fable-5".to_string(),
                homes: vec![home.clone()],
            },
            Timing {
                start_wait: Duration::from_secs(600),
                ..Timing::default()
            },
        );
        outcome(&services, "m-other", "delivered").await;
        outcome(&services, "m-compact", "queued").await;
        run_for(Duration::from_secs(30)).await;
        assert!(!typed_model(&tmux), "nothing before the compaction starts");

        outcome(&services, "m-compact", "delivered").await;
        run_for(Duration::from_millis(4_800)).await;
        assert!(!typed_model(&tmux), "not before 5 s");
        run_for(Duration::from_millis(400)).await;
        assert!(typed_model(&tmux), "{:?}", tmux.calls());
        // What Claude Code does: the dialog shows, and confirming rewrites the default.
        tmux.queue_capture(format!("  {SWITCH_MODAL}\n   ❯ 1. Yes, switch to Fable"));
        std::fs::write(
            home.join("settings.json"),
            before.replace("\"sonnet\"", "\"claude-fable-5\""),
        )
        .unwrap();
        run_for(Duration::from_secs(4)).await;
        task.await.unwrap();

        assert!(
            tmux.calls()
                .iter()
                .any(|call| call == &format!("enter:{PANE}")),
            "{:?}",
            tmux.calls()
        );
        assert_eq!(
            std::fs::read_to_string(home.join("settings.json")).unwrap(),
            before
        );
        let seat = services
            .registry
            .get(&SeatId::from(SEAT))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(seat.model.as_deref(), Some("claude-fable-5"));
        let returned = services
            .event_bus
            .latest_matching(&SeatId::from(SEAT), &[RETURN_KIND])
            .await
            .unwrap()
            .expect("audited");
        let payload: Value = serde_json::from_str(&returned.payload).unwrap();
        assert_eq!(payload["returned"], true, "{payload}");
        assert_eq!(payload["confirmed"], true, "{payload}");
        assert_eq!(
            payload["settings"][0]["found"], "claude-fable-5",
            "{payload}"
        );
        assert_eq!(payload["settings"][0]["left"], "sonnet", "{payload}");
        assert_eq!(payload["settings"][0]["exact"], true, "{payload}");
        std::fs::remove_dir_all(&home).unwrap();
    }

    fn zero() -> Timing {
        Timing {
            after_start: Duration::ZERO,
            modal: Duration::ZERO,
            settle: Duration::ZERO,
            start_wait: Duration::ZERO,
        }
    }

    fn settings_home(name: &str, text: &str) -> std::path::PathBuf {
        let home = std::env::temp_dir().join(format!(
            "pij-cheap-{name}-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
        ));
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(home.join("settings.json"), text).unwrap();
        home
    }

    fn pending(model: &str, homes: Vec<std::path::PathBuf>) -> Return {
        Return {
            seat: SeatId::from(SEAT),
            compact_msg_id: "m-compact".to_string(),
            model: model.to_string(),
            homes,
        }
    }

    /// Review of #215, F002: the dialog came late (not on screen at the one
    /// read), so nothing is pressed and the switch never applies. The seat
    /// keeps its true model (Sonnet), the return is reported incomplete, and
    /// settings are byte-identical.
    #[tokio::test]
    async fn a_late_dialog_leaves_the_seat_on_its_true_model_and_reports_it() {
        let store = FreshStore::new();
        let (services, tmux) = rig(&store).await;
        let before = "{\n  \"model\": \"sonnet\"\n}\n";
        let home = settings_home("late", before);
        let switched = switch_back(
            &services,
            &pending("claude-fable-5", vec![home.clone()]),
            zero(),
        )
        .await
        .unwrap();
        assert!(
            switched.typed && !switched.confirmed && !switched.applied,
            "{switched:?}"
        );
        assert!(
            switched
                .why
                .as_deref()
                .unwrap()
                .contains("not seen to apply"),
            "{switched:?}"
        );
        assert!(!tmux.calls().iter().any(|call| call.starts_with("enter:")));
        let seat = services
            .registry
            .get(&SeatId::from(SEAT))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            seat.model.as_deref(),
            Some("claude-sonnet-5-5"),
            "its true model"
        );
        assert_eq!(
            std::fs::read_to_string(home.join("settings.json")).unwrap(),
            before
        );
        std::fs::remove_dir_all(&home).unwrap();
    }

    /// Review of #215 round 2, S215-002: the user's default already is the
    /// seat's own model, so the switch back rewrites it to the same value and
    /// settings can't show whether it applied. The seat is not claimed to be
    /// on Sonnet (nor on its model): its recorded model becomes unknown, and
    /// the audit says the return is unconfirmed and names the last confirmed
    /// model. Settings stay byte-identical.
    #[tokio::test]
    async fn a_default_already_on_the_model_leaves_the_return_unconfirmed() {
        let store = FreshStore::new();
        let (services, tmux) = rig(&store).await;
        let before = "{\n  \"model\": \"claude-fable-5\"\n}\n";
        let home = settings_home("same", before);
        let pending = pending("claude-fable-5", vec![home.clone()]);
        tokio::time::pause();
        let task = {
            let (services, pending) = (services.clone(), pending.clone());
            tokio::spawn(async move {
                let timing = Timing {
                    modal: Duration::from_secs(1),
                    ..zero()
                };
                switch_back(&services, &pending, timing).await
            })
        };
        run_for(Duration::from_millis(500)).await;
        assert!(typed_model(&tmux), "{:?}", tmux.calls());
        // The dialog shows; Claude rewrites the default to the value it had.
        tmux.queue_capture(format!("  {SWITCH_MODAL}\n   ❯ 1. Yes, switch to Fable"));
        run_for(Duration::from_secs(1)).await;
        let outcome = task.await.unwrap();
        let switched = outcome.as_ref().unwrap();
        assert!(
            switched.typed && switched.confirmed && !switched.applied && switched.unconfirmed,
            "{switched:?}"
        );
        assert!(
            tmux.calls()
                .iter()
                .any(|call| call == &format!("enter:{PANE}")),
            "{:?}",
            tmux.calls()
        );
        let seat = services
            .registry
            .get(&SeatId::from(SEAT))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(seat.model, None, "neither Sonnet nor its model is asserted");
        audit(&services, &pending, &outcome).await;
        let returned = services
            .event_bus
            .latest_matching(&SeatId::from(SEAT), &[RETURN_KIND])
            .await
            .unwrap()
            .expect("audited");
        let payload: Value = serde_json::from_str(&returned.payload).unwrap();
        assert_eq!(payload["returned"], false, "{payload}");
        assert_eq!(payload["unconfirmed"], true, "{payload}");
        assert_eq!(
            payload["last_confirmed_model"], "claude-sonnet-5-5",
            "{payload}"
        );
        assert!(
            !payload["why"].as_str().unwrap().contains("stays on Sonnet"),
            "{payload}"
        );
        assert_eq!(
            std::fs::read_to_string(home.join("settings.json")).unwrap(),
            before
        );
        std::fs::remove_dir_all(&home).unwrap();
    }

    /// Review of #215, F001: a settings snapshot that fails refuses before
    /// anything is typed; a model id outside the grammar is never typed.
    #[tokio::test]
    async fn a_failed_snapshot_or_a_bad_model_id_types_nothing() {
        let store = FreshStore::new();
        let (services, tmux) = rig(&store).await;
        let home = settings_home("invalid", "{ not json");
        let before = tmux.calls().len();
        let error = switch_back(
            &services,
            &pending("claude-fable-5", vec![home.clone()]),
            zero(),
        )
        .await
        .unwrap_err();
        assert!(error.contains("snapshot failed"), "{error}");
        let forged = switch_back(&services, &pending("x\n/clear", Vec::new()), zero())
            .await
            .unwrap_err();
        assert!(forged.contains("not a model id"), "{forged}");
        assert_eq!(
            tmux.calls().len(),
            before,
            "nothing typed: {:?}",
            tmux.calls()
        );
        std::fs::remove_dir_all(&home).unwrap();
    }

    /// A compact that never starts switches nothing and says so.
    #[tokio::test]
    async fn a_compact_that_never_starts_switches_nothing() {
        let store = FreshStore::new();
        let (services, tmux) = rig(&store).await;
        tokio::time::pause();
        let task = arm(
            &services,
            Return {
                compact_msg_id: "m-never".to_string(),
                ..pending("claude-fable-5", Vec::new())
            },
            Timing {
                start_wait: Duration::from_secs(60),
                ..Timing::default()
            },
        );
        outcome(&services, "m-never", "refused").await;
        run_for(Duration::from_secs(10)).await;
        task.await.unwrap();
        assert!(!typed_model(&tmux), "nothing typed");
        let returned = services
            .event_bus
            .latest_matching(&SeatId::from(SEAT), &[RETURN_KIND])
            .await
            .unwrap()
            .expect("audited");
        assert!(
            returned.payload.contains("\"returned\":false"),
            "{}",
            returned.payload
        );
    }
}
