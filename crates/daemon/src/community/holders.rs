//! Holders documents (`pij/holders`, plan 195): who has held a role, in order.
//! Pure: builds and reads documents, no IO.

use pij_store::Assignment;
use serde_json::{Map, Value, json};

use super::Refusal;

/// The seat role an assignment stamps (ruling C1): `pa` for the role `pa`,
/// `pm` for `pm` or any `pm-<n>`, `worker` for every other role.
pub fn seat_role_for(role: &str) -> &'static str {
    match role {
        "pa" => "pa",
        "pm" => "pm",
        _ if role
            .strip_prefix("pm-")
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())) =>
        {
            "pm"
        }
        _ => "worker",
    }
}

/// The checks ddocs cannot express. A closed row carries `to` and
/// `released_by` together, an open row neither (plan 195, from docs-system).
/// No two rows are the same holding (seat, instance and `from`), since that is
/// the store's key.
///
/// # Errors
/// `E-RS-DIRECTORY-ROW` naming the role and seat.
pub fn check_rows(role: &str, rows: &[Assignment]) -> Result<(), Refusal> {
    if let Some(row) = rows
        .iter()
        .find(|row| row.to.is_some() != row.released_by.is_some())
    {
        return Err(Refusal::new(
            E_ROW,
            format!(
                "role {role:?}: the row for {} from {} must carry `to` and `released_by` together",
                row.seat, row.from
            ),
        ));
    }
    for (index, row) in rows.iter().enumerate() {
        if rows[..index].iter().any(|earlier| {
            earlier.seat == row.seat && earlier.instance == row.instance && earlier.from == row.from
        }) {
            return Err(Refusal::new(
                E_ROW,
                format!(
                    "role {role:?}: two rows for {} from {} are the same holding",
                    row.seat, row.from
                ),
            )
            .with_next("Give each row its own `from` on the governance branch, then rebuild"));
        }
    }
    Ok(())
}

/// A holders row the writer will not accept.
const E_ROW: &str = "E-RS-DIRECTORY-ROW";

/// Now, in UTC to the millisecond, as holders documents write it
/// (`2026-11-04T05:00:00.000Z`).
pub fn now() -> String {
    stamp(time::OffsetDateTime::now_utc())
}

fn stamp(at: time::OffsetDateTime) -> String {
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        at.year(),
        u8::from(at.month()),
        at.day(),
        at.hour(),
        at.minute(),
        at.second(),
        at.millisecond()
    )
}

/// The `from` of a new holding of `seat` (and `instance`): now, moved on a
/// millisecond at a time until no row in `rows` has it. `from` is part of a
/// holding's identity, so an assign, release and reassign within one
/// millisecond still makes two holdings. Call it under the writer lock, with
/// the role's committed rows.
pub fn fresh_from(rows: &[Assignment], seat: &str, instance: &str) -> String {
    let mut at = time::OffsetDateTime::now_utc();
    loop {
        let from = stamp(at);
        if !rows
            .iter()
            .any(|row| row.seat == seat && row.instance == instance && row.from == from)
        {
            return from;
        }
        at += time::Duration::milliseconds(1);
    }
}

/// Close the open rows `ends` name (same role, seat, instance and `from`).
/// Returns the rows those ends now describe, as written, for the store: ends
/// applied here and ends the branch had already closed. An end whose holding
/// is not in `rows` at all is left out, so it stays pending.
pub fn apply_ends(rows: &mut [Assignment], ends: &[Assignment]) -> Vec<Assignment> {
    let mut landed = Vec::new();
    for end in ends {
        if let Some(row) = rows.iter_mut().find(|row| {
            row.role == end.role
                && row.seat == end.seat
                && row.instance == end.instance
                && row.from == end.from
        }) {
            if row.to.is_none() {
                row.to.clone_from(&end.to);
                row.released_by.clone_from(&end.released_by);
                row.release_why.clone_from(&end.release_why);
            }
            landed.push(row.clone());
        }
    }
    landed
}

/// The holders document for `role` with `rows` in order.
pub fn to_document(role: &str, rows: &[Assignment]) -> Value {
    let rows: Vec<Value> = rows.iter().map(row_value).collect();
    json!({
        "dd": {"schema": "pij/holders"},
        "sections": [
            {"name": "meta", "value": {"role": role}},
            {"name": "rows", "value": rows},
        ],
        "references": [],
    })
}

fn row_value(row: &Assignment) -> Value {
    let mut value = Map::new();
    let mut put = |key: &str, field: Option<&String>| {
        if let Some(field) = field.filter(|field| !field.is_empty()) {
            value.insert(key.into(), json!(field));
        }
    };
    put("seat", Some(&row.seat));
    put(
        "harness",
        Some(row.harness.as_ref().unwrap_or(&UNKNOWN.to_string())),
    );
    put("harness_session", row.session.as_ref());
    put(
        "machine",
        Some(row.machine.as_ref().unwrap_or(&UNKNOWN.to_string())),
    );
    put("instance_key", Some(&row.instance));
    put("from", Some(&row.from));
    put("to", row.to.as_ref());
    put("assigned_by", Some(&row.assigned_by));
    put("released_by", row.released_by.as_ref());
    put("why", Some(&row.why));
    put("release_why", row.release_why.as_ref());
    Value::Object(value)
}

/// What a holders document says when a harness or machine is not known.
const UNKNOWN: &str = "unknown";

/// The rows a holders document records, as store rows for `project`. A row a
/// pending close-end has not reached the branch yet keeps `end_pending` false:
/// the branch is what it says.
///
/// # Errors
/// `E-RS-GOV-GIT` when the document is not a holders document.
pub fn from_document(project: &str, document: &Value) -> Result<Vec<Assignment>, Refusal> {
    let bad = || {
        Refusal::new(
            super::E_GOV_GIT,
            "a holders document has no meta.role or rows",
        )
    };
    let sections = document["sections"].as_array().ok_or_else(bad)?;
    let section = |name: &str| {
        sections
            .iter()
            .find(|section| section["name"] == name)
            .map(|section| &section["value"])
    };
    let role = section("meta")
        .and_then(|meta| meta["role"].as_str())
        .ok_or_else(bad)?;
    let rows = section("rows").and_then(Value::as_array).ok_or_else(bad)?;
    Ok(rows
        .iter()
        .map(|row| {
            let text = |key: &str| row[key].as_str().map(str::to_string);
            let known = |key: &str| text(key).filter(|value| value != UNKNOWN);
            Assignment {
                project: project.to_string(),
                role: role.to_string(),
                instance: text("instance_key").unwrap_or_default(),
                seat: text("seat").unwrap_or_default(),
                harness: known("harness"),
                session: text("harness_session"),
                machine: known("machine"),
                from: text("from").unwrap_or_default(),
                to: text("to"),
                assigned_by: text("assigned_by").unwrap_or_default(),
                released_by: text("released_by"),
                why: text("why").unwrap_or_default(),
                release_why: text("release_why"),
                end_pending: false,
            }
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(seat: &str, instance: &str, to: Option<&str>) -> Assignment {
        Assignment {
            project: "pij".into(),
            role: "reviewer".into(),
            instance: instance.into(),
            seat: seat.into(),
            harness: Some("copilot".into()),
            session: Some("s-1".into()),
            machine: Some("local".into()),
            from: "2026-11-04T05:00:00Z".into(),
            to: to.map(str::to_string),
            assigned_by: "pij-pm".into(),
            released_by: to.map(|_| "pij-pm".to_string()),
            why: "review #114".into(),
            release_why: to.map(|_| "done".to_string()),
            end_pending: false,
        }
    }

    #[test]
    fn c1_maps_pa_pm_and_everything_else() {
        for (role, seat_role) in [
            ("pa", "pa"),
            ("pm", "pm"),
            ("pm-001", "pm"),
            ("pm-", "worker"),
            ("pm-x", "worker"),
            ("pmo", "worker"),
            ("security", "worker"),
            ("prime", "worker"),
        ] {
            assert_eq!(seat_role_for(role), seat_role, "{role}");
        }
    }

    #[test]
    fn a_document_round_trips_open_closed_and_instance_rows() {
        let rows = vec![
            row("pij-a", "", Some("2026-11-04T06:00:00Z")),
            row("pij-b", "114", None),
        ];
        let document = to_document("reviewer", &rows);
        let written = &document["sections"][1]["value"];
        assert_eq!(written[0]["to"], "2026-11-04T06:00:00Z");
        assert!(
            written[0].get("instance_key").is_none(),
            "an empty instance is left out"
        );
        assert_eq!(written[1]["instance_key"], "114");
        assert!(written[1].get("to").is_none(), "an open row has no to");
        assert_eq!(from_document("pij", &document).unwrap(), rows);
    }

    #[test]
    fn an_unknown_harness_or_machine_is_written_as_unknown_and_read_back_as_none() {
        let mut bare = row("pij-a", "", None);
        bare.harness = None;
        bare.machine = None;
        let document = to_document("reviewer", std::slice::from_ref(&bare));
        assert_eq!(document["sections"][1]["value"][0]["machine"], "unknown");
        assert_eq!(from_document("pij", &document).unwrap(), vec![bare]);
    }

    #[test]
    fn a_row_carries_to_and_released_by_together_or_neither() {
        let open = row("pij-a", "", None);
        let closed = row("pij-b", "", Some("2026-11-04T06:00:00Z"));
        assert!(check_rows("reviewer", &[open.clone(), closed.clone()]).is_ok());
        let mut half = closed;
        half.released_by = None;
        assert_eq!(
            check_rows("reviewer", &[half]).unwrap_err().code,
            "E-RS-DIRECTORY-ROW"
        );
        let mut other = open;
        other.released_by = Some("pij-pm".into());
        assert_eq!(
            check_rows("reviewer", &[other]).unwrap_err().code,
            "E-RS-DIRECTORY-ROW"
        );
    }

    #[test]
    fn two_rows_for_one_holding_are_refused() {
        let first = row("pij-a", "", Some("2026-11-04T06:00:00Z"));
        let mut again = row("pij-a", "", None);
        again.from.clone_from(&first.from);
        let refusal = check_rows("reviewer", &[first.clone(), again.clone()]).unwrap_err();
        assert_eq!(refusal.code, "E-RS-DIRECTORY-ROW");
        assert!(
            refusal.message.contains("same holding"),
            "{}",
            refusal.message
        );
        again.instance = "114".into();
        assert!(check_rows("reviewer", &[first, again]).is_ok());
    }

    #[test]
    fn now_is_utc_to_the_millisecond() {
        let stamp = now();
        assert_eq!(stamp.len(), 24, "{stamp}");
        assert!(
            stamp.ends_with('Z') && stamp.as_bytes()[10] == b'T' && stamp.as_bytes()[19] == b'.',
            "{stamp}"
        );
    }

    #[test]
    fn a_fresh_from_never_repeats_the_same_seats_holding() {
        let mut rows = Vec::new();
        for _ in 0..50 {
            let mut next = row("pij-a", "", None);
            next.from = fresh_from(&rows, "pij-a", "");
            rows.push(next);
        }
        let mut froms: Vec<&str> = rows.iter().map(|row| row.from.as_str()).collect();
        froms.sort_unstable();
        froms.dedup();
        assert_eq!(froms.len(), 50);
    }

    #[test]
    fn ends_report_what_landed_and_leave_missing_holdings_pending() {
        let open = row("pij-a", "", None);
        let closed = row("pij-b", "", Some("2026-11-04T06:00:00Z"));
        let mut rows = vec![open.clone(), closed.clone()];
        let end = |seat: &str, from: &str| {
            let mut end = row(seat, "", Some("2026-11-04T07:00:00Z"));
            end.from = from.into();
            end.release_why = Some("close".into());
            end.end_pending = true;
            end
        };
        let landed = apply_ends(
            &mut rows,
            &[
                end("pij-a", &open.from),
                end("pij-b", &closed.from),
                end("pij-c", &open.from),
            ],
        );
        assert_eq!(rows[0].release_why.as_deref(), Some("close"));
        assert_eq!(
            rows[1], closed,
            "an already closed row keeps the branch's end"
        );
        assert_eq!(
            landed, rows,
            "pij-c's holding is not on the branch, so it stays pending"
        );
        assert!(landed.iter().all(|row| !row.end_pending));
    }
}
