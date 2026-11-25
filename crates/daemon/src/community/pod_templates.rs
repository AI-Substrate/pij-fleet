//! Shipped pod templates (PRD 010 req-02, blueprint 005 c-pod-templates):
//! every version of every template under `templates/pods/`, compiled in like
//! the role templates, so naming one (`shipped:<name>[@v<n>]`), listing them,
//! adopting one or diffing a copy needs no install and launches nothing.
//! A version is never edited after it ships; a new one is a new row.
//!
//! A repo owns its copies: `adopt` and `community init` copy a version into
//! governance or a path the repo names, recording the version it came from
//! in the copy's `meta.from`; `template-diff` shows what changed upstream
//! since then and applies nothing. Pure: no IO.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::Refusal;
use super::layout::COMMUNITY;
use super::pod::{self, E_TEMPLATE, TEMPLATE_FILE, Template};
use super::templates::{diff, parse_spec};

/// How a pod command names a shipped template: `shipped:<name>[@v<n>]`.
pub const PREFIX: &str = "shipped:";

/// One shipped version: its files, relative to the template's folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Shipped {
    /// The template name, the folder under `templates/pods/`.
    pub name: &'static str,
    /// The version, the `v<n>` folder.
    pub version: u32,
    /// `(path inside the template folder, bytes)`; the first is
    /// [`TEMPLATE_FILE`].
    pub files: &'static [(&'static str, &'static [u8])],
}

impl Shipped {
    /// `shipped:<name>@v<n>`.
    pub fn reference(&self) -> String {
        format!("{PREFIX}{}@v{}", self.name, self.version)
    }

    /// The template file's bytes.
    pub fn template_bytes(&self) -> &'static [u8] {
        self.files
            .iter()
            .find(|(path, _)| *path == TEMPLATE_FILE)
            .map_or(&[][..], |(_, bytes)| *bytes)
    }
}

macro_rules! pod_file {
    ($name:literal, $version:literal, $path:literal) => {
        (
            $path,
            include_bytes!(concat!(
                "../../../../templates/pods/",
                $name,
                "/v",
                $version,
                "/",
                $path
            )) as &'static [u8],
        )
    };
}

/// Every shipped `templates/pods/<name>/v<n>/`, by name then version.
const SHIPPED: &[Shipped] = &[
    Shipped {
        name: "build",
        version: 1,
        files: &[pod_file!("build", "1", "pod-template.dd.json")],
    },
    Shipped {
        name: "live-call",
        version: 1,
        files: &[pod_file!("live-call", "1", "pod-template.dd.json")],
    },
];

/// Every shipped version.
pub fn all() -> &'static [Shipped] {
    SHIPPED
}

/// `name`, at `version` or its latest.
///
/// # Errors
/// [`E_TEMPLATE`] naming the shipped templates, or the versions of `name`.
pub fn get(name: &str, version: Option<u32>) -> Result<&'static Shipped, Refusal> {
    let versions: Vec<&Shipped> = SHIPPED.iter().filter(|s| s.name == name).collect();
    if versions.is_empty() {
        let names: Vec<&str> = names();
        return Err(Refusal::new(
            E_TEMPLATE,
            format!(
                "pij ships no pod template {name:?}; it ships {} (pij pod templates)",
                names.join(", ")
            ),
        ));
    }
    match version {
        None => Ok(versions
            .iter()
            .max_by_key(|s| s.version)
            .copied()
            .unwrap_or(versions[0])),
        Some(wanted) => versions
            .iter()
            .find(|s| s.version == wanted)
            .copied()
            .ok_or_else(|| {
                Refusal::new(
                    E_TEMPLATE,
                    format!(
                        "pod template {name} has no v{wanted}; it has {}",
                        versions
                            .iter()
                            .map(|s| format!("v{}", s.version))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                )
            }),
    }
}

/// Each shipped name once, sorted.
pub fn names() -> Vec<&'static str> {
    let mut names: Vec<&str> = SHIPPED.iter().map(|s| s.name).collect();
    names.sort_unstable();
    names.dedup();
    names
}

/// The shipped template `given` names, when it starts with [`PREFIX`].
///
/// # Errors
/// [`E_TEMPLATE`] for an unknown name or version, `E-RS-ARG` for a bad spec.
pub fn named(given: &str) -> Option<Result<&'static Shipped, Refusal>> {
    let spec = given.strip_prefix(PREFIX)?;
    Some(parse_spec(spec).and_then(|(name, version)| get(&name, version)))
}

/// Read and check a shipped template. Its `source` is its pinned reference,
/// so a pod records exactly which version it read.
///
/// # Errors
/// What [`pod::parse`] refuses.
pub fn read(shipped: &Shipped) -> Result<Template, Refusal> {
    pod::parse(shipped.template_bytes(), &shipped.reference())
}

/// The listing `pij pod templates` prints: each name, its versions and
/// summary, and how to use one.
pub fn listing() -> Value {
    let rows: Vec<Value> = names()
        .into_iter()
        .filter_map(|name| get(name, None).ok())
        .map(|latest| {
            let summary = read(latest)
                .ok()
                .map(|_| {
                    serde_json::from_slice::<Value>(latest.template_bytes())
                        .ok()
                        .and_then(|document| {
                            document["sections"].as_array().and_then(|sections| {
                                sections
                                    .iter()
                                    .find(|section| section["name"] == "meta")
                                    .map(|meta| meta["value"]["summary"].clone())
                            })
                        })
                        .unwrap_or(Value::Null)
                })
                .unwrap_or(Value::Null);
            json!({
                "name": latest.name,
                "latest": latest.version,
                "versions": SHIPPED.iter().filter(|s| s.name == latest.name).map(|s| s.version).collect::<Vec<_>>(),
                "summary": summary,
                "use": format!("pij pod plan {PREFIX}{} --set <name>=<value>", latest.name),
                "adopt": format!("pij pod adopt {} [--into <path>] --why <text>", latest.name),
            })
        })
        .collect();
    json!({"templates": rows})
}

/// The governance folder a template adopted there lives in.
pub fn governance_folder(name: &str) -> PathBuf {
    Path::new(COMMUNITY).join("pod-templates").join(name)
}

/// The files of `shipped` as a copy writes them under `folder`: the template
/// file with `meta.from` naming the version it came from, the rest verbatim.
///
/// # Errors
/// [`E_TEMPLATE`] when the shipped template file is not JSON.
pub fn copy_files(shipped: &Shipped, folder: &Path) -> Result<Vec<(PathBuf, Vec<u8>)>, Refusal> {
    shipped
        .files
        .iter()
        .map(|(path, bytes)| {
            let bytes = if *path == TEMPLATE_FILE {
                let mut document: Value = serde_json::from_slice(bytes).map_err(|error| {
                    Refusal::new(
                        E_TEMPLATE,
                        format!("{}: not JSON: {error}", shipped.reference()),
                    )
                })?;
                if let Some(meta) = document["sections"]
                    .as_array_mut()
                    .and_then(|sections| sections.iter_mut().find(|s| s["name"] == "meta"))
                {
                    meta["value"]["from"] = json!(shipped.reference());
                }
                super::role::to_bytes(&document)
            } else {
                bytes.to_vec()
            };
            Ok((folder.join(path), bytes))
        })
        .collect()
}

/// `meta.from` of a template file: the shipped version it was copied from.
pub fn copied_from(bytes: &[u8]) -> Option<String> {
    let document: Value = serde_json::from_slice(bytes).ok()?;
    document["sections"]
        .as_array()?
        .iter()
        .find(|section| section["name"] == "meta")?["value"]["from"]
        .as_str()
        .map(str::to_string)
}

/// What changed upstream since the copy in `bytes` was taken: the shipped
/// version it names in `meta.from`, the latest, and the unified diff of the
/// template file between them (empty when it is the latest). Applies nothing.
///
/// # Errors
/// [`E_TEMPLATE`] for a copy that names no shipped version, or one pij does
/// not ship.
pub fn upstream_diff(bytes: &[u8], shown: &str) -> Result<Value, Refusal> {
    let from = copied_from(bytes).ok_or_else(|| {
        Refusal::new(
            E_TEMPLATE,
            format!(
                "{shown} names no shipped version in meta.from, so it has no upstream: it is the repo's own template"
            ),
        )
    })?;
    let old = named(&from).unwrap_or_else(|| {
        Err(Refusal::new(
            E_TEMPLATE,
            format!("{shown}: meta.from {from:?} is not shipped:<name>@v<n>"),
        ))
    })?;
    let new = get(old.name, None)?;
    let text = if new.version > old.version {
        diff(
            &String::from_utf8_lossy(old.template_bytes()),
            &String::from_utf8_lossy(new.template_bytes()),
            &old.reference(),
            &new.reference(),
        )
    } else {
        String::new()
    };
    Ok(json!({
        "template": shown,
        "from": old.reference(),
        "latest": new.reference(),
        "current": new.version <= old.version,
        "diff": text,
        "applied": "nothing: copy what you want into your template by hand",
    }))
}

/// A template's slot roles, sorted: from the shipped one it names, or the
/// file at its path (relative to `repo` unless absolute). `None` when it
/// cannot be read. A few file reads.
pub fn template_roles(repo: &Path, record: &Value) -> Option<Vec<String>> {
    let source = pod::meta(record)["template"].as_str()?.to_string();
    let template = match named(&source) {
        Some(shipped) => read(shipped.ok()?).ok()?,
        None => {
            let path = Path::new(&source);
            let path = if path.is_absolute() {
                path.to_path_buf()
            } else {
                repo.join(path)
            };
            pod::read(&path, &source).ok()?
        }
    };
    let mut roles: Vec<String> = template.slots.into_iter().map(|slot| slot.role).collect();
    roles.sort();
    Some(roles)
}

/// What the prime's view suggests saving as a template (PRD 010 req-04): a
/// role made by hand (no `meta.template`) whose type and parameters match an
/// earlier one, and two pods whose members fill the same roles while that set
/// differs from their template's (a group grown while it ran, made a second
/// time). `roles` is `(id, document)`; `pods` is `(pod, record, template
/// slot roles)`. Pure; a suggestion creates nothing.
pub fn save_suggestions(
    roles: &[(String, Value)],
    pods: &[(String, Value, Option<Vec<String>>)],
) -> Vec<Value> {
    let mut found = Vec::new();
    // (type, sorted parameter names) -> the role ids with that shape.
    type Shape = (String, Vec<String>);
    let mut shapes: Vec<(Shape, Vec<String>)> = Vec::new();
    for (id, document) in roles {
        let meta = pod::meta(document);
        if meta["template"]
            .as_str()
            .is_some_and(|name| !name.is_empty())
        {
            continue;
        }
        let mut params: Vec<String> = super::role::parameters(document)
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        if params.is_empty() {
            continue;
        }
        params.sort();
        let key = (meta["type"].as_str().unwrap_or("none").to_string(), params);
        match shapes.iter_mut().find(|(shape, _)| *shape == key) {
            Some((_, ids)) => ids.push(id.clone()),
            None => shapes.push((key, vec![id.clone()])),
        }
    }
    for ((kind, params), ids) in shapes.into_iter().filter(|(_, ids)| ids.len() > 1) {
        found.push(json!({
            "what": "role",
            "roles": ids,
            "why": format!(
                "{} roles of type {kind} with parameters {} were made by hand; save the shape once",
                ids.len(),
                params.join(", ")
            ),
            "command": format!("pij directory export {} --as-template <path> --why <text>", ids[0]),
        }));
    }
    let mut groups: Vec<(Vec<String>, Vec<String>)> = Vec::new();
    for (name, record, slot_roles) in pods {
        let mut members: Vec<String> = pod::records(record, "members")
            .iter()
            .filter_map(|member| member["role"].as_str().map(str::to_string))
            .collect();
        members.sort();
        if members.is_empty() || slot_roles.as_ref() == Some(&members) {
            continue;
        }
        match groups.iter_mut().find(|(shape, _)| *shape == members) {
            Some((_, names)) => names.push(name.clone()),
            None => groups.push((members, vec![name.clone()])),
        }
    }
    for (members, names) in groups.into_iter().filter(|(_, names)| names.len() > 1) {
        found.push(json!({
            "what": "pod",
            "pods": names,
            "why": format!(
                "{} pods grew to the same roles ({}), which their template does not have; save that shape",
                names.len(),
                members.join(", ")
            ),
            "command": format!(
                "pij pod export {} --as-template <path> --why <text>",
                names[names.len() - 1]
            ),
        }));
    }
    found
}

/// The pod template `pij pod export` writes from a pod's record and the
/// template it came from (PRD 010 req-04): every member as a slot (a member
/// added while it ran becomes a slot of its own), the template's
/// parameters, wiring, consulted roles, deliverables and procedures. It
/// carries no seat id, path or parameter value. `template` is `None` when
/// the original cannot be read; the record's consulted roles and
/// deliverables are used then.
pub fn exported(name: &str, pod_name: &str, record: &Value, template: Option<&Value>) -> Value {
    let section_of = |document: &Value, wanted: &str| -> Value {
        document["sections"]
            .as_array()
            .and_then(|sections| sections.iter().find(|section| section["name"] == wanted))
            .map_or(Value::Null, |section| section["value"].clone())
    };
    let original_slots: Vec<Value> = template
        .map(|template| section_of(template, "slots"))
        .and_then(|slots| slots.as_array().cloned())
        .unwrap_or_default();
    let mut slots: Vec<Value> = Vec::new();
    for member in pod::records(record, "members") {
        let slot = original_slots
            .iter()
            .find(|slot| slot["id"] == member["slot"])
            .cloned()
            .unwrap_or_else(|| {
                // A seat added while it ran: its slot is named for its role,
                // never for the seat.
                let role = member["role"].as_str().unwrap_or("member");
                let base = match member["instance"].as_str().filter(|i| !i.is_empty()) {
                    Some(instance) => format!("{role}-{instance}"),
                    None => role.to_string(),
                };
                let taken = |id: &str| {
                    original_slots.iter().any(|slot| slot["id"] == id)
                        || slots.iter().any(|slot| slot["id"] == id)
                };
                let mut id = base.clone();
                let mut n = 2;
                while taken(&id) {
                    id = format!("{base}-{n}");
                    n += 1;
                }
                id.truncate(32);
                let mut slot = json!({"id": id, "role": member["role"]});
                if let Some(instance) = member["instance"].as_str().filter(|i| !i.is_empty()) {
                    slot["instance"] = json!(instance);
                }
                if let Some(keep) = member["keep_open"].as_str() {
                    slot["keep_open"] = json!(keep);
                }
                slot
            });
        slots.push(slot);
    }
    let parameters = template.map_or_else(
        || {
            Value::Array(
                pod::records(record, "parameters")
                    .iter()
                    .map(|row| json!({"name": row["name"], "need": "optional"}))
                    .collect(),
            )
        },
        |template| section_of(template, "parameters"),
    );
    let pick = |wanted: &str| {
        template.map_or_else(|| section_of(record, wanted), |t| section_of(t, wanted))
    };
    let title = template
        .map(|template| section_of(template, "meta")["title"].clone())
        .and_then(|title| title.as_str().map(str::to_string))
        .unwrap_or_else(|| name.to_string());
    // Wiring and stage needs name slots: keep only the slots exported, so a
    // slot that had no member leaves no dangling name.
    let kept: Vec<Value> = slots.iter().map(|slot| slot["id"].clone()).collect();
    // A slot that must not share a dropped slot's model has nothing left to
    // differ from (B347-R1-03).
    for slot in &mut slots {
        if slot
            .get("not_model_of")
            .is_some_and(|other| !kept.contains(other))
            && let Some(fields) = slot.as_object_mut()
        {
            fields.remove("not_model_of");
        }
    }
    let mut sections = vec![
        json!({"name": "meta", "value": {
            "name": name,
            "title": title,
            "version": "1",
            "summary": format!("Exported from pod {pod_name}, with the seats it ran with."),
        }}),
        json!({"name": "parameters", "value": if parameters.is_null() { json!([]) } else { parameters }}),
        json!({"name": "slots", "value": slots}),
    ];
    let fit = |wanted: &str, mut value: Value| -> Value {
        if let Some(rows) = value.as_array_mut() {
            match wanted {
                "wiring" => {
                    rows.retain(|row| kept.contains(&row["from"]) && kept.contains(&row["to"]));
                }
                "deliverables" => {
                    for row in rows.iter_mut() {
                        if let Some(needs) = row["stage_needs"].as_array_mut() {
                            for need in needs.iter_mut() {
                                if let Some(named) = need["slots"].as_array_mut() {
                                    named.retain(|slot| kept.contains(slot));
                                }
                            }
                            needs.retain(|need| {
                                need["slots"]
                                    .as_array()
                                    .is_some_and(|named| !named.is_empty())
                            });
                        }
                    }
                }
                _ => {}
            }
        }
        value
    };
    for wanted in ["wiring", "consulted", "deliverables", "procedures"] {
        let value = fit(wanted, pick(wanted));
        if value.as_array().is_some_and(|rows| !rows.is_empty()) {
            sections.push(json!({"name": wanted, "value": value}));
        }
    }
    json!({"dd": {"schema": "pij/pod-template"}, "sections": sections, "references": []})
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Plan 254 ac-0001: a version is never edited after it ships. A new
    /// version adds a row here; an edited one fails.
    #[test]
    fn a_shipped_pod_template_is_never_edited() {
        use sha2::{Digest, Sha256};
        const PINNED: [(&str, u32, &str); 2] = [
            (
                "build",
                1,
                "dd9d32d8a07ee3bd37e4045b48fc4a2f24ba1ae4489849a59e28c4b174be19f5",
            ),
            (
                "live-call",
                1,
                "5aa309c4f49306d3334b5569b91675717dfa4cb9b65c6f05c35d6e66e92caba8",
            ),
        ];
        assert_eq!(all().len(), PINNED.len(), "pin every new version here");
        for (name, version, pinned) in PINNED {
            let shipped = get(name, Some(version)).expect("a pinned version ships");
            let hex: String = Sha256::digest(shipped.template_bytes())
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect();
            assert_eq!(
                hex, pinned,
                "{name}@v{version} was edited after it shipped; add a new version"
            );
            read(shipped).expect("every shipped version parses");
        }
    }

    /// Plan 254 ac-0002, ac-0003: a copy records where it came from, and the
    /// diff of a copy of the latest is empty; an unknown copy has no upstream.
    #[test]
    fn a_copy_names_its_version_and_diffs_against_upstream() {
        let shipped = named("shipped:build").unwrap().unwrap();
        assert_eq!(shipped.reference(), "shipped:build@v1");
        let files = copy_files(shipped, Path::new("pods/build")).unwrap();
        assert_eq!(files[0].0, Path::new("pods/build/pod-template.dd.json"));
        assert_eq!(
            copied_from(&files[0].1).as_deref(),
            Some("shipped:build@v1")
        );
        let diffed = upstream_diff(&files[0].1, "pods/build").unwrap();
        assert_eq!(diffed["current"], true);
        assert_eq!(diffed["diff"], "");
        let own = upstream_diff(shipped.template_bytes(), "mine").unwrap_err();
        assert!(own.message.contains("the repo's own template"), "{own}");
        assert!(named("shipped:nope").unwrap().is_err());
        assert!(named("pods/build").is_none());
    }

    /// Plan 255 ac-0001: an export carries every member as a slot, the
    /// added one included, and no seat id or parameter value.
    #[test]
    fn an_export_has_a_slot_per_member_and_no_seat() {
        let template: Value =
            serde_json::from_slice(get("build", None).unwrap().template_bytes()).unwrap();
        let record = json!({"sections": [
            {"name": "meta", "value": {"template": "shipped:build@v1"}},
            {"name": "parameters", "value": [{"name": "subject", "value": "step 2"}]},
            {"name": "members", "value": [
                {"slot": "coder", "role": "coder", "seat": "pij-a"},
                {"slot": "pij-b", "role": "security", "seat": "pij-b", "keep_open": "yes"},
            ]},
        ]});
        let document = exported("grown", "b1", &record, Some(&template));
        let text = document.to_string();
        assert!(
            !text.contains("pij-a") && !text.contains("pij-b") && !text.contains("step 2"),
            "{text}"
        );
        let parsed = pod::parse(&serde_json::to_vec(&document).unwrap(), "grown").unwrap();
        let ids: Vec<&str> = parsed.slots.iter().map(|slot| slot.id.as_str()).collect();
        assert_eq!(ids, ["coder", "security"]);
        assert_eq!(parsed.slots[1].keep_open, Some(true));
        assert!(
            !text.contains("\"reviewer\"") && !text.contains("\"project-manager\""),
            "no wiring or stage need names a slot it dropped: {text}"
        );
    }

    /// B347-R1-03: the build pod with its coder released exports its project
    /// manager and reviewer as a template that parses: the reviewer's
    /// not_model_of names a slot that is gone, so it goes too.
    #[test]
    fn a_pod_reshaped_without_the_author_still_exports() {
        let template: Value =
            serde_json::from_slice(get("build", None).unwrap().template_bytes()).unwrap();
        let record = json!({"sections": [
            {"name": "meta", "value": {"template": "shipped:build@v1"}},
            {"name": "members", "value": [
                {"slot": "project-manager", "role": "pm", "seat": "pij-a"},
                {"slot": "reviewer", "role": "reviewer", "seat": "pij-b"},
            ]},
        ]});
        let document = exported("reshaped", "b1", &record, Some(&template));
        let parsed = pod::parse(&serde_json::to_vec(&document).unwrap(), "reshaped").unwrap();
        let ids: Vec<&str> = parsed.slots.iter().map(|slot| slot.id.as_str()).collect();
        assert_eq!(ids, ["project-manager", "reviewer"]);
        assert_eq!(parsed.slots[1].not_model_of, None);
    }

    /// Plan 255 ac-0003: a role shape made twice by hand, and two pods grown
    /// to the same roles, are suggested for saving; a role from a template
    /// and a pod that matches its template are not.
    #[test]
    fn a_shape_made_twice_is_suggested_for_saving() {
        let role = |template: Option<&str>| {
            json!({"sections": [
                {"name": "meta", "value": {"type": "knowledge", "template": template}},
                {"name": "parameters", "value": [{"name": "subject", "need": "required"}]},
            ]})
        };
        let pod_of = |roles: &[&str]| json!({"sections": [{"name": "members", "value": roles.iter().map(|role| json!({"role": role})).collect::<Vec<_>>()}]});
        let roles = vec![
            ("expert-a".to_string(), role(None)),
            ("expert-b".to_string(), role(None)),
            ("expert-c".to_string(), role(Some("topic-expert"))),
        ];
        let slots = Some(vec!["coder".to_string(), "pm".to_string()]);
        let pods = vec![
            (
                "one".to_string(),
                pod_of(&["pm", "coder", "security"]),
                slots.clone(),
            ),
            (
                "two".to_string(),
                pod_of(&["security", "coder", "pm"]),
                slots.clone(),
            ),
            ("three".to_string(), pod_of(&["pm", "coder"]), slots),
        ];
        let found = save_suggestions(&roles, &pods);
        assert_eq!(found.len(), 2, "{found:?}");
        assert_eq!(found[0]["roles"], json!(["expert-a", "expert-b"]));
        assert_eq!(found[1]["pods"], json!(["one", "two"]));
        assert!(
            found[1]["command"]
                .as_str()
                .unwrap()
                .starts_with("pij pod export two")
        );
    }
}
