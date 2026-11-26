//! Build and stamp `pij/role` documents. Pure: no IO, no launches.

use serde::Serialize;
use serde_json::{Map, Value, json};

use std::path::{Component, Path};

use super::{E_BRIEF_MISSING, Refusal};

/// Role fields given as `pij directory add` flags. Absent fields are left
/// out, so ddocs names what is missing (for example scope or the packet).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RoleFields {
    /// `meta.title`.
    pub title: Option<String>,
    /// `meta.type`: one of pij's ten seat types (PRD 006). `kind` and
    /// `lifecycle` are retired: read from old documents, never written.
    pub role_type: Option<String>,
    /// `meta.keeps`: the artefact a knowledge role keeps current.
    pub keeps: Option<String>,
    /// `meta.sole_writer_of`: what a steward role is the only writer of.
    pub sole_writer_of: Option<String>,
    /// `meta.type_suggestion`: the type the creator wished existed.
    pub type_suggestion: Option<String>,
    /// `--type-change <rule>=<why>`: `type_changes`, merged by rule.
    pub type_changes: Vec<(String, String)>,
    /// `--param <name>[:optional]`: `parameters`, `(name, required)` in order
    /// (PRD 007 req-01). Replaces the whole list when given.
    pub parameters: Vec<(String, bool)>,
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
    /// `edit --add-<list>` and `--remove-<list>` (F152): one item each,
    /// applied after any replacing flag, removes before adds.
    pub list_ops: Vec<ListOp>,
}

/// A role's list-valued fields: the flag that names each, and where it lives
/// in the document (F152).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RoleList {
    /// `--scope-path`: `scope.paths`.
    ScopePaths,
    /// `--rule`: `scope.rules`.
    Rules,
    /// `--out-of-scope`: `out_of_scope`.
    OutOfScope,
    /// `--forbidden`: `forbidden_paths`.
    Forbidden,
    /// `--input`: `inputs`.
    Inputs,
    /// `--output-kind`: `output_kinds`.
    OutputKinds,
}

impl RoleList {
    /// Every list, in the order an edit applies and reports them.
    pub const ALL: [Self; 6] = [
        Self::ScopePaths,
        Self::Rules,
        Self::OutOfScope,
        Self::Forbidden,
        Self::Inputs,
        Self::OutputKinds,
    ];

    /// The flag that replaces it (`--add-<flag>` and `--remove-<flag>` edit it).
    pub const fn flag(self) -> &'static str {
        match self {
            Self::ScopePaths => "scope-path",
            Self::Rules => "rule",
            Self::OutOfScope => "out-of-scope",
            Self::Forbidden => "forbidden",
            Self::Inputs => "input",
            Self::OutputKinds => "output-kind",
        }
    }

    /// `(section, key within it)`; `None` for a section that is the list.
    const fn location(self) -> (&'static str, Option<&'static str>) {
        match self {
            Self::ScopePaths => ("scope", Some("paths")),
            Self::Rules => ("scope", Some("rules")),
            Self::OutOfScope => ("out_of_scope", None),
            Self::Forbidden => ("forbidden_paths", None),
            Self::Inputs => ("inputs", None),
            Self::OutputKinds => ("output_kinds", None),
        }
    }

    /// The list's items in `document`, empty when absent.
    pub fn items(self, document: &Value) -> Vec<String> {
        let (name, key) = self.location();
        let value = section(document, name);
        let list = match key {
            Some(key) => value.map(|value| &value[key]),
            None => value,
        };
        list.and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    }

    fn set(self, sections: &mut Vec<Value>, items: &[String]) {
        let (name, key) = self.location();
        match key {
            Some(key) => slot(sections, name, json!({}))[key] = json!(items),
            None => *slot(sections, name, json!([])) = json!(items),
        }
    }
}

/// One `--add-<list>` or `--remove-<list>` item (F152).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListOp {
    /// Which list.
    pub list: RoleList,
    /// Add (`true`) or remove.
    pub add: bool,
    /// The item, matched exactly.
    pub item: String,
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
        ("type", &fields.role_type),
        ("keeps", &fields.keeps),
        ("sole_writer_of", &fields.sole_writer_of),
        ("type_suggestion", &fields.type_suggestion),
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
    if !fields.type_changes.is_empty() {
        sections.push(
            json!({"name": "type_changes", "value": type_changes(&[], &fields.type_changes)}),
        );
    }
    if !fields.parameters.is_empty() {
        sections
            .push(json!({"name": "parameters", "value": parameters_value(&fields.parameters)?}));
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
    if !fields.parameters.is_empty() {
        *slot(sections, "parameters", json!([])) = parameters_value(&fields.parameters)?;
    }
    if !fields.type_changes.is_empty() {
        let slot = slot(sections, "type_changes", json!([]));
        let current = slot.as_array().cloned().unwrap_or_default();
        *slot = type_changes(&current, &fields.type_changes);
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
    apply_list_ops(&mut document, &fields.list_ops)?;
    Ok(document)
}

/// A role's declared parameters, `(name, required)` in order (PRD 007 req-01).
pub fn parameters(document: &Value) -> Vec<(String, bool)> {
    section(document, "parameters")
        .and_then(Value::as_array)
        .map(|rows| {
            rows.iter()
                .filter_map(|row| {
                    Some((
                        row["name"].as_str()?.to_string(),
                        row["need"].as_str() != Some("optional"),
                    ))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// `--param` values as the `parameters` section: each name takes the one name
/// grammar (`[a-z0-9][a-z0-9-]{0,31}`) and appears once.
fn parameters_value(given: &[(String, bool)]) -> Result<Value, Refusal> {
    let mut rows = Vec::new();
    for (index, (name, required)) in given.iter().enumerate() {
        if !pij_core::render::is_name(name) {
            return Err(Refusal::new(
                "E-RS-ARG",
                format!(
                    "--param {:?}: a parameter name is 1-32 lowercase letters, digits or '-', not starting with '-'",
                    name.escape_debug().take(32).collect::<String>()
                ),
            ));
        }
        if given[..index].iter().any(|(earlier, _)| earlier == name) {
            return Err(Refusal::new(
                "E-RS-ARG",
                format!("--param {name} is given twice"),
            ));
        }
        rows.push(json!({"name": name, "need": if *required { "required" } else { "optional" }}));
    }
    Ok(json!(rows))
}

/// A parameter a role does not declare was given (PRD 007 req-01).
pub const E_PARAM_UNKNOWN: &str = "E-RS-PARAM-UNKNOWN";
/// A required parameter was not given (PRD 007 req-01).
pub const E_PARAM_MISSING: &str = "E-RS-PARAM-MISSING";

/// `sets` (`--set <name>=<value>`) checked against role `id`'s declared
/// parameters: the filled values in declaration order.
///
/// # Errors
/// `E-RS-ARG` for a value that is not one line or a name given twice;
/// [`E_PARAM_UNKNOWN`] listing the declared names; [`E_PARAM_MISSING`] naming
/// every required one left out.
pub fn fill_parameters(
    id: &str,
    document: &Value,
    sets: &[(String, String)],
) -> Result<Vec<(String, String)>, Refusal> {
    let declared = parameters(document);
    for (index, (name, value)) in sets.iter().enumerate() {
        if sets[..index].iter().any(|(earlier, _)| earlier == name) {
            return Err(Refusal::new(
                "E-RS-ARG",
                format!("--set {name} is given twice"),
            ));
        }
        if value.contains(['\n', '\r']) || value.trim().is_empty() {
            return Err(Refusal::new(
                "E-RS-ARG",
                format!("--set {name}: the value must be one non-empty line"),
            ));
        }
        if !declared.iter().any(|(known, _)| known == name) {
            let names: Vec<&str> = declared.iter().map(|(known, _)| known.as_str()).collect();
            return Err(Refusal::new(
                E_PARAM_UNKNOWN,
                format!(
                    "role {id:?} declares no parameter {:?}; it declares {}",
                    name.escape_debug().take(32).collect::<String>(),
                    if names.is_empty() {
                        "none".to_string()
                    } else {
                        names.join(", ")
                    }
                ),
            ));
        }
    }
    let missing: Vec<&str> = declared
        .iter()
        .filter(|(name, required)| *required && !sets.iter().any(|(given, _)| given == name))
        .map(|(name, _)| name.as_str())
        .collect();
    if !missing.is_empty() {
        return Err(Refusal::new(
            E_PARAM_MISSING,
            format!(
                "role {id:?} needs {}: give each with --set <name>=<value>",
                missing
                    .iter()
                    .map(|name| format!("--set {name}=<value>"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        ));
    }
    Ok(declared
        .iter()
        .filter_map(|(name, _)| sets.iter().find(|(given, _)| given == name).cloned())
        .collect())
}

/// `current` type changes with each `(rule, why)` replacing the row for its
/// rule or appended after them.
fn type_changes(current: &[Value], given: &[(String, String)]) -> Value {
    let mut rows = current.to_vec();
    for (rule, why) in given {
        let row = json!({"rule": rule, "why": why});
        match rows.iter_mut().find(|row| row["rule"] == rule.as_str()) {
            Some(existing) => *existing = row,
            None => rows.push(row),
        }
    }
    json!(rows)
}

/// Apply `--add-<list>` and `--remove-<list>` (F152), in a stated order:
/// after any replacing flag, per list, every remove then every add, each in
/// the order given. A remove of an item the list does not hold, or an add of
/// one it already holds, refuses: nothing is written.
///
/// # Errors
/// `E-RS-ARG` naming the list and the item.
pub fn apply_list_ops(document: &mut Value, ops: &[ListOp]) -> Result<(), Refusal> {
    for list in RoleList::ALL {
        let mine: Vec<&ListOp> = ops.iter().filter(|op| op.list == list).collect();
        if mine.is_empty() {
            continue;
        }
        let mut items = list.items(document);
        for op in mine.iter().filter(|op| !op.add) {
            let Some(at) = items.iter().position(|item| *item == op.item) else {
                return Err(Refusal::new(
                    "E-RS-ARG",
                    format!(
                        "--remove-{} {}: not in the list ({})",
                        list.flag(),
                        pij_core::render::caller_text(&op.item),
                        shown_list(&items)
                    ),
                ));
            };
            items.remove(at);
        }
        for op in mine.iter().filter(|op| op.add) {
            if items.contains(&op.item) {
                return Err(Refusal::new(
                    "E-RS-ARG",
                    format!(
                        "--add-{} {}: already in the list",
                        list.flag(),
                        pij_core::render::caller_text(&op.item)
                    ),
                ));
            }
            items.push(op.item.clone());
        }
        let Some(sections) = document["sections"].as_array_mut() else {
            return Err(Refusal::new(
                "E-RS-ARG",
                "the role document has no sections array",
            ));
        };
        list.set(sections, &items);
    }
    Ok(())
}

/// A list's items in a refusal or receipt: each fenced, comma-joined.
pub fn shown_list(items: &[String]) -> String {
    if items.is_empty() {
        return "empty".to_string();
    }
    items
        .iter()
        .map(|item| pij_core::render::caller_text(item))
        .collect::<Vec<_>>()
        .join(", ")
}

/// What an edit did to each list (F152): `{list, before, after, removed,
/// added}` for every list whose items changed, in [`RoleList::ALL`] order.
pub fn list_changes(before: &Value, after: &Value) -> Vec<Value> {
    RoleList::ALL
        .iter()
        .filter_map(|list| {
            let (old, new) = (list.items(before), list.items(after));
            (old != new).then(|| {
                json!({
                    "list": list.flag(),
                    "before": old,
                    "after": new,
                    "removed": old.iter().filter(|item| !new.contains(item)).collect::<Vec<_>>(),
                    "added": new.iter().filter(|item| !old.contains(item)).collect::<Vec<_>>(),
                })
            })
        })
        .collect()
}

/// `meta.state` for a role that is no longer in use (plan 206, req-28).
pub const RETIRED: &str = "retired";

/// Where a role stands (plan 206): active unless retired, and the role it was
/// merged into, if any. What the view shows and the send path refuses on.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Standing {
    /// `active` or `retired`; a role that records no state is active.
    pub state: String,
    /// The role a merged role now lives under.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub merged_into: Option<String>,
}

impl Standing {
    /// Whether the role is retired (a merged role is retired too).
    pub fn retired(&self) -> bool {
        self.state == RETIRED
    }
}

/// The standing `document`'s meta records.
pub fn standing(document: &Value) -> Standing {
    let meta = section(document, "meta");
    let text = |field: &str| {
        meta.and_then(|meta| meta[field].as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    };
    Standing {
        state: text("state").unwrap_or_else(|| "active".to_string()),
        merged_into: text("merged_into"),
    }
}

/// `document` marked retired, and merged into `into` when given.
///
/// # Errors
/// `E-RS-ARG` when the document has no sections array.
pub fn retire(mut document: Value, into: Option<&str>) -> Result<Value, Refusal> {
    let Some(sections) = document["sections"].as_array_mut() else {
        return Err(Refusal::new(
            "E-RS-ARG",
            "the role document has no sections array",
        ));
    };
    let meta = slot(sections, "meta", json!({}));
    meta["state"] = json!(RETIRED);
    if let Some(into) = into {
        meta["merged_into"] = json!(into);
    }
    Ok(document)
}

/// `document` with `committed`'s `meta.state` and `meta.merged_into` where it
/// leaves them out, so a whole-document edit never silently un-retires a role
/// (docs-system verdict on #135, note 3).
pub fn carry_standing(mut document: Value, committed: &Value) -> Value {
    let Some(from) = section(committed, "meta").cloned() else {
        return document;
    };
    if let Some(sections) = document["sections"].as_array_mut() {
        let meta = slot(sections, "meta", json!({}));
        for field in ["state", "merged_into"] {
            if meta.get(field).is_none()
                && let Some(value) = from.get(field)
            {
                meta[field] = value.clone();
            }
        }
    }
    document
}

/// `document` carrying `committed`'s changelog in place of its own, so a
/// whole-document edit never rewrites history (plan 206, F20).
pub fn keep_changelog(mut document: Value, committed: &Value) -> Value {
    let history = section(committed, "changelog").cloned();
    if let Some(sections) = document["sections"].as_array_mut() {
        sections.retain(|section| section["name"] != "changelog");
        if let Some(history) = history {
            sections.push(json!({"name": "changelog", "value": history}));
        }
    }
    document
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
    let standing = standing(&document);
    if standing.merged_into.is_some() && !standing.retired() {
        return Err(Refusal::new(
            "E-RS-ARG",
            format!(
                "role {id:?} records meta.merged_into without meta.state retired; a merge writes both (pij directory merge)"
            ),
        ));
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

/// A standing packet that is given but invalid (an empty inline text, or
/// both text and brief), as a problem for the add's one refusal (review
/// B363-R1-01): the same check [`stamp`] runs, with the flag that fixes it.
pub fn packet_problem(document: &Value, id: &str) -> Option<super::types::Problem> {
    check_packet(document, id)
        .err()
        .map(|refusal| super::types::Problem {
            flag: "--packet <text> or --brief <path>".to_string(),
            refusal,
        })
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

/// The fields the pij/role schema requires that `pij directory add` fills
/// from a flag: (section, meta field or `None` for the section's own key
/// field, the flag). id and changelog are filled by pij. Kept equal to the
/// schema by a test.
pub const SCHEMA_FIELDS: [(&str, &str, &str); 4] = [
    ("meta", "title", "--title <text>"),
    ("meta", "holders", "--holders one|many"),
    ("scope", "rules", "--rule <text>"),
    (
        "standing_packet",
        "source",
        "--packet <text> or --brief <path>",
    ),
];

/// The flags for every [`SCHEMA_FIELDS`] entry `document` lacks: the field
/// absent, or its section absent. Presence only, as the schema checks it;
/// ddocs stays the authority when the role is written.
pub fn schema_gaps(document: &Value) -> Vec<String> {
    SCHEMA_FIELDS
        .iter()
        .filter(|(name, field, _)| {
            section(document, name).is_none_or(|value| value.get(*field).is_none())
        })
        .map(|(_, _, flag)| (*flag).to_string())
        .collect()
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

    /// OSK-DIRECTORY-ADD-SERIAL: the fields `schema_gaps` lists are exactly
    /// the ones the pij/role schema requires, bar those pij fills itself.
    #[test]
    fn schema_fields_are_the_role_schemas_required_fields() {
        let schema: Value = serde_json::from_slice(include_bytes!(
            "../../../../.dd/schemas/pij/role/schema.json"
        ))
        .unwrap();
        let mut required = Vec::new();
        for (name, spec) in schema["sections"].as_object().unwrap() {
            let fields = spec["shape"]["required"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            if name == "meta" {
                for field in fields {
                    let field = field.as_str().unwrap();
                    if field != "id" {
                        required.push((name.clone(), field.to_string()));
                    }
                }
            } else if spec["required"] == true && name != "changelog" {
                for field in fields {
                    required.push((name.clone(), field.as_str().unwrap().to_string()));
                }
            }
        }
        required.sort();
        let mut table: Vec<(String, String)> = SCHEMA_FIELDS
            .iter()
            .map(|(name, field, _)| ((*name).to_string(), (*field).to_string()))
            .collect();
        table.sort();
        assert_eq!(table, required);
    }

    #[test]
    fn standing_reads_active_retired_and_merged_and_retire_sets_them() {
        let active = json!({"sections": [{"name": "meta", "value": {"id": "qa"}}]});
        assert_eq!(
            standing(&active),
            Standing {
                state: "active".into(),
                merged_into: None
            }
        );
        assert!(!standing(&active).retired());
        let retired = retire(active.clone(), None).unwrap();
        assert!(standing(&retired).retired());
        assert_eq!(standing(&retired).merged_into, None);
        let merged = retire(active, Some("security")).unwrap();
        assert_eq!(
            standing(&merged),
            Standing {
                state: "retired".into(),
                merged_into: Some("security".into())
            }
        );
        assert!(standing(&json!({})).state == "active", "no meta is active");
        let committed = retire(
            json!({"sections": [{"name": "meta", "value": {"id": "qa"}}]}),
            Some("sec"),
        )
        .unwrap();
        let carried = carry_standing(
            json!({"sections": [{"name": "meta", "value": {"id": "qa", "title": "Q"}}]}),
            &committed,
        );
        assert_eq!(standing(&carried), standing(&committed));
        let explicit = carry_standing(
            json!({"sections": [{"name": "meta", "value": {"id": "qa", "state": "active"}}]}),
            &committed,
        );
        assert_eq!(standing(&explicit).state, "active", "a given state is kept");
        assert!(retire(json!({}), None).is_err());
    }

    fn fields() -> RoleFields {
        RoleFields {
            title: Some("Security".into()),
            role_type: Some("reviewer".into()),
            type_changes: vec![("compact-after-job".into(), "carries open findings".into())],
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
        assert_eq!(section(&document, "meta").unwrap()["type"], "reviewer");
        assert!(section(&document, "meta").unwrap().get("kind").is_none());
        assert_eq!(
            section(&document, "type_changes").unwrap(),
            &json!([{"rule": "compact-after-job", "why": "carries open findings"}])
        );
        let changed = apply_fields(
            document.clone(),
            &RoleFields {
                type_changes: vec![
                    ("compact-after-job".into(), "standing context".into()),
                    ("independent".into(), "x".into()),
                ],
                ..RoleFields::default()
            },
        )
        .unwrap();
        assert_eq!(
            section(&changed, "type_changes").unwrap(),
            &json!([
                {"rule": "compact-after-job", "why": "standing context"},
                {"rule": "independent", "why": "x"}
            ]),
            "a change to the same rule replaces its row"
        );
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

    #[test]
    fn a_role_declares_parameters_and_filling_them_names_what_is_wrong() {
        let document = from_fields(
            "notes",
            &RoleFields {
                parameters: vec![("subject".into(), true), ("session".into(), false)],
                ..RoleFields::default()
            },
        )
        .unwrap();
        assert_eq!(
            section(&document, "parameters").unwrap(),
            &json!([{"name": "subject", "need": "required"}, {"name": "session", "need": "optional"}])
        );
        let set = |pairs: &[(&str, &str)]| -> Vec<(String, String)> {
            pairs
                .iter()
                .map(|(name, value)| ((*name).into(), (*value).into()))
                .collect()
        };
        assert_eq!(
            fill_parameters(
                "notes",
                &document,
                &set(&[("session", "s1"), ("subject", "acme")])
            )
            .unwrap(),
            set(&[("subject", "acme"), ("session", "s1")]),
            "declaration order"
        );
        let missing = fill_parameters("notes", &document, &set(&[("session", "s1")])).unwrap_err();
        assert_eq!(missing.code, E_PARAM_MISSING);
        assert!(
            missing.message.contains("--set subject=<value>"),
            "{}",
            missing.message
        );
        let unknown = fill_parameters(
            "notes",
            &document,
            &set(&[("subject", "a"), ("colour", "b")]),
        )
        .unwrap_err();
        assert_eq!(unknown.code, E_PARAM_UNKNOWN);
        assert!(
            unknown.message.contains("subject, session"),
            "{}",
            unknown.message
        );
        assert!(fill_parameters("notes", &document, &set(&[("subject", "a\nb")])).is_err());
        for bad in [
            vec![("Bad".to_string(), true)],
            vec![("a".into(), true), ("a".into(), false)],
        ] {
            assert!(
                from_fields(
                    "x",
                    &RoleFields {
                        parameters: bad,
                        ..RoleFields::default()
                    }
                )
                .is_err()
            );
        }
    }
}
