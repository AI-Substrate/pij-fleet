//! Pure event-subscription filtering.
//!
//! Runtime delivery belongs in `pij-daemon`; this module only decides whether an
//! already-decoded [`Event`] belongs to a subscription. Kinds remain strings on
//! purpose: additive kinds must pass an unfiltered subscription without a core
//! release teaching an enum a new variant first.

use std::collections::BTreeSet;

use crate::model::{Event, SeatId};

/// A subscription's optional kind and seat constraints.
///
/// Empty kind sets match nothing. An absent kind set matches every kind,
/// including kinds this build has never heard of.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EventFilter {
    kinds: Option<BTreeSet<String>>,
    seat: Option<SeatId>,
}

impl EventFilter {
    /// Match every event, including additive unknown kinds.
    pub fn all() -> Self {
        Self::default()
    }

    /// Match exactly the supplied kind strings.
    pub fn kinds(kinds: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            kinds: Some(kinds.into_iter().map(Into::into).collect()),
            seat: None,
        }
    }

    /// Additionally restrict this filter to one seat.
    pub fn for_seat(mut self, seat: impl Into<SeatId>) -> Self {
        self.seat = Some(seat.into());
        self
    }

    /// Whether `event` belongs to this subscription.
    pub fn matches(&self, event: &Event) -> bool {
        self.kinds
            .as_ref()
            .is_none_or(|kinds| kinds.contains(&event.kind))
            && self
                .seat
                .as_ref()
                .is_none_or(|seat| event.seat.as_ref() == Some(seat))
    }
}

#[cfg(test)]
mod tests {
    use super::EventFilter;
    use crate::model::{Event, SeatId};

    fn event(kind: &str, seat: Option<&str>) -> Event {
        Event {
            seq: None,
            v: 1,
            at: 1,
            kind: kind.to_string(),
            seat: seat.map(SeatId::from),
            payload: "{}".to_string(),
        }
    }

    #[test]
    fn all_forwards_additive_kinds_but_explicit_filters_stay_exact() {
        let unknown = event("future.kind", Some("pij-a"));
        assert!(EventFilter::all().matches(&unknown));
        assert!(EventFilter::kinds(["future.kind"]).matches(&unknown));
        assert!(!EventFilter::kinds(["message"]).matches(&unknown));
        assert!(!EventFilter::kinds(Vec::<String>::new()).matches(&unknown));
    }

    #[test]
    fn kind_and_seat_constraints_are_both_required() {
        let filter = EventFilter::kinds(["message"]).for_seat("pij-a");
        assert!(filter.matches(&event("message", Some("pij-a"))));
        assert!(!filter.matches(&event("receipt", Some("pij-a"))));
        assert!(!filter.matches(&event("message", Some("pij-b"))));
        assert!(!filter.matches(&event("message", None)));
    }
}
