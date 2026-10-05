//! Human seat-address parsing for federation: `<seat>` or `<seat>@<machine>`.

use std::fmt;

use crate::model::Destination;

/// A malformed `<seat>` or `<seat>@<machine-alias>` address.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AddressError(String);

impl fmt::Display for AddressError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for AddressError {}

/// Parse a human seat address.
///
/// An unqualified address is local. Within the seat id, `@@` represents a
/// literal `@`; the first unescaped `@` separates the optional machine alias.
/// Machine aliases cannot contain `@`.
pub fn parse_destination(input: &str) -> Result<Destination, AddressError> {
    let mut seat = String::new();
    let mut machine: Option<String> = None;
    let mut chars = input.chars().peekable();

    while let Some(character) = chars.next() {
        if let Some(machine_alias) = &mut machine {
            if character == '@' {
                return Err(AddressError(
                    "machine aliases cannot contain '@' — escape '@' only inside the seat as '@@'"
                        .to_string(),
                ));
            }
            machine_alias.push(character);
            continue;
        }

        if character != '@' {
            seat.push(character);
            continue;
        }

        if chars.peek() == Some(&'@') {
            chars.next();
            seat.push('@');
        } else {
            machine = Some(String::new());
        }
    }

    if seat.is_empty() {
        return Err(AddressError("a seat address needs a seat id".to_string()));
    }
    if machine.as_deref() == Some("") {
        return Err(AddressError(
            "a qualified seat address needs a machine alias after '@'".to_string(),
        ));
    }

    Ok(Destination {
        seat: seat.into(),
        machine,
    })
}

/// Render a destination into the exact grammar accepted by
/// [`parse_destination`].
pub fn render_destination(destination: &Destination) -> String {
    let escaped_seat = destination.seat.as_str().replace('@', "@@");
    match &destination.machine {
        Some(machine) => format!("{escaped_seat}@{machine}"),
        None => escaped_seat,
    }
}

#[cfg(test)]
mod tests {
    use crate::model::Destination;

    use super::{parse_destination, render_destination};

    /// The ONE grammar, shared with the TS harnesses (plan 164 review F01b):
    /// `crates/testkit/fixtures/golden/address/cases.json` is read by this test
    /// and by the OMP/Pi and Copilot parsers' tests, so the sides cannot drift.
    #[test]
    fn the_shared_golden_address_cases_hold() {
        let cases: Vec<serde_json::Value> = serde_json::from_str(include_str!(
            "../../testkit/fixtures/golden/address/cases.json"
        ))
        .expect("golden address cases");
        assert!(!cases.is_empty());
        for case in cases {
            let input = case["input"].as_str().expect("input");
            let parsed = parse_destination(input);
            if case["error"] == true {
                assert!(parsed.is_err(), "{input:?} must be refused");
                continue;
            }
            let parsed = parsed.unwrap_or_else(|error| panic!("{input:?}: {error}"));
            assert_eq!(parsed.seat.as_str(), case["seat"].as_str().expect("seat"), "{input:?}");
            assert_eq!(parsed.machine.as_deref(), case["machine"].as_str(), "{input:?}");
            assert_eq!(parse_destination(&render_destination(&parsed)).ok(), Some(parsed));
        }
    }

    #[test]
    fn an_unqualified_address_always_means_local() {
        let destination = parse_destination("pij-local-seat").expect("parse local seat");

        assert_eq!(destination, Destination::local("pij-local-seat"));
        assert!(destination.is_local());
    }

    #[test]
    fn address_parse_table_covers_degenerate_and_escaped_cases() {
        let cases = [
            ("seat", Some(("seat", None))),
            ("seat@machine", Some(("seat", Some("machine")))),
            ("seat@@id", Some(("seat@id", None))),
            ("seat@@id@machine", Some(("seat@id", Some("machine")))),
            ("seat@@@@id", Some(("seat@@id", None))),
            ("", None),
            ("@machine", None),
            ("seat@", None),
            ("seat@machine@other", None),
            ("seat@machine@@other", None),
        ];

        for (input, expected) in cases {
            let actual = parse_destination(input);
            match expected {
                Some((seat, machine)) => {
                    let actual = actual.unwrap_or_else(|error| panic!("{input}: {error}"));
                    assert_eq!(actual.seat.as_str(), seat, "{input}");
                    assert_eq!(actual.machine.as_deref(), machine, "{input}");
                }
                None => assert!(actual.is_err(), "{input} unexpectedly parsed as {actual:?}"),
            }
        }
    }

    #[test]
    fn every_rendered_address_parses_back_to_the_same_destination() {
        let cases = [
            Destination::local("seat"),
            Destination::local("seat@id"),
            Destination {
                seat: "seat".into(),
                machine: Some("machine".to_string()),
            },
            Destination {
                seat: "seat@id".into(),
                machine: Some("machine".to_string()),
            },
        ];

        for destination in cases {
            let rendered = render_destination(&destination);
            assert_eq!(parse_destination(&rendered), Ok(destination), "{rendered}");
        }
    }
}
