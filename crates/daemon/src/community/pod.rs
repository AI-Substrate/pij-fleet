//! Pod templates and pods (PRD 008 req-01, req-02, req-07, req-12; blueprint
//! 005 c-pod-schema). A pod template is a folder holding
//! `pod-template.dd.json` (`pij/pod-template`), read from whatever path a pod
//! command names: pij does not enforce where it lives (decision d-13). A pod
//! is the running group made from one; its record is
//! `community/pods/<pod>/pod.dd.json` (`pij/pod`) on the governance branch,
//! written only by pij. Seat facts (folder, sessions, process) stay in the
//! database and are never copied in.
//!
//! Pure: reading a template is one file read; nothing here launches.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::Refusal;
use super::layout::{COMMUNITY, RolePaths};

/// A template's file inside its folder.
pub const TEMPLATE_FILE: &str = "pod-template.dd.json";
/// A pod's record inside its folder.
pub const POD_FILE: &str = "pod.dd.json";
/// A template that cannot be read or does not hold together.
pub const E_TEMPLATE: &str = "E-RS-POD-TEMPLATE";
/// A parameter a template does not declare, or a required one left out.
pub const E_POD_PARAM: &str = "E-RS-POD-PARAM";

/// One slot: a seat the pod starts in a role.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Slot {
    /// The slot's id within the template.
    pub id: String,
    /// The role its seat starts in.
    pub role: String,
    /// The holding's instance key; empty for none.
    pub instance: String,
    /// The template parameters this slot uses.
    pub parameters: Vec<String>,
    /// The tools its seat must confirm when the pod starts (req-19).
    pub tools: Vec<String>,
    /// A slot whose model this one must not share (req-21).
    pub not_model_of: Option<String>,
    /// Left running when the pod goes down (req-10).
    pub keep_open: bool,
    /// Text this slot's seat is told at start, after its role.
    pub seed: Option<String>,
}

/// A role the pod consults and never starts (req-02, req-18).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Consulted {
    /// The role.
    pub role: String,
    /// Needed live before `up` (a warning, never a refusal).
    pub before_up: bool,
}

/// A deliverable (req-16, req-17, req-20).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Deliverable {
    /// Its id within the template.
    pub id: String,
    /// The role that owns it.
    pub owner: String,
    /// Its stages, in order; one unnamed stage when the template names none.
    pub stages: Vec<String>,
    /// The role its stages are written back to.
    pub write_back: Option<String>,
}

/// A read, checked pod template.
#[derive(Clone, Debug, PartialEq)]
pub struct Template {
    /// Where it was read from, as the command named it.
    pub source: String,
    /// `meta.name`.
    pub name: String,
    /// `meta.version`.
    pub version: String,
    /// SHA-256 of the file's bytes, so a pod records exactly what it read.
    pub hash: String,
    /// Declared parameters, `(name, required)`, in order.
    pub parameters: Vec<(String, bool)>,
    /// Its slots, in order.
    pub slots: Vec<Slot>,
    /// Roles it consults.
    pub consulted: Vec<Consulted>,
    /// Its deliverables.
    pub deliverables: Vec<Deliverable>,
}

fn section<'a>(document: &'a Value, name: &str) -> Option<&'a Value> {
    document["sections"]
        .as_array()?
        .iter()
        .find(|section| section["name"] == name)
        .map(|section| &section["value"])
}

fn rows<'a>(document: &'a Value, name: &str) -> &'a [Value] {
    section(document, name)
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

fn text(value: &Value) -> String {
    value.as_str().unwrap_or_default().trim().to_string()
}

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .map(|items| {
            items
                .iter()
                .map(text)
                .filter(|item| !item.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

fn refuse(source: &str, why: impl std::fmt::Display) -> Refusal {
    Refusal::new(E_TEMPLATE, format!("pod template {source}: {why}"))
}

fn role_id(source: &str, what: &str, id: &str) -> Result<String, Refusal> {
    RolePaths::new(id)
        .map_err(|_| refuse(source, format!("{what} names role {id:?}, not a role id")))?;
    Ok(id.to_string())
}

/// Read the template at `path`: a folder holding [`TEMPLATE_FILE`], or that
/// file itself. `shown` is how the command named it.
///
/// # Errors
/// [`E_TEMPLATE`] for a missing or unreadable file, or one [`parse`] refuses.
pub fn read(path: &Path, shown: &str) -> Result<Template, Refusal> {
    let file = if path.is_dir() {
        path.join(TEMPLATE_FILE)
    } else {
        path.to_path_buf()
    };
    let bytes = std::fs::read(&file)
        .map_err(|error| refuse(shown, format!("cannot read {}: {error}", file.display())))?;
    parse(&bytes, shown)
}

/// Parse and check a template's bytes: its schema, unique slot ids in the name
/// grammar, role ids, every parameter a slot uses declared, `not_model_of`
/// naming another slot, unique deliverable ids each with an owner.
///
/// # Errors
/// [`E_TEMPLATE`] naming what is wrong.
pub fn parse(bytes: &[u8], source: &str) -> Result<Template, Refusal> {
    let document: Value = serde_json::from_slice(bytes)
        .map_err(|error| refuse(source, format!("not JSON: {error}")))?;
    if document["dd"]["schema"] != "pij/pod-template" {
        return Err(refuse(
            source,
            "it must declare dd.schema \"pij/pod-template\"",
        ));
    }
    let meta = section(&document, "meta").cloned().unwrap_or(Value::Null);
    let name = text(&meta["name"]);
    if !pij_core::render::is_name(&name) {
        return Err(refuse(
            source,
            "meta.name must be 1-32 lowercase letters, digits or '-'",
        ));
    }
    let parameters: Vec<(String, bool)> = rows(&document, "parameters")
        .iter()
        .map(|row| (text(&row["name"]), row["need"] != "optional"))
        .collect();
    let mut slots: Vec<Slot> = Vec::new();
    for row in rows(&document, "slots") {
        let id = text(&row["id"]);
        if !pij_core::render::is_name(&id) {
            return Err(refuse(
                source,
                format!("slot id {id:?} must be 1-32 lowercase letters, digits or '-'"),
            ));
        }
        if slots.iter().any(|slot| slot.id == id) {
            return Err(refuse(source, format!("slot {id} is named twice")));
        }
        let uses = strings(&row["parameters"]);
        if let Some(unknown) = uses
            .iter()
            .find(|used| !parameters.iter().any(|(name, _)| name == *used))
        {
            return Err(refuse(
                source,
                format!(
                    "slot {id} uses parameter {unknown:?}, which the template does not declare"
                ),
            ));
        }
        slots.push(Slot {
            role: role_id(source, &format!("slot {id}"), &text(&row["role"]))?,
            instance: text(&row["instance"]),
            parameters: uses,
            tools: strings(&row["tools"]),
            not_model_of: Some(text(&row["not_model_of"])).filter(|other| !other.is_empty()),
            keep_open: row["keep_open"] == "yes",
            seed: Some(text(&row["seed"])).filter(|seed| !seed.is_empty()),
            id,
        });
    }
    if slots.is_empty() {
        return Err(refuse(source, "it has no slots"));
    }
    for slot in &slots {
        if let Some(other) = &slot.not_model_of
            && (other == &slot.id || !slots.iter().any(|known| &known.id == other))
        {
            return Err(refuse(
                source,
                format!(
                    "slot {} must not share the model of {other:?}, which is not another slot",
                    slot.id
                ),
            ));
        }
    }
    let consulted = rows(&document, "consulted")
        .iter()
        .map(|row| {
            Ok(Consulted {
                role: role_id(source, "a consulted role", &text(&row["role"]))?,
                before_up: row["before_up"] == "yes",
            })
        })
        .collect::<Result<Vec<_>, Refusal>>()?;
    let mut deliverables: Vec<Deliverable> = Vec::new();
    for row in rows(&document, "deliverables") {
        let id = text(&row["id"]);
        if !pij_core::render::is_name(&id) || deliverables.iter().any(|known| known.id == id) {
            return Err(refuse(
                source,
                format!("deliverable id {id:?} is not a unique name"),
            ));
        }
        let write_back = Some(text(&row["write_back"])).filter(|role| !role.is_empty());
        deliverables.push(Deliverable {
            owner: role_id(source, &format!("deliverable {id}"), &text(&row["owner"]))?,
            stages: Some(strings(&row["stages"]))
                .filter(|stages| !stages.is_empty())
                .unwrap_or_else(|| vec!["done".to_string()]),
            write_back: write_back
                .map(|role| role_id(source, &format!("deliverable {id}'s write-back"), &role))
                .transpose()?,
            id,
        });
    }
    Ok(Template {
        source: source.to_string(),
        name,
        version: text(&meta["version"]),
        hash: format!("{:x}", Sha256::digest(bytes)),
        parameters,
        slots,
        consulted,
        deliverables,
    })
}

/// `--set <name>=<value>` checked against the template's parameters: the
/// filled values in declaration order.
///
/// # Errors
/// [`E_POD_PARAM`] for a name the template does not declare (listing those it
/// does), every required one left out, or a value that is not one line.
pub fn fill(
    template: &Template,
    sets: &[(String, String)],
) -> Result<Vec<(String, String)>, Refusal> {
    for (index, (name, value)) in sets.iter().enumerate() {
        if sets[..index].iter().any(|(earlier, _)| earlier == name) {
            return Err(Refusal::new(
                E_POD_PARAM,
                format!("--set {name} is given twice"),
            ));
        }
        if value.trim().is_empty() || value.chars().any(char::is_control) {
            return Err(Refusal::new(
                E_POD_PARAM,
                format!("--set {name}: the value must be one non-empty line"),
            ));
        }
        if !template.parameters.iter().any(|(known, _)| known == name) {
            let names: Vec<&str> = template
                .parameters
                .iter()
                .map(|(known, _)| known.as_str())
                .collect();
            return Err(Refusal::new(
                E_POD_PARAM,
                format!(
                    "pod template {} declares no parameter {:?}; it declares {}",
                    template.name,
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
    let missing: Vec<String> = template
        .parameters
        .iter()
        .filter(|(name, required)| *required && !sets.iter().any(|(given, _)| given == name))
        .map(|(name, _)| format!("--set {name}=<value>"))
        .collect();
    if !missing.is_empty() {
        return Err(Refusal::new(
            E_POD_PARAM,
            format!(
                "pod template {} needs {}",
                template.name,
                missing.join(", ")
            ),
        ));
    }
    Ok(template
        .parameters
        .iter()
        .filter_map(|(name, _)| sets.iter().find(|(given, _)| given == name).cloned())
        .collect())
}

/// Where a pod's record lives on the governance branch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PodPaths {
    /// `community/pods/<pod>/`.
    pub dir: PathBuf,
    /// `community/pods/<pod>/pod.dd.json`.
    pub document: PathBuf,
}

impl PodPaths {
    /// The paths for pod `pod`, after checking its name.
    ///
    /// # Errors
    /// `E-RS-ARG` for a name outside the name grammar.
    pub fn new(pod: &str) -> Result<Self, Refusal> {
        if !pij_core::render::is_name(pod) {
            return Err(Refusal::new(
                "E-RS-ARG",
                "a pod's name is 1-32 lowercase letters, digits or '-', not starting with '-'",
            ));
        }
        let dir = Path::new(COMMUNITY).join("pods").join(pod);
        Ok(Self {
            document: dir.join(POD_FILE),
            dir,
        })
    }

    /// `community/pods/<pod>/first-turns/<seat>.md`.
    pub fn first_turn(&self, seat: &str) -> PathBuf {
        self.dir.join("first-turns").join(format!("{seat}.md"))
    }
}

/// What a new pod records about itself.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NewPod {
    /// The pod's name.
    pub pod: String,
    /// Its subject (a parameter named `subject`), when the template has one.
    pub subject: Option<String>,
    /// The folder its work is in (a parameter named `work-folder`), for the
    /// freeze record at down (req-14).
    pub work_folder: Option<String>,
    /// Started with `--fresh` (req-12).
    pub fresh: bool,
    /// The seat that ran `up`.
    pub by: String,
    /// When, as holders documents write it.
    pub at: String,
}

/// A new pod's `pij/pod` document, state `up`, with no members yet.
pub fn document(template: &Template, filled: &[(String, String)], pod: &NewPod) -> Value {
    let mut meta = json!({
        "pod": pod.pod,
        "template": template.source,
        "template_version": template.version,
        "template_hash": template.hash,
        "state": "up",
        "created_by": pod.by,
        "created_at": pod.at,
    });
    if let Some(subject) = &pod.subject {
        meta["subject"] = json!(subject);
    }
    if let Some(folder) = &pod.work_folder {
        meta["work_folder"] = json!(folder);
    }
    if pod.fresh {
        meta["fresh"] = json!("yes");
    }
    json!({
        "dd": {"schema": "pij/pod"},
        "sections": [
            {"name": "meta", "value": meta},
            {"name": "parameters", "value": filled.iter().map(|(name, value)| json!({"name": name, "value": value})).collect::<Vec<_>>()},
            {"name": "members", "value": []},
            {"name": "log", "value": [{"at": pod.at, "by": pod.by, "change": format!("up from {} (version {}, sha256 {})", template.source, template.version, &template.hash[..12])}]},
        ],
        "references": [],
    })
}

/// `document` with `change` by `by` at `at` appended to its log.
pub fn logged(mut document: Value, at: &str, by: &str, change: &str) -> Value {
    if let Some(sections) = document["sections"].as_array_mut()
        && let Some(log) = sections.iter_mut().find(|section| section["name"] == "log")
        && let Some(rows) = log["value"].as_array_mut()
    {
        rows.push(json!({"at": at, "by": by, "change": change}));
    }
    document
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn template_bytes(extra_slot_param: &str) -> Vec<u8> {
        serde_json::to_vec(&json!({
            "dd": {"schema": "pij/pod-template"},
            "sections": [
                {"name": "meta", "value": {"name": "build", "title": "Build pod", "version": "1"}},
                {"name": "parameters", "value": [
                    {"name": "subject", "need": "required"},
                    {"name": "step", "need": "required"},
                    {"name": "branch", "need": "optional"},
                ]},
                {"name": "slots", "value": [
                    {"id": "pm", "role": "pm-001", "parameters": ["step"]},
                    {"id": "coder", "role": "coder", "instance": "build", "parameters": ["step", extra_slot_param], "tools": ["git"]},
                    {"id": "reviewer", "role": "reviewer", "not_model_of": "coder", "keep_open": "no"},
                ]},
                {"name": "consulted", "value": [{"role": "security", "before_up": "yes"}]},
                {"name": "deliverables", "value": [{"id": "pr", "owner": "pm-001", "stages": ["draft", "final"], "write_back": "docs-keeper"}]},
            ],
            "references": [],
        }))
        .unwrap()
    }

    #[test]
    fn a_template_reads_its_slots_consulted_roles_and_deliverables() {
        let template = parse(&template_bytes("branch"), "pods/build").unwrap();
        assert_eq!(template.name, "build");
        assert_eq!(template.slots.len(), 3);
        assert_eq!(template.slots[1].tools, ["git"]);
        assert_eq!(template.slots[2].not_model_of.as_deref(), Some("coder"));
        assert!(template.consulted[0].before_up);
        assert_eq!(
            template.deliverables[0].write_back.as_deref(),
            Some("docs-keeper")
        );
        assert_eq!(template.hash.len(), 64);
    }

    #[test]
    fn a_slot_using_an_undeclared_parameter_is_refused_by_name() {
        let refused = parse(&template_bytes("ticket"), "pods/build").unwrap_err();
        assert_eq!(refused.code, E_TEMPLATE);
        assert!(
            refused
                .message
                .contains("slot coder uses parameter \"ticket\""),
            "{refused}"
        );
    }

    #[test]
    fn filling_refuses_an_unknown_name_and_names_every_missing_one() {
        let template = parse(&template_bytes("branch"), "pods/build").unwrap();
        let missing = fill(&template, &[]).unwrap_err();
        assert!(
            missing
                .message
                .contains("--set subject=<value>, --set step=<value>"),
            "{missing}"
        );
        let unknown = fill(&template, &[("ticket".into(), "1".into())]).unwrap_err();
        assert!(
            unknown
                .message
                .contains("it declares subject, step, branch"),
            "{unknown}"
        );
        let filled = fill(
            &template,
            &[
                ("step".into(), "s01".into()),
                ("subject".into(), "acme".into()),
            ],
        )
        .unwrap();
        assert_eq!(filled[0].0, "subject", "declaration order");
    }

    #[test]
    fn a_pod_document_records_its_template_and_never_a_seat_path() {
        let template = parse(&template_bytes("branch"), "pods/build").unwrap();
        let pod = NewPod {
            pod: "build-s01".into(),
            subject: Some("acme".into()),
            by: "pij-a".into(),
            at: "2026-11-25T00:00:00.000Z".into(),
            ..NewPod::default()
        };
        let document = document(&template, &[("subject".into(), "acme".into())], &pod);
        let meta = section(&document, "meta").unwrap();
        assert_eq!(meta["template"], "pods/build");
        assert_eq!(meta["template_hash"], template.hash);
        assert_eq!(meta["state"], "up");
        let logged = logged(document, "t", "pij-a", "member added");
        assert_eq!(rows(&logged, "log").len(), 2);
        assert_eq!(
            PodPaths::new("build-s01").unwrap().document,
            Path::new(COMMUNITY).join("pods/build-s01/pod.dd.json")
        );
        assert!(PodPaths::new("../x").is_err());
    }
}
