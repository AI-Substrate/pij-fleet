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

/// Now, in UTC, as holders documents write it (`2026-11-04T05:00:00Z`).
pub fn now() -> String {
    let now = time::OffsetDateTime::now_utc();
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        now.year(),
        u8::from(now.month()),
        now.day(),
        now.hour(),
        now.minute(),
        now.second()
    )
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
    fn now_is_utc_to_the_second() {
        let stamp = now();
        assert_eq!(stamp.len(), 20, "{stamp}");
        assert!(
            stamp.ends_with('Z') && stamp.as_bytes()[10] == b'T',
            "{stamp}"
        );
    }
}
