//! Closed renderings of caller- and model-supplied text for daemon-authored
//! messages (F127, F132).
//!
//! A `pij-bg` message, a watchdog nudge or a generated notice reaches a model
//! as the daemon's own words. Any field in it that a seat, a client or a model
//! chose is shown only when it fits that field's grammar, and otherwise as its
//! length (`invalid (N chars)`), so it can never carry a newline, forged
//! `[pij from ...]` framing or instructions. Free text a message exists to
//! carry (a reason, a body) is fenced instead: escaped onto one line inside
//! quotes, labelled with who wrote it.

use std::borrow::Cow;

use crate::model::{invalid_text, is_canonical_seat_id};
use crate::orchestration::SEAT_ROLES;

fn closed(raw: &str, fits: bool) -> Cow<'_, str> {
    if fits {
        Cow::Borrowed(raw)
    } else {
        Cow::Owned(invalid_text(raw))
    }
}

/// A seat id: the closed seat-id grammar ([`crate::model::SeatId::is_canonical`]).
pub fn seat(id: &str) -> Cow<'_, str> {
    closed(id, is_canonical_seat_id(id))
}

/// A message or request id: letters, digits, `.`, `_`, `:` and `-`, at most
/// 128 (every id pij and its clients mint). The id is caller-chosen.
pub fn msg_id(id: &str) -> Cow<'_, str> {
    closed(
        id,
        (1..=128).contains(&id.len())
            && id.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'-')
            }),
    )
}

/// Whether `name` is a directory name a daemon-authored message may show as
/// it is: `[a-z0-9][a-z0-9-]{0,31}`, the shape of every project, role and
/// holding instance in use (`pij-fleet`, `pm-001`, `175`, `f114`,
/// `questions-context`). The one grammar for them: the memory turn (#184),
/// role addresses and every other message use it.
pub fn is_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    (1..=32).contains(&bytes.len())
        && bytes[0] != b'-'
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-')
}

/// A directory name ([`is_name`]): a project, role or holding instance.
pub fn name(name: &str) -> Cow<'_, str> {
    closed(name, is_name(name))
}

/// Whether `slug` is a project slug as `pij project create` makes one:
/// lowercase letters and digits in hyphen-joined runs, at most 48.
pub fn is_project(slug: &str) -> bool {
    (1..=48).contains(&slug.len())
        && slug.split('-').all(|run| {
            !run.is_empty()
                && run
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        })
}

/// A project slug ([`is_project`]).
pub fn project(slug: &str) -> Cow<'_, str> {
    closed(slug, is_project(slug))
}

/// A tmux window name pij gives a seat: `<project>:<role>[-<instance>]` or a
/// seat id, each part already rendered, so lowercase letters, digits, `:`,
/// `-`, and the space and parentheses of `invalid (N chars)`; at most 160.
pub fn window(name: &str) -> Cow<'_, str> {
    closed(
        name,
        (1..=160).contains(&name.len())
            && name.bytes().all(|byte| {
                byte.is_ascii_lowercase()
                    || byte.is_ascii_digit()
                    || matches!(byte, b':' | b'-' | b' ' | b'(' | b')')
            }),
    )
}

/// A seat role: the closed vocabulary.
pub fn role(role: &str) -> Cow<'_, str> {
    closed(role, SEAT_ROLES.contains(&role))
}

/// A git head: 7 to 40 lowercase hex digits.
pub fn head(head: &str) -> Cow<'_, str> {
    closed(
        head,
        (7..=40).contains(&head.len())
            && head
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
    )
}

/// A model selector a seat reported: letters, digits and `._:/@+[]-`, at most
/// 128. No whitespace, quote or control can pass, so no sentence or frame.
pub fn model(model: &str) -> Cow<'_, str> {
    closed(
        model,
        (1..=128).contains(&model.len())
            && model.bytes().all(|byte| {
                byte.is_ascii_alphanumeric()
                    || matches!(
                        byte,
                        b'.' | b'_' | b':' | b'/' | b'@' | b'+' | b'[' | b']' | b'-'
                    )
            }),
    )
}

/// A one-word value such as an output result or a quota type: a lowercase
/// letter, then lowercase letters, digits, `_` or `-`, at most 32.
pub fn word(word: &str) -> Cow<'_, str> {
    let bytes = word.as_bytes();
    closed(
        word,
        (1..=32).contains(&bytes.len())
            && bytes[0].is_ascii_lowercase()
            && bytes.iter().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-')
            }),
    )
}

/// An output subject: a lowercase kind, `:`, then letters, digits and
/// `._/@-`, at most 96 (`pr:182`, `issue:F123`, `step:s05`, `role:pa`).
pub fn subject(subject: &str) -> Cow<'_, str> {
    let fits = subject.len() <= 96
        && subject.split_once(':').is_some_and(|(kind, rest)| {
            !kind.is_empty()
                && kind.bytes().all(|byte| byte.is_ascii_lowercase())
                && !rest.is_empty()
                && rest.bytes().all(|byte| {
                    byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'/' | b'@' | b'-')
                })
        });
    closed(subject, fits)
}

/// A machine name a holding recorded: letters, digits, `.`, `_` and `-`, at
/// most 64 (a host name).
pub fn machine(machine: &str) -> Cow<'_, str> {
    closed(
        machine,
        (1..=64).contains(&machine.len())
            && machine
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-')),
    )
}

/// Text an error carries from elsewhere (a reason, a record, an adapter's
/// words): as it is when it is one plain line with no forged frame, else
/// fenced (F142, #193 round 3). An error's text reaches receipts and refusals
/// through `to_string()`, so it is made safe where it is written.
pub fn line(text: &str) -> Cow<'_, str> {
    if text.chars().count() <= FENCE_MAX
        && !text.chars().any(char::is_control)
        && !text.contains("[pij from")
    {
        Cow::Borrowed(text)
    } else {
        Cow::Owned(fenced(text, FENCE_MAX))
    }
}

/// How much of a fenced reason a generated notice shows.
pub const FENCE_MAX: usize = 500;

/// Free text a message exists to carry, fenced: escaped onto one line inside
/// double quotes, cut at `max` characters with `…` when longer. The caller
/// names who wrote it.
pub fn fenced(text: &str, max: usize) -> String {
    let mut out = String::from("\"");
    let mut chars = text.chars();
    for ch in chars.by_ref().take(max) {
        out.extend(ch.escape_debug());
    }
    if chars.next().is_some() {
        out.push('…');
    }
    out.push('"');
    out
}
