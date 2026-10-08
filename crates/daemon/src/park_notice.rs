//! Tell a sender, once, that its message parked undelivered (plan 167 R3).
//!
//! Every park commits a sender-addressed `delivery.parked` fact, but a spine fact
//! wakes nobody: a sender that saw `queued` never learnt its message was lost.
//! Modelled on the death sweep's obituary, `pij-bg` sends one short notice and
//! records `delivery.park-notice`, so a replayed park is never re-sent. A sender
//! with no live seat (missing, tombstoned, remote, or `pij-bg` itself) gets
//! nothing, and that is not an error.

use pij_core::BG_ACTOR;
use pij_core::error::Result;
use pij_core::events::EventFilter;
use pij_core::model::{DeliveryFailure, Event, JobId, Seq};
use pij_core::ports::Spine;
use serde::Deserialize;
use serde_json::json;
use tokio_stream::StreamExt;

use crate::Services;

const PARKED_KIND: &str = "delivery.parked";
/// Durable once-per-job record of the notice, keyed by `msg_id` for lookup.
pub const NOTICE_KIND: &str = "delivery.park-notice";

/// The canonical `delivery.parked` payload (`pij_core::delivery::parked_events`).
#[derive(Deserialize)]
struct Parked {
    #[serde(rename = "messageId")]
    message_id: String,
    #[serde(rename = "jobId")]
    job_id: JobId,
    recipient: String,
    outcome: DeliveryFailure,
    reason: String,
}

fn body(parked: &Parked) -> String {
    format!(
        "[pij] message {} to {} was not delivered ({}: {}). Resend after the target recovers.",
        parked.message_id,
        parked.recipient,
        parked.outcome.as_str(),
        parked.reason
    )
}

/// Notify the sender of one parked job, at most once per job.
///
/// # Errors
/// Registry, spine, delivery or event-publication failures.
pub async fn notify(services: &Services, event: &Event) -> Result<()> {
    let Some(sender) = &event.seat else {
        return Ok(());
    };
    // User spine kinds are open strings; a same-named event that does not
    // decode as a park receipt is not one.
    let Ok(parked) = serde_json::from_str::<Parked>(&event.payload) else {
        return Ok(());
    };
    let live = services
        .registry
        .get(sender)
        .await?
        .is_some_and(|seat| seat.tombstoned_at.is_none());
    if !live {
        return Ok(());
    }
    let notified = services
        .event_bus
        .latest_matching_message(sender, NOTICE_KIND, &parked.message_id)
        .await?
        .and_then(|prior| serde_json::from_str::<serde_json::Value>(&prior.payload).ok())
        .is_some_and(|prior| prior["job_id"] == json!(parked.job_id));
    if notified {
        return Ok(());
    }
    let notice = services
        .delivery
        .send(BG_ACTOR.into(), sender.clone(), body(&parked))
        .await?;
    services
        .event_bus
        .publish(Event {
            seq: None,
            v: 1,
            at: event.at,
            kind: NOTICE_KIND.into(),
            seat: Some(sender.clone()),
            payload: json!({
                "msg_id": parked.message_id,
                "job_id": parked.job_id,
                "recipient": parked.recipient,
                "notice": notice,
            })
            .to_string(),
        })
        .await?;
    Ok(())
}

async fn notify_logged(services: &Services, event: &Event) {
    if let Err(error) = notify(services, event).await {
        eprintln!("pij-rs park notice failed: {error}");
    }
}

/// Follow live parks for the daemon's lifetime. A lagged subscriber replays the
/// spine after the last park it handled; the per-job record keeps that idempotent.
/// Failures are logged per notice and never stop the follower.
pub async fn follow(services: Services) {
    let mut events = services
        .event_bus
        .subscribe_live(EventFilter::kinds([PARKED_KIND]));
    let mut dropped = 0;
    let mut last: Option<Seq> = None;
    while let Some(event) = events.next().await {
        if events.dropped_count() != dropped {
            dropped = events.dropped_count();
            match last {
                Some(since) => match services.event_bus.tail(None, since).await {
                    Ok(missed) => {
                        for missed in missed {
                            if missed.kind == PARKED_KIND && missed.seq < event.seq {
                                notify_logged(&services, &missed).await;
                            }
                        }
                    }
                    Err(error) => eprintln!("pij-rs park notice: lag replay failed: {error}"),
                },
                None => eprintln!("pij-rs park notice: subscriber lagged before its first park"),
            }
        }
        notify_logged(&services, &event).await;
        last = event.seq;
    }
    eprintln!("pij-rs park notice: delivery.parked subscription ended");
}
