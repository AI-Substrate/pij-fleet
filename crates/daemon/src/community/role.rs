//! Build and stamp `pij/role` documents. Pure: no IO, no launches.

use serde_json::{Map, Value, json};

use std::path::{Component, Path};

use super::{E_BRIEF_MISSING, Refusal};

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
    /// `standing_packet` brief file: a repo-relative link to an existing file.
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

/// `document` with every field `fields` gives replacing its own (plan 194:
/// `add --from template:<name>` then flags). A repeated flag replaces the
/// whole list; a packet or brief replaces the standing packet.
///
/// # Errors
/// `E-RS-ARG` when both `--packet` and `--brief` are given, or the document
/// has no sections array.
pub fn apply_fields(mut document: Value, fields: &RoleFields) -> Result<Value, Refusal> {
    let overrides = from_fields("override", fields)?;
    let Some(sections) = document["sections"].as_array_mut() else {
        return Err(Refusal::new(
            "E-RS-ARG",
            "the role document has no sections array",
        ));
    };
    if let Some(given) = section(&overrides, "meta").and_then(Value::as_object) {
        let meta = slot(sections, "meta", json!({}));
        for (key, value) in given.iter().filter(|(key, _)| *key != "id") {
            meta[key] = value.clone();
        }
    }
    if !fields.rules.is_empty() {
        slot(sections, "scope", json!({}))["rules"] = json!(fields.rules);
    }
    if !fields.scope_paths.is_empty() {
        slot(sections, "scope", json!({}))["paths"] = json!(fields.scope_paths);
    }
    for (name, values) in [
        ("out_of_scope", &fields.out_of_scope),
        ("forbidden_paths", &fields.forbidden),
        ("inputs", &fields.inputs),
        ("output_kinds", &fields.output_kinds),
    ] {
        if !values.is_empty() {
            *slot(sections, name, json!([])) = json!(values);
        }
    }
    for (key, value) in [
        ("harness", &fields.harness),
        ("model", &fields.model),
        ("effort", &fields.effort),
    ] {
        if let Some(value) = value {
            slot(sections, "default_seat", json!({}))[key] = json!(value);
        }
    }
    if let Some(packet) = section(&overrides, "standing_packet") {
        *slot(sections, "standing_packet", json!({})) = packet.clone();
    }
    Ok(document)
}

/// The value of section `name`, added with `empty` when the document lacks it.
fn slot<'a>(sections: &'a mut Vec<Value>, name: &str, empty: Value) -> &'a mut Value {
    let at = match sections.iter().position(|section| section["name"] == name) {
        Some(at) => at,
        None => {
            sections.push(json!({"name": name, "value": empty}));
            sections.len() - 1
        }
    };
    &mut sections[at]["value"]
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
    check_packet(&document, id)?;
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

/// The pairing the schema cannot express (ddocs has no union type): an inline
/// packet carries `text` and no `brief`; a brief packet carries `brief` and no
/// `text`. A missing `standing_packet` is left for ddocs to name.
fn check_packet(document: &Value, id: &str) -> Result<(), Refusal> {
    let Some(packet) = section(document, "standing_packet") else {
        return Ok(());
    };
    let filled = |field: &str| {
        packet[field]
            .as_str()
            .is_some_and(|value| !value.trim().is_empty())
    };
    let (needs, forbids) = match packet["source"].as_str() {
        Some("inline") => ("text", "brief"),
        Some("brief") => ("brief", "text"),
        _ => return Ok(()),
    };
    if !filled(needs) || !packet[forbids].is_null() {
        return Err(Refusal::new(
            "E-RS-ARG",
            format!(
                "role {id:?}: a standing_packet with source {:?} carries `{needs}` and no `{forbids}`",
                packet["source"].as_str().unwrap_or_default()
            ),
        ));
    }
    Ok(())
}

/// A `source: brief` packet must name a readable file inside the governance
/// worktree. ddocs only warns on a missing link target (exit 0), so the writer
/// checks it before writing anything (review of #114, F001). Other documents,
/// and documents ddocs will reject anyway, pass through.
///
/// # Errors
/// [`E_BRIEF_MISSING`] with the role id and the brief path.
pub fn check_brief(bytes: &[u8], worktree: &Path) -> Result<(), Refusal> {
    let Ok(document) = serde_json::from_slice::<Value>(bytes) else {
        return Ok(());
    };
    let Some(packet) = section(&document, "standing_packet") else {
        return Ok(());
    };
    let Some(brief) = packet["brief"]
        .as_str()
        .filter(|_| packet["source"] == "brief")
    else {
        return Ok(());
    };
    let relative = Path::new(brief);
    let inside = !brief.is_empty()
        && relative
            .components()
            .all(|part| matches!(part, Component::Normal(_) | Component::CurDir));
    let root = worktree
        .canonicalize()
        .unwrap_or_else(|_| worktree.to_path_buf());
    let readable = inside
        && worktree
            .join(relative)
            .canonicalize()
            .is_ok_and(|target| target.starts_with(&root) && target.is_file())
        && std::fs::File::open(worktree.join(relative)).is_ok();
    if readable {
        return Ok(());
    }
    Err(Refusal::new(
        E_BRIEF_MISSING,
        format!(
            "role {:?}: the standing packet's brief {brief:?} is not a readable file inside the governance worktree {}",
            id_of(&document).unwrap_or("?"),
            worktree.display()
        ),
    )
    .with_next(
        "Commit the brief to the governance branch first (a repo-relative path), or give the packet inline with --packet.",
    ))
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
    fn a_packet_must_pair_its_source_with_text_or_brief() {
        let with = |packet: Value| {
            let mut document = from_fields("pm", &RoleFields::default()).unwrap();
            document["sections"]
                .as_array_mut()
                .unwrap()
                .push(json!({"name": "standing_packet", "value": packet}));
            stamp(document, "pm", "s", "w", "d")
        };
        assert!(with(json!({"source": "inline", "text": "t"})).is_ok());
        assert!(with(json!({"source": "brief", "brief": "briefs/pm.md"})).is_ok());
        for bad in [
            json!({"source": "inline"}),
            json!({"source": "inline", "text": " "}),
            json!({"source": "inline", "text": "t", "brief": "b.md"}),
            json!({"source": "brief", "text": "t"}),
            json!({"source": "brief", "brief": "b.md", "text": "t"}),
        ] {
            assert_eq!(with(bad.clone()).unwrap_err().code, "E-RS-ARG", "{bad}");
        }
    }

    #[test]
    fn flags_override_a_template_document_field_by_field() {
        let base = json!({"dd": {"schema": "pij/role"}, "sections": [
            {"name": "meta", "value": {"id": "pm", "title": "PM", "holders": "one"}},
            {"name": "scope", "value": {"rules": ["old"], "paths": ["a/**"]}},
            {"name": "default_seat", "value": {"harness": "omp"}},
            {"name": "standing_packet", "value": {"source": "inline", "text": "t"}},
        ], "references": []});
        let fields = RoleFields {
            title: Some("PM for roadmap 001".into()),
            rules: vec!["new".into()],
            model: Some("m".into()),
            output_kinds: vec!["note".into()],
            brief: Some("briefs/pm.md".into()),
            ..RoleFields::default()
        };
        let document = apply_fields(base.clone(), &fields).unwrap();
        let meta = section(&document, "meta").unwrap();
        assert_eq!(meta["id"], "pm", "the id is not a field");
        assert_eq!(meta["title"], "PM for roadmap 001");
        assert_eq!(meta["holders"], "one");
        assert_eq!(
            section(&document, "scope").unwrap(),
            &json!({"rules": ["new"], "paths": ["a/**"]})
        );
        assert_eq!(
            section(&document, "default_seat").unwrap(),
            &json!({"harness": "omp", "model": "m"})
        );
        assert_eq!(
            section(&document, "output_kinds").unwrap(),
            &json!(["note"])
        );
        assert_eq!(
            section(&document, "standing_packet").unwrap(),
            &json!({"source": "brief", "brief": "briefs/pm.md"})
        );
        assert_eq!(
            apply_fields(base.clone(), &RoleFields::default()).unwrap(),
            base
        );
    }

    #[test]
    fn bytes_are_tab_indented_with_a_trailing_newline() {
        let text = String::from_utf8(to_bytes(&json!({"a": [1]}))).unwrap();
        assert_eq!(text, "{\n\t\"a\": [\n\t\t1\n\t]\n}\n");
    }
}
