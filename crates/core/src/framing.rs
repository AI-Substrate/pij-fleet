//! Canonical framing for messages injected by the Rust daemon.

use crate::model::SeatId;

const LEGACY_HEAD: &str = "[pij from ";
const RUST_HEAD: &str = "[pij-rs from ";
const TAIL: &str = "[/pij]";

/// Frame one pij-rs payload with an addressable sender and an explicit tail.
///
/// Pointers use this same shape even though they carry no message body. Keeping
/// every injection uniform means readers never need to know which kind arrived;
/// callers supply payload text and this function alone owns the envelope.
#[must_use]
pub fn frame_message(from: &SeatId, from_machine: Option<&str>, payload: &str) -> String {
    let machine_len = from_machine.map_or(0, |machine| machine.len() + 1);
    let mut framed = String::with_capacity(
        RUST_HEAD.len() + from.as_str().len() + machine_len + payload.len() + TAIL.len() + 3,
    );
    framed.push_str(RUST_HEAD);
    framed.push_str(from.as_str());
    if let Some(machine) = from_machine {
        framed.push('@');
        framed.push_str(machine);
    }
    framed.push_str("]\n");
    framed.push_str(payload);
    framed.push('\n');
    framed.push_str(TAIL);
    framed
}

/// Extract the payload of a standard Rust or legacy pij message envelope.
#[must_use]
pub fn message_payload(frame: &str) -> Option<&str> {
    rust_parts(frame)
        .or_else(|| legacy_parts(frame))
        .map(|(_, payload)| payload)
}

/// Compare a recorded injection with observed composer text.
///
/// Exact whitespace-collapsed echoes remain exempt. During the generation
/// cutover, the equivalent legacy head/body and pij-rs head/body/tail are also
/// equivalent; any adjacent or changed human text remains a mismatch.
#[must_use]
pub fn self_injection_matches(recorded: &str, observed: &str) -> bool {
    if collapsed_eq(recorded, observed) {
        return true;
    }

    matches!(
        (legacy_parts(recorded), rust_parts(observed)),
        (Some(recorded), Some(observed))
            if recorded.0 == observed.0 && collapsed_eq(recorded.1, observed.1)
    )
}

fn collapsed_eq(left: &str, right: &str) -> bool {
    left.split_whitespace().eq(right.split_whitespace())
}

fn legacy_parts(frame: &str) -> Option<(&str, &str)> {
    let (sender, payload) = frame.strip_prefix(LEGACY_HEAD)?.split_once(']')?;
    Some((sender, payload.trim_start_matches(char::is_whitespace)))
}

fn rust_parts(frame: &str) -> Option<(&str, &str)> {
    let (sender, payload_and_tail) = frame.strip_prefix(RUST_HEAD)?.split_once(']')?;
    let payload = payload_and_tail
        .trim_start_matches(char::is_whitespace)
        .strip_suffix(TAIL)?
        .trim_end_matches(char::is_whitespace);
    Some((sender, payload))
}

#[cfg(test)]
mod tests {
    use super::{frame_message, self_injection_matches};
    use crate::model::SeatId;

    #[test]
    fn frame_names_local_and_machine_qualified_senders() {
        assert_eq!(
            frame_message(&SeatId::from("pij-a"), None, "hello"),
            "[pij-rs from pij-a]\nhello\n[/pij]"
        );
        assert_eq!(
            frame_message(&SeatId::from("pij-a"), Some("desktop"), "hello"),
            "[pij-rs from pij-a@desktop]\nhello\n[/pij]"
        );
    }

    #[test]
    fn generation_transition_matches_only_the_same_message() {
        let legacy = "[pij from pij-a] hello there";
        let current = "[pij-rs from pij-a]\nhello there\n[/pij]";
        assert!(self_injection_matches(legacy, current));
        assert!(!self_injection_matches(
            legacy,
            "[pij-rs from pij-a]\nhello there and human text\n[/pij]"
        ));
        assert!(!self_injection_matches(
            legacy,
            "[pij-rs from pij-b]\nhello there\n[/pij]"
        ));
    }
}
