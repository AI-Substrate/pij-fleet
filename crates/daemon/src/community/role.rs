//! Build and stamp `pij/role` documents. Pure: no IO, no launches.

use serde_json::{Map, Value, json};

use super::Refusal;

/// Role fields given as `pij directory add` flags. Absent fields are left
/// out, so ddocs names what is missing (for example scope or the packet).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RoleFields {
    /// `meta.title`.
    pub title: Option<String>,
    /// `meta.kind`: specialist or general.
    pub kind: Option<String>,
    /// `meta.lifecycle`: long-lived or per-task.
    pub lifecycle: Option<String>,
    /// `meta.holders`: one or many.
    pub holders: Option<String>,
    /// `meta.instance_key`.
    pub instance_key: Option<String>,
    /// `meta.default_limit` (ruling C5), like `45m`.
    pub default_limit: Option<String>,
    /// `meta.summary`.
    pub summary: Option<String>,
    /// `meta.template`.
    pub template: Option<String>,
    /// `meta.template_version`.
    pub template_version: Option<String>,
    /// `scope.paths`.
    pub scope_paths: Vec<String>,
    /// `scope.rules`.
    pub rules: Vec<String>,
    /// `out_of_scope`.
    pub out_of_scope: Vec<String>,
    /// `forbidden_paths`.
    pub forbidden: Vec<String>,
    /// `default_seat.harness`.
    pub harness: Option<String>,
    /// `default_seat.model`.
    pub model: Option<String>,
    /// `default_seat.effort`.
    pub effort: Option<String>,
    /// `inputs`.
    pub inputs: Vec<String>,
    /// `output_kinds`.
    pub output_kinds: Vec<String>,
    /// `standing_packet` inline text.
    pub packet: Option<String>,
    /// `standing_packet` brief file, linked from the role document.
    pub brief: Option<String>,
}

/// A role document from flags, without a changelog (see [`stamp`]).
///
/// # Errors
/// `E-RS-ARG` when both `--packet` and `--brief` are given.
pub fn from_fields(id: &str, fields: &RoleFields) -> Result<Value, Refusal> {
    let mut meta = Map::new();
    meta.insert("id".into(), json!(id));
    for (key, value) in [
        ("title", &fields.title),
        ("kind", &fields.kind),
        ("lifecycle", &fields.lifecycle),
        ("holders", &fields.holders),
        ("instance_key", &fields.instance_key),
        ("default_limit", &fields.default_limit),
        ("summary", &fields.summary),
        ("template", &fields.template),
        ("template_version", &fields.template_version),
    ] {
        if let Some(value) = value {
            meta.insert(key.into(), json!(value));
        }
    }
    let mut sections = vec![json!({"name": "meta", "value": meta})];
    if !fields.rules.is_empty() || !fields.scope_paths.is_empty() {
        let mut scope = Map::new();
        scope.insert("paths".into(), json!(fields.scope_paths));
        if !fields.rules.is_empty() {
            scope.insert("rules".into(), json!(fields.rules));
        }
        sections.push(json!({"name": "scope", "value": scope}));
    }
    for (name, values) in [
        ("out_of_scope", &fields.out_of_scope),
        ("forbidden_paths", &fields.forbidden),
    ] {
        if !values.is_empty() {
            sections.push(json!({"name": name, "value": values}));
        }
    }
    if let Some(harness) = &fields.harness {
        let mut seat = Map::new();
        seat.insert("harness".into(), json!(harness));
        for (key, value) in [("model", &fields.model), ("effort", &fields.effort)] {
            if let Some(value) = value {
                seat.insert(key.into(), json!(value));
            }
        }
        sections.push(json!({"name": "default_seat", "value": seat}));
    }
    for (name, values) in [
        ("inputs", &fields.inputs),
        ("output_kinds", &fields.output_kinds),
    ] {
        if !values.is_empty() {
            sections.push(json!({"name": name, "value": values}));
        }
    }
    match (&fields.packet, &fields.brief) {
        (Some(_), Some(_)) => {
            return Err(Refusal::new(
                "E-RS-ARG",
                "give the standing packet as --packet <text> or --brief <path>, not both",
            ));
        }
        (Some(text), None) => sections
            .push(json!({"name": "standing_packet", "value": {"source": "inline", "text": text}})),
        (None, Some(brief)) => sections
            .push(json!({"name": "standing_packet", "value": {"source": "brief", "brief": brief}})),
        (None, None) => {}
    }
    Ok(json!({"dd": {"schema": "pij/role"}, "sections": sections, "references": []}))
}

/// The role id a document names in `meta.id`.
pub fn id_of(document: &Value) -> Option<&str> {
    section(document, "meta")?.get("id")?.as_str()
}

/// Add one changelog row (`date`, `by`, `why`) to `document`, checking it is
/// a `pij/role` for `id`. The changelog section is created if absent.
///
/// # Errors
/// `E-RS-ARG` when the document is not a `pij/role`, or names another id.
pub fn stamp(
    mut document: Value,
    id: &str,
    by: &str,
    why: &str,
    date: &str,
) -> Result<Value, Refusal> {
    if document["dd"]["schema"] != "pij/role" {
        return Err(Refusal::new(
            "E-RS-ARG",
            format!("the role document for {id:?} must declare dd.schema \"pij/role\""),
        ));
    }
    match id_of(&document) {
        Some(named) if named == id => {}
        named => {
            return Err(Refusal::new(
                "E-RS-ARG",
                format!("the role document names meta.id {named:?}, not {id:?}"),
            ));
        }
    }
    let row = json!({"date": date, "by": by, "why": why});
    let Some(sections) = document["sections"].as_array_mut() else {
        return Err(Refusal::new(
            "E-RS-ARG",
            "the role document has no sections array",
        ));
    };
    match sections
        .iter_mut()
        .find(|section| section["name"] == "changelog")
    {
        Some(changelog) => match changelog["value"].as_array_mut() {
            Some(rows) => rows.push(row),
            None => changelog["value"] = json!([row]),
        },
        None => sections.push(json!({"name": "changelog", "value": [row]})),
    }
    Ok(document)
}

/// The bytes the writer commits: tab-indented, with a trailing newline, like
/// the repository's Biome-formatted documents.
pub fn to_bytes(document: &Value) -> Vec<u8> {
    let mut bytes = Vec::new();
    let formatter = serde_json::ser::PrettyFormatter::with_indent(b"\t");
    let mut serializer = serde_json::Serializer::with_formatter(&mut bytes, formatter);
    serde::Serialize::serialize(document, &mut serializer).expect("a JSON value serialises");
    bytes.push(b'\n');
    bytes
}

/// Today's date in UTC, `YYYY-MM-DD`, for a changelog row.
pub fn today() -> String {
    let now = time::OffsetDateTime::now_utc();
    format!(
        "{:04}-{:02}-{:02}",
        now.year(),
        u8::from(now.month()),
        now.day()
    )
}

fn section<'a>(document: &'a Value, name: &str) -> Option<&'a Value> {
    document["sections"]
        .as_array()?
        .iter()
        .find(|section| section["name"] == name)
        .map(|section| &section["value"])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fields() -> RoleFields {
        RoleFields {
            title: Some("Security".into()),
            kind: Some("specialist".into()),
            lifecycle: Some("long-lived".into()),
            holders: Some("one".into()),
            default_limit: Some("45m".into()),
            rules: vec!["A verdict on every PR at its exact head.".into()],
            harness: Some("copilot".into()),
            effort: Some("high".into()),
            output_kinds: vec!["verdict".into()],
            packet: Some("You hold the security role.".into()),
            ..RoleFields::default()
        }
    }

    #[test]
    fn flags_become_a_role_document_and_stamping_adds_one_changelog_row() {
        let document = from_fields("security", &fields()).unwrap();
        assert_eq!(id_of(&document), Some("security"));
        assert_eq!(section(&document, "meta").unwrap()["default_limit"], "45m");
        assert_eq!(
            section(&document, "scope").unwrap(),
            &json!({"paths": [], "rules": ["A verdict on every PR at its exact head."]})
        );
        assert_eq!(
            section(&document, "default_seat").unwrap(),
            &json!({"harness": "copilot", "effort": "high"})
        );
        assert!(section(&document, "changelog").is_none());

        let stamped = stamp(
            document,
            "security",
            "pij-brisk-marten",
            "first cut",
            "2026-11-04",
        )
        .unwrap();
        let stamped = stamp(
            stamped,
            "security",
            "pij-far-jackal",
            "second",
            "2026-11-05",
        )
        .unwrap();
        assert_eq!(
            section(&stamped, "changelog").unwrap(),
            &json!([
                {"date": "2026-11-04", "by": "pij-brisk-marten", "why": "first cut"},
                {"date": "2026-11-05", "by": "pij-far-jackal", "why": "second"},
            ])
        );
    }

    #[test]
    fn missing_scope_or_packet_is_left_out_for_ddocs_to_name() {
        let document = from_fields("bare", &RoleFields::default()).unwrap();
        assert!(section(&document, "scope").is_none());
        assert!(section(&document, "standing_packet").is_none());
        let both = RoleFields {
            packet: Some("x".into()),
            brief: Some("brief.md".into()),
            ..RoleFields::default()
        };
        assert_eq!(from_fields("x", &both).unwrap_err().code, "E-RS-ARG");
        let brief = RoleFields {
            brief: Some("../briefs/qa.md".into()),
            ..RoleFields::default()
        };
        assert_eq!(
            section(&from_fields("qa", &brief).unwrap(), "standing_packet").unwrap(),
            &json!({"source": "brief", "brief": "../briefs/qa.md"})
        );
    }

    #[test]
    fn stamping_refuses_another_schema_or_another_id() {
        let document = from_fields("pm", &fields()).unwrap();
        assert_eq!(
            stamp(document.clone(), "qa", "s", "w", "d")
                .unwrap_err()
                .code,
            "E-RS-ARG"
        );
        let mut other = document;
        other["dd"]["schema"] = json!("pij/output");
        assert_eq!(
            stamp(other, "pm", "s", "w", "d").unwrap_err().code,
            "E-RS-ARG"
        );
    }

    #[test]
    fn bytes_are_tab_indented_with_a_trailing_newline() {
        let text = String::from_utf8(to_bytes(&json!({"a": [1]}))).unwrap();
        assert_eq!(text, "{\n\t\"a\": [\n\t\t1\n\t]\n}\n");
    }
}
