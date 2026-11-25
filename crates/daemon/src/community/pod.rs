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
    /// Left running when the pod goes down (req-10): `Some(true)` marked
    /// keep-open, `Some(false)` marked not, `None` the role type's default
    /// (a knowledge seat stays open).
    pub keep_open: Option<bool>,
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
    /// The slots each stage needs, where the template says; a stage it does
    /// not name needs every slot (req-16).
    pub needs: Vec<(String, Vec<String>)>,
}

/// Where a procedure a template names lives (PRD 010 req-01).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProcedureFrom {
    /// In the template's own folder, `procedures/<area>/PROC-<area>-NNNN-<slug>/`,
    /// so it travels with the template.
    Template,
    /// Linked: in the repo's procedures (or shipped with the pij skill).
    Repo,
}

/// A procedure a template names (PRD 010 req-01).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcedureRef {
    /// `PROC-<area>-NNNN`.
    pub id: String,
    /// Where it lives.
    pub from: ProcedureFrom,
    /// What the pod uses it for: `up`, `run` or `close`; empty for none.
    /// `pij pod down` runs the `close` one first, through the pij CLI.
    pub used_for: String,
}

/// The procedure use `pij pod down` runs before the freeze.
pub const CLOSE_USE: &str = "close";
/// The uses a template's procedure may name.
pub const PROCEDURE_USES: [&str; 3] = ["up", "run", CLOSE_USE];
/// Procedure areas the pij skill ships, present wherever pij is installed.
pub const SKILL_PROCEDURE_AREAS: [&str; 2] = ["meta", "docs"];

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
    /// How its slots feed each other (req-01, req-02): `(from, to, what)`.
    pub wiring: Vec<(String, String, String)>,
    /// The procedures it names, its own and linked (PRD 010 req-01).
    pub procedures: Vec<ProcedureRef>,
    /// The folder it was read from; empty when parsed from bytes alone.
    pub folder: PathBuf,
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
    let mut template = parse(&bytes, shown)?;
    template.folder = file.parent().map(Path::to_path_buf).unwrap_or_default();
    Ok(template)
}

/// `PROC-<area>-NNNN` split into its area, or `None` for another shape.
fn procedure_area(id: &str) -> Option<&str> {
    let rest = id.strip_prefix("PROC-")?;
    let (area, number) = rest.split_once('-')?;
    let area_ok = area.starts_with(|c: char| c.is_ascii_lowercase())
        && area
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit());
    (area_ok && number.len() == 4 && number.chars().all(|c| c.is_ascii_digit())).then_some(area)
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
            keep_open: match row["keep_open"].as_str() {
                Some("yes") => Some(true),
                Some("no") => Some(false),
                _ => None,
            },
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
        let stages = Some(strings(&row["stages"]))
            .filter(|stages| !stages.is_empty())
            .unwrap_or_else(|| vec!["done".to_string()]);
        let mut needs = Vec::new();
        for need in row["stage_needs"].as_array().map_or(&[][..], Vec::as_slice) {
            let stage = text(&need["stage"]);
            let slots_needed = strings(&need["slots"]);
            if !stages.contains(&stage) {
                return Err(refuse(
                    source,
                    format!(
                        "deliverable {id} names needs for stage {stage:?}, which it does not have"
                    ),
                ));
            }
            if let Some(unknown) = slots_needed
                .iter()
                .find(|slot| !slots.iter().any(|known| &known.id == *slot))
            {
                return Err(refuse(
                    source,
                    format!(
                        "deliverable {id}'s stage {stage} needs slot {unknown:?}, which is not a slot"
                    ),
                ));
            }
            needs.push((stage, slots_needed));
        }
        deliverables.push(Deliverable {
            owner: role_id(source, &format!("deliverable {id}"), &text(&row["owner"]))?,
            stages,
            needs,
            write_back: write_back
                .map(|role| role_id(source, &format!("deliverable {id}'s write-back"), &role))
                .transpose()?,
            id,
        });
    }
    let wiring = rows(&document, "wiring")
        .iter()
        .map(|row| (text(&row["from"]), text(&row["to"]), text(&row["what"])))
        .filter(|(from, to, _)| !from.is_empty() && !to.is_empty())
        .collect();
    let mut procedures: Vec<ProcedureRef> = Vec::new();
    for row in rows(&document, "procedures") {
        let id = text(&row["id"]);
        if procedure_area(&id).is_none() {
            return Err(refuse(
                source,
                format!("procedure {id:?} is not a procedure id (PROC-<area>-NNNN)"),
            ));
        }
        if procedures.iter().any(|known| known.id == id) {
            return Err(refuse(source, format!("procedure {id} is named twice")));
        }
        let from = match row["from"].as_str() {
            Some("template") => ProcedureFrom::Template,
            Some("repo") => ProcedureFrom::Repo,
            _ => {
                return Err(refuse(
                    source,
                    format!("procedure {id}: from must be template or repo"),
                ));
            }
        };
        let used_for = text(&row["use"]);
        if !used_for.is_empty() && !PROCEDURE_USES.contains(&used_for.as_str()) {
            return Err(refuse(
                source,
                format!(
                    "procedure {id}: use must be one of {}",
                    PROCEDURE_USES.join(", ")
                ),
            ));
        }
        if used_for == CLOSE_USE && procedures.iter().any(|known| known.used_for == CLOSE_USE) {
            return Err(refuse(source, "it names more than one close procedure"));
        }
        procedures.push(ProcedureRef { id, from, used_for });
    }
    Ok(Template {
        wiring,
        source: source.to_string(),
        name,
        version: text(&meta["version"]),
        hash: format!("{:x}", Sha256::digest(bytes)),
        parameters,
        slots,
        consulted,
        deliverables,
        procedures,
        folder: PathBuf::new(),
    })
}

impl Template {
    /// The procedure `pij pod down` runs before the freeze, if it names one.
    pub fn close_procedure(&self) -> Option<&ProcedureRef> {
        self.procedures
            .iter()
            .find(|procedure| procedure.used_for == CLOSE_USE)
    }
}

/// Whether `procedure` is where `template` says, read from the files: the
/// template's own under `<folder>/procedures/<area>/`, a linked one in the
/// repo's procedures home (`.pij-procedures.json`'s `home`, else
/// `docs/project/procedures`) or the pij skill's (its areas are shipped with
/// pij, so present wherever it is installed). A few directory reads.
pub fn procedure_present(template: &Template, repo: &Path, procedure: &ProcedureRef) -> bool {
    let Some(area) = procedure_area(&procedure.id) else {
        return false;
    };
    let in_home = |home: &Path| {
        let prefix = format!("{}-", procedure.id);
        std::fs::read_dir(home.join(area)).is_ok_and(|entries| {
            entries.flatten().any(|entry| {
                entry.file_name().to_string_lossy().starts_with(&prefix)
                    && entry.path().join("procedure.dd.json").is_file()
            })
        })
    };
    match procedure.from {
        ProcedureFrom::Template => in_home(&template.folder.join("procedures")),
        ProcedureFrom::Repo => {
            SKILL_PROCEDURE_AREAS.contains(&area)
                || in_home(&repo.join(procedures_home(repo)))
                || in_home(&repo.join("skills/pij/procedures"))
        }
    }
}

/// A repo's procedures home, relative to it: `.pij-procedures.json`'s `home`
/// when it is a plain relative path, else `docs/project/procedures`.
fn procedures_home(repo: &Path) -> String {
    std::fs::read(repo.join(".pij-procedures.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|config| config["home"].as_str().map(str::to_string))
        .filter(|home| !home.starts_with('/') && !home.split('/').any(|part| part == ".."))
        .map_or_else(
            || "docs/project/procedures".to_string(),
            |home| home.trim_end_matches('/').to_string(),
        )
}

/// A warning for each procedure the template names that is not where it
/// says (PRD 010 req-01): `plan` prints them, `up` logs them. Warnings only.
pub fn procedure_checks(
    template: &Template,
    present: &dyn Fn(&ProcedureRef) -> bool,
) -> Vec<Warning> {
    template
        .procedures
        .iter()
        .filter(|procedure| !present(procedure))
        .map(|procedure| match procedure.from {
            ProcedureFrom::Template => Warning {
                about: format!("procedure {}", procedure.id),
                text: format!(
                    "the template names it as its own, and {} has no procedures/<area>/{}-<slug>/",
                    template.source, procedure.id
                ),
                fix: format!(
                    "write {}/procedures/<area>/{}-<slug>/procedure.dd.json (PRD 005's layout), or change its from to repo",
                    template.folder.display(),
                    procedure.id
                ),
            },
            ProcedureFrom::Repo => Warning {
                about: format!("procedure {}", procedure.id),
                text: "the template links it from the repo, and this repo does not have it".to_string(),
                fix: format!(
                    "copy {} into this repo's procedures, or write one: pij proc new <area> <slug> --title <t>",
                    procedure.id
                ),
            },
        })
        .collect()
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
            {"name": "consulted", "value": template.consulted.iter().map(|role| json!({"role": role.role, "before_up": if role.before_up { "yes" } else { "no" }})).collect::<Vec<_>>()},
            {"name": "deliverables", "value": template.deliverables.iter().map(|deliverable| {
                let mut row = json!({"id": deliverable.id, "owner": deliverable.owner, "stages": deliverable.stages});
                if let Some(role) = &deliverable.write_back {
                    row["write_back"] = json!(role);
                }
                if !deliverable.needs.is_empty() {
                    row["stage_needs"] = json!(deliverable.needs.iter().map(|(stage, slots)| json!({"stage": stage, "slots": slots})).collect::<Vec<_>>());
                }
                row
            }).collect::<Vec<_>>()},
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

/// What a pod check found and the command that fixes it. Every check warns;
/// none refuses (decision d-12).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Warning {
    /// The slot, role or deliverable it is about.
    pub about: String,
    /// What is wrong.
    pub text: String,
    /// The command (or step) that fixes it.
    pub fix: String,
}

impl Warning {
    /// One line: `<about>: <text>. Fix: <fix>`.
    pub fn line(&self) -> String {
        format!("{}: {}. Fix: {}", self.about, self.text, self.fix)
    }
}

/// One slot as `plan` and `up` resolve it before any launch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SlotPlan {
    /// The template's slot.
    pub slot: Slot,
    /// The filled `--set` values the slot uses, as `<name>=<value>`.
    pub sets: Vec<String>,
    /// The role's harness, from its launch settings, when it names one.
    pub harness: Option<String>,
    /// The role's model, from its launch settings, when it names one.
    pub model: Option<String>,
    /// The assembled first turn, or why it did not build.
    pub first_turn: Result<String, String>,
}

/// The `--set` values `slot` uses, as `<name>=<value>`, in its order.
pub fn slot_sets(slot: &Slot, filled: &[(String, String)]) -> Vec<String> {
    slot.parameters
        .iter()
        .filter_map(|name| {
            filled
                .iter()
                .find(|(given, _)| given == name)
                .map(|(name, value)| format!("{name}={value}"))
        })
        .collect()
}

/// The checks `plan` prints and `up` logs, from facts read before any
/// launch: `live` says whether a role has a live holder in the project, and
/// `exists` whether the project has the role. Warnings only (d-12).
pub fn checks(
    template: &Template,
    plans: &[SlotPlan],
    live: &dyn Fn(&str) -> bool,
    exists: &dyn Fn(&str) -> bool,
) -> Vec<Warning> {
    let mut warnings = Vec::new();
    for plan in plans {
        if let Err(why) = &plan.first_turn {
            warnings.push(Warning {
                about: format!("slot {}", plan.slot.id),
                text: format!("role {} cannot start: {why}", plan.slot.role),
                fix: format!(
                    "pij directory show {} --first-turn, then add or edit the role (pij directory --help)",
                    plan.slot.role
                ),
            });
        }
    }
    for consulted in template.consulted.iter().filter(|role| role.before_up) {
        if !live(&consulted.role) {
            warnings.push(Warning {
                about: format!("consulted role {}", consulted.role),
                text: "it is needed before up and has no live holder".to_string(),
                fix: format!("pij spawn --as role:{} <harness> ...", consulted.role),
            });
        }
    }
    for plan in plans {
        let Some(other) = &plan.slot.not_model_of else {
            continue;
        };
        let Some(author) = plans.iter().find(|known| &known.slot.id == other) else {
            continue;
        };
        let shared = match (&plan.model, &author.model) {
            (Some(mine), Some(theirs)) if mine == theirs => Some(format!(
                "it runs {mine}, the same model as slot {other}, whose work it reviews"
            )),
            (Some(_), Some(_)) => None,
            _ => Some(format!(
                "pij cannot tell whether it shares a model with slot {other}: a role names none"
            )),
        };
        if let Some(text) = shared {
            warnings.push(Warning {
                about: format!("slot {}", plan.slot.id),
                text,
                fix: format!(
                    "pij directory edit {} --model <another model> --why <text>",
                    plan.slot.role
                ),
            });
        }
    }
    for deliverable in &template.deliverables {
        let roles = std::iter::once(("owner", &deliverable.owner)).chain(
            deliverable
                .write_back
                .iter()
                .map(|role| ("write-back", role)),
        );
        for (what, role) in roles {
            if !exists(role) {
                warnings.push(Warning {
                    about: format!("deliverable {}", deliverable.id),
                    text: format!("its {what} role {role} does not exist in this project"),
                    fix: format!("pij directory add {role} --type <type> --why <text>"),
                });
            }
        }
    }
    warnings
}

/// The canary message a slot's seat is sent after it starts (req-19).
pub fn canary_prompt(pod: &str, slot: &Slot) -> String {
    format!(
        "pij pod {pod}: tool canary for slot {}. Confirm each tool this slot needs that you can use: {}. Run:\n\n  pij pod confirm {pod} --tool <tool> [--tool <tool>]...\n\nnaming only the tools you can use. Leave out any you cannot; pij reports it with its fix.",
        slot.id,
        slot.tools.join(", ")
    )
}

/// The tools `slot` names that `confirmed` leaves out.
pub fn missing(slot: &Slot, confirmed: &[String]) -> Vec<String> {
    slot.tools
        .iter()
        .filter(|tool| !confirmed.contains(tool))
        .cloned()
        .collect()
}

/// A member's canary line: what is missing and how to fix it.
pub fn degraded(pod: &str, missing: &[String], seat: &str) -> String {
    format!(
        "degraded: {seat} did not confirm {}; pij hands it no pod work until it does. Fix: give the seat those tools, then have it run pij pod confirm {pod} --tool <tool>...",
        missing.join(", ")
    )
}

/// One member row for the pod's record.
pub fn member(slot: &Slot, seat: &str, canary: &str) -> Value {
    let mut row = json!({"slot": slot.id, "role": slot.role, "seat": seat, "canary": canary});
    if !slot.instance.is_empty() {
        row["instance"] = json!(slot.instance);
    }
    if let Some(keep) = slot.keep_open {
        row["keep_open"] = json!(if keep { "yes" } else { "no" });
    }
    row
}

/// `document` with `rows` appended to its members.
pub fn with_members(mut document: Value, rows: Vec<Value>) -> Value {
    if let Some(members) = section_mut(&mut document, "members").and_then(Value::as_array_mut) {
        members.extend(rows);
    }
    document
}

/// `document` with `seat`'s canary set to `canary`.
pub fn with_canary(mut document: Value, seat: &str, canary: &str) -> Value {
    if let Some(members) = section_mut(&mut document, "members").and_then(Value::as_array_mut) {
        for member in members.iter_mut().filter(|member| member["seat"] == seat) {
            member["canary"] = json!(canary);
        }
    }
    document
}

fn section_mut<'a>(document: &'a mut Value, name: &str) -> Option<&'a mut Value> {
    document["sections"]
        .as_array_mut()?
        .iter_mut()
        .find(|section| section["name"] == name)
        .map(|section| &mut section["value"])
}

/// A pod record's section `name` (its rows, for an array section).
pub fn records<'a>(document: &'a Value, name: &str) -> &'a [Value] {
    rows(document, name)
}

/// A pod record's meta.
pub fn meta(document: &Value) -> Value {
    section(document, "meta").cloned().unwrap_or(Value::Null)
}

/// The slots `stage` of the recorded `deliverable` row needs: those its
/// `stage_needs` name, else every member's slot.
pub fn stage_needs(deliverable: &Value, stage: &str, members: &[Value]) -> Vec<String> {
    deliverable["stage_needs"]
        .as_array()
        .and_then(|needs| needs.iter().find(|need| need["stage"] == stage))
        .map(|need| {
            need["slots"]
                .as_array()
                .map(|slots| {
                    slots
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default()
        })
        .unwrap_or_else(|| {
            members
                .iter()
                .filter_map(|member| member["slot"].as_str().map(str::to_string))
                .collect()
        })
}

/// A recorded deliverable's stages in order: each recorded one with what it
/// folds in, and each open one with the slots it still waits on, those its
/// needs name that no recorded stage of the deliverable has folded in.
pub fn stage_view(deliverable: &Value, recorded: &[Value], members: &[Value]) -> Vec<Value> {
    let id = deliverable["id"].as_str().unwrap_or_default();
    let mine: Vec<&Value> = recorded
        .iter()
        .filter(|row| row["deliverable"] == id)
        .collect();
    let folded: Vec<&str> = mine
        .iter()
        .flat_map(|row| row["from"].as_array().into_iter().flatten())
        .filter_map(Value::as_str)
        .collect();
    deliverable["stages"]
        .as_array()
        .map_or(&[][..], Vec::as_slice)
        .iter()
        .filter_map(Value::as_str)
        .map(|stage| match mine.iter().find(|row| row["stage"] == stage) {
            Some(row) => json!({"stage": stage, "recorded": true, "output": row["output"], "from": row["from"], "write_back": row["write_back"]}),
            None => json!({
                "stage": stage,
                "recorded": false,
                "waits_on": stage_needs(deliverable, stage, members)
                    .into_iter()
                    .filter(|slot| !folded.contains(&slot.as_str()))
                    .collect::<Vec<_>>(),
            }),
        })
        .collect()
}

/// `document` with one recorded stage row added (a `stages` section is made
/// on the first).
pub fn with_stage(mut document: Value, row: Value) -> Value {
    if section_mut(&mut document, "stages").is_none()
        && let Some(sections) = document["sections"].as_array_mut()
    {
        sections.push(json!({"name": "stages", "value": []}));
    }
    if let Some(rows) = section_mut(&mut document, "stages").and_then(Value::as_array_mut) {
        rows.push(row);
    }
    document
}

/// `document` with parameter `name` set to `value` (replaced, or added).
///
/// `subject` and `work-folder` also drive the pod (its seats' subject, the
/// freeze at down), so their meta copies change in the same write and never
/// diverge from the parameter (#346 B346-R1-06).
pub fn with_parameter(mut document: Value, name: &str, value: &str) -> Value {
    if let Some(rows) = section_mut(&mut document, "parameters").and_then(Value::as_array_mut) {
        match rows.iter_mut().find(|row| row["name"] == name) {
            Some(row) => row["value"] = json!(value),
            None => rows.push(json!({"name": name, "value": value})),
        }
    }
    let operational = match name {
        "subject" => Some("subject"),
        "work-folder" => Some("work_folder"),
        _ => None,
    };
    if let Some(field) = operational
        && let Some(meta) = section_mut(&mut document, "meta")
    {
        meta[field] = json!(value);
    }
    document
}

/// Refuse any `--set` value that looks like a secret before anything is
/// written or sent: a pod's parameters and log are committed to the
/// governance branch, which may be public (#346 B346-R1-01). The existing
/// first-turn brake, applied to every parameter, used or not; it only ever
/// commits less. The refusal names the parameter, never the value.
///
/// # Errors
/// [`super::first_turn::E_SECRET`] naming the parameter.
pub fn refuse_secret_parameters(sets: &[(String, String)]) -> Result<(), Refusal> {
    match sets
        .iter()
        .find(|(_, value)| super::first_turn::secret_value(value))
    {
        Some((name, _)) => Err(Refusal::new(
            super::first_turn::E_SECRET,
            format!(
                "--set {name} looks like a secret (a KEY, TOKEN, SECRET or PASSWORD assignment, or a long mixed-case run); a pod's parameters are committed to the governance branch, which may be public. Pass a reference to where the secret lives instead"
            ),
        )),
        None => Ok(()),
    }
}

/// A seat already in another pod (req-01, "refused by a second").
pub const E_POD_MEMBER: &str = "E-RS-POD-MEMBER";

/// Refuse to add `seat` to `pod` when another pod's record in `worktree`
/// already holds it, naming that pod and the fix (#346 B346-R1-05). Record
/// integrity, not a gate on anyone: two pods must never both own one seat's
/// close. A keep-open member left running by a pod that is not up no longer
/// belongs to it. Read inside the writer's build, so it is the current state.
///
/// # Errors
/// [`E_POD_MEMBER`].
pub fn refuse_second_pod(worktree: &Path, pod: &str, seat: &str) -> Result<(), Refusal> {
    let Ok(entries) = std::fs::read_dir(worktree.join(COMMUNITY).join("pods")) else {
        return Ok(());
    };
    for entry in entries.flatten() {
        let other = entry.file_name().to_string_lossy().to_string();
        if other == pod {
            continue;
        }
        let Some(document) = std::fs::read(entry.path().join(POD_FILE))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        else {
            continue;
        };
        let up = meta(&document)["state"] == "up";
        if records(&document, "members")
            .iter()
            .any(|row| row["seat"] == seat && (up || row["keep_open"] != "yes"))
        {
            return Err(Refusal::new(
                E_POD_MEMBER,
                format!(
                    "{seat} is already a member of pod {other}; a seat belongs to one pod. Release it first: pij pod release {other} {seat}"
                ),
            ));
        }
    }
    Ok(())
}

/// The pod's current work folder: its `work-folder` parameter, which `set`
/// may have given after up, else the one recorded at up (#346 B346-R1-06).
pub fn work_folder(document: &Value) -> Option<String> {
    records(document, "parameters")
        .iter()
        .find(|row| row["name"] == "work-folder")
        .and_then(|row| row["value"].as_str())
        .map(str::to_string)
        .or_else(|| meta(document)["work_folder"].as_str().map(str::to_string))
        .filter(|folder| !folder.is_empty())
}

/// `document` with its meta.state set to `state` (`up`, `closing`, `closed`).
pub fn with_state(mut document: Value, state: &str) -> Value {
    if let Some(meta) = section_mut(&mut document, "meta") {
        meta["state"] = json!(state);
    }
    document
}

/// `document` without `seat`'s member row, and that row when it had one.
pub fn without_member(mut document: Value, seat: &str) -> (Value, Option<Value>) {
    let mut removed = None;
    if let Some(rows) = section_mut(&mut document, "members").and_then(Value::as_array_mut)
        && let Some(index) = rows.iter().position(|row| row["seat"] == seat)
    {
        removed = Some(rows.remove(index));
    }
    (document, removed)
}

/// Whether pij may hand `member` pod work: a brake on pij's own dispatch,
/// which only ever sends less. A degraded member gets none; anyone may still
/// send it anything.
pub fn may_dispatch(member: &Value) -> bool {
    !member["canary"]
        .as_str()
        .is_some_and(|canary| canary.starts_with("degraded"))
}

/// Whether `pod down` leaves `member` running (req-10): marked keep-open, or
/// unmarked and its role is of type knowledge.
pub fn keeps_open(member: &Value, role_type: Option<&str>) -> bool {
    match member["keep_open"].as_str() {
        Some("yes") => true,
        Some("no") => false,
        _ => role_type == Some("knowledge"),
    }
}

/// Where a pod's work folder stood at `down` (req-14).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Freeze {
    /// The checked-out branch, or `(detached)`.
    pub branch: String,
    /// The head commit, or `(initial)` before the first.
    pub head: String,
    /// Changed or untracked paths.
    pub uncommitted: usize,
}

impl Freeze {
    /// One line for the pod's log.
    pub fn line(&self, folder: &str) -> String {
        format!(
            "freeze: {folder} is on {} at {}{}",
            self.branch,
            self.head,
            if self.uncommitted == 0 {
                String::new()
            } else {
                format!(
                    "; warning: the run is unfrozen, {} uncommitted path(s) there",
                    self.uncommitted
                )
            }
        )
    }
}

/// Parse `git status --porcelain=v2 --branch`.
pub fn parse_freeze(status: &str) -> Freeze {
    let header = |key: &str| {
        status
            .lines()
            .find_map(|line| line.strip_prefix(&format!("# branch.{key} ")))
            .unwrap_or_default()
            .to_string()
    };
    Freeze {
        branch: header("head"),
        head: header("oid"),
        uncommitted: status
            .lines()
            .filter(|line| !line.starts_with('#') && !line.is_empty())
            .count(),
    }
}

/// `folder`'s branch, head and uncommitted paths: ONE git launch (a read,
/// taking no index lock).
///
/// # Errors
/// Why git could not say.
pub fn freeze(folder: &Path) -> Result<Freeze, String> {
    let output = super::git::git(
        folder,
        &["status", "--porcelain=v2", "--branch"],
        super::git::Locks::Read,
    )
    .map_err(|error| format!("git could not run: {error}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    Ok(parse_freeze(&String::from_utf8_lossy(&output.stdout)))
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

    /// #346 B346-R1-01: a secret-shaped value is refused naming the
    /// parameter; a real path with capitals is not a secret.
    #[test]
    fn a_secret_shaped_value_is_refused_and_a_path_is_not() {
        let refused =
            refuse_secret_parameters(&[("branch".into(), "API_TOKEN=abcd1234EFGH5678".into())])
                .unwrap_err();
        assert_eq!(refused.code, super::super::first_turn::E_SECRET);
        assert!(refused.message.contains("--set branch"), "{refused}");
        assert!(!refused.message.contains("abcd1234"), "{refused}");
        let run = "aB3dE5fG7hJ9kL1mN3pQ5rS7tU9vW1xY3zA";
        assert!(refuse_secret_parameters(&[("note".into(), run.into())]).is_err());
        assert!(
            refuse_secret_parameters(&[(
                "work-folder".into(),
                "/Users/Someone/Work/pij-laneA/T/pij-pod-late-folder-123-456".into()
            )])
            .is_ok()
        );
        assert!(
            refuse_secret_parameters(&[("work-folder".into(), format!("/tmp/{run}"))]).is_err(),
            "a secret as a path segment is still caught"
        );
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

    fn with_procedures(procedures: &Value) -> Vec<u8> {
        let mut document: Value = serde_json::from_slice(&template_bytes("branch")).unwrap();
        document["sections"]
            .as_array_mut()
            .unwrap()
            .push(json!({"name": "procedures", "value": procedures}));
        serde_json::to_vec(&document).unwrap()
    }

    /// Plan 253 ac-0001: a template's procedures are read with where they
    /// live; a bad id, an unknown from or use, or two close procedures refuse.
    #[test]
    fn a_template_reads_its_procedures_and_refuses_two_close_ones() {
        let template = parse(
            &with_procedures(&json!([
                {"id": "PROC-pod-0001", "from": "template", "use": "close"},
                {"id": "PROC-fleet-0002", "from": "repo"},
            ])),
            "pods/build",
        )
        .unwrap();
        assert_eq!(template.procedures[1].from, ProcedureFrom::Repo);
        assert_eq!(template.close_procedure().unwrap().id, "PROC-pod-0001");
        for (bad, why) in [
            (
                json!([{"id": "pod-0001", "from": "template"}]),
                "not a procedure id",
            ),
            (
                json!([{"id": "PROC-pod-0001", "from": "elsewhere"}]),
                "from must be",
            ),
            (
                json!([{"id": "PROC-pod-0001", "from": "repo", "use": "later"}]),
                "use must be",
            ),
            (
                json!([
                    {"id": "PROC-pod-0001", "from": "repo", "use": "close"},
                    {"id": "PROC-pod-0002", "from": "repo", "use": "close"},
                ]),
                "more than one close procedure",
            ),
        ] {
            let refused = parse(&with_procedures(&bad), "pods/build").unwrap_err();
            assert!(refused.message.contains(why), "{why}: {refused}");
        }
    }

    /// Plan 253 ac-0002: a procedure is present where the template says, and
    /// a linked one missing from the repo warns with its fix.
    #[test]
    fn a_linked_procedure_missing_from_the_repo_warns() {
        let scratch = std::env::temp_dir().join(format!(
            "pij-pod-procedures-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let repo = scratch.join("repo");
        let folder = scratch.join("tpl");
        std::fs::create_dir_all(folder.join("procedures/pod/PROC-pod-0001-close")).unwrap();
        std::fs::write(
            folder.join("procedures/pod/PROC-pod-0001-close/procedure.dd.json"),
            "{}",
        )
        .unwrap();
        std::fs::create_dir_all(repo.join("work/fleet/PROC-fleet-0002-branch")).unwrap();
        std::fs::write(
            repo.join("work/fleet/PROC-fleet-0002-branch/procedure.dd.json"),
            "{}",
        )
        .unwrap();
        std::fs::write(repo.join(".pij-procedures.json"), r#"{"home": "work"}"#).unwrap();
        let mut template = parse(
            &with_procedures(&json!([
                {"id": "PROC-pod-0001", "from": "template"},
                {"id": "PROC-fleet-0002", "from": "repo"},
                {"id": "PROC-fleet-0003", "from": "repo"},
                {"id": "PROC-pod-0004", "from": "template"},
            ])),
            "tpl",
        )
        .unwrap();
        template.folder = folder;
        let present = |procedure: &ProcedureRef| procedure_present(&template, &repo, procedure);
        let found: Vec<bool> = template.procedures.iter().map(present).collect();
        assert_eq!(found, [true, true, false, false]);
        let warned = procedure_checks(&template, &present);
        assert_eq!(
            warned.iter().map(|w| w.about.as_str()).collect::<Vec<_>>(),
            ["procedure PROC-fleet-0003", "procedure PROC-pod-0004"]
        );
        assert!(warned[0].fix.contains("pij proc new"), "{:?}", warned[0]);
        assert!(
            warned[1]
                .fix
                .contains("procedures/<area>/PROC-pod-0004-<slug>"),
            "{:?}",
            warned[1]
        );
        std::fs::remove_dir_all(&scratch).unwrap();
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

    fn planned(template: &Template, models: [Option<&str>; 3]) -> Vec<SlotPlan> {
        template
            .slots
            .iter()
            .zip(models)
            .map(|(slot, model)| SlotPlan {
                slot: slot.clone(),
                sets: Vec::new(),
                harness: None,
                model: model.map(str::to_string),
                first_turn: Ok(String::new()),
            })
            .collect()
    }

    /// d-12: each check warns with its fix; nothing here refuses.
    #[test]
    fn the_checks_warn_with_their_fixes_and_only_when_a_fact_is_missing() {
        let template = parse(&template_bytes("branch"), "pods/build").unwrap();
        let mut plans = planned(&template, [Some("m1"), Some("m1"), Some("m1")]);
        let warned = checks(&template, &plans, &|_| false, &|_| false);
        let abouts: Vec<&str> = warned
            .iter()
            .map(|warning| warning.about.as_str())
            .collect();
        assert_eq!(
            abouts,
            [
                "consulted role security",
                "slot reviewer",
                "deliverable pr",
                "deliverable pr"
            ]
        );
        assert!(warned[1].text.contains("the same model as slot coder"));
        assert!(warned[3].text.contains("write-back role docs-keeper"));
        assert!(warned[3].fix.starts_with("pij directory add docs-keeper"));

        plans[1].first_turn = Err("role coder does not exist".to_string());
        plans[2].model = Some("m2".to_string());
        let warned = checks(&template, &plans, &|_| true, &|_| true);
        assert_eq!(warned.len(), 1, "{warned:?}");
        assert!(
            warned[0]
                .line()
                .starts_with("slot coder: role coder cannot start")
        );
    }

    #[test]
    fn a_tool_left_unconfirmed_degrades_only_that_seat() {
        let template = parse(&template_bytes("branch"), "pods/build").unwrap();
        let coder = &template.slots[1];
        assert!(missing(coder, &["git".to_string()]).is_empty());
        let left = missing(coder, &[]);
        assert_eq!(left, ["git"]);
        let document = with_members(
            json!({"sections": [{"name": "members", "value": []}]}),
            vec![
                member(coder, "pij-a", "pending"),
                member(&template.slots[0], "pij-b", "none asked"),
            ],
        );
        let document = with_canary(document, "pij-a", &degraded("build", &left, "pij-a"));
        let members = records(&document, "members");
        assert!(
            members[0]["canary"]
                .as_str()
                .unwrap()
                .starts_with("degraded: pij-a did not confirm git")
        );
        assert_eq!(members[0]["instance"], "build");
        assert_eq!(members[1]["canary"], "none asked");
    }

    #[test]
    fn a_freeze_reads_branch_head_and_uncommitted_paths() {
        let clean = parse_freeze("# branch.oid abc123\n# branch.head main\n");
        assert_eq!(
            (
                clean.branch.as_str(),
                clean.head.as_str(),
                clean.uncommitted
            ),
            ("main", "abc123", 0)
        );
        assert_eq!(clean.line("/w"), "freeze: /w is on main at abc123");
        let dirty = parse_freeze(
            "# branch.oid abc123\n# branch.head main\n1 .M N... 100644 100644 100644 a b f.rs\n? new.txt\n",
        );
        assert_eq!(dirty.uncommitted, 2);
        assert!(
            dirty
                .line("/w")
                .ends_with("the run is unfrozen, 2 uncommitted path(s) there")
        );
    }

    /// req-10: a knowledge member stays open unless marked not; a mark wins.
    #[test]
    fn keep_open_follows_the_mark_then_the_knowledge_type() {
        assert!(keeps_open(&json!({}), Some("knowledge")));
        assert!(!keeps_open(&json!({"keep_open": "no"}), Some("knowledge")));
        assert!(keeps_open(&json!({"keep_open": "yes"}), Some("coder")));
        assert!(!keeps_open(&json!({}), Some("coder")));
        assert!(!may_dispatch(&json!({"canary": "degraded: x"})));
        assert!(may_dispatch(&json!({"canary": "confirmed"})));
    }

    /// req-16: an open stage waits only on the slots it needs that no
    /// recorded stage folded in; a template naming an unknown stage or slot
    /// for a stage's needs is refused.
    #[test]
    fn an_open_stage_waits_only_on_what_no_recorded_stage_folded_in() {
        let deliverable = json!({"id": "pr", "stages": ["draft", "final"],
            "stage_needs": [{"stage": "draft", "slots": ["pm"]}]});
        let members = [
            json!({"slot": "pm"}),
            json!({"slot": "coder"}),
            json!({"slot": "reviewer"}),
        ];
        let view = stage_view(&deliverable, &[], &members);
        assert_eq!(view[0]["waits_on"], json!(["pm"]));
        assert_eq!(view[1]["waits_on"], json!(["pm", "coder", "reviewer"]));
        let recorded = [
            json!({"deliverable": "pr", "stage": "draft", "output": "d.md", "from": ["pm", "coder"]}),
        ];
        let view = stage_view(&deliverable, &recorded, &members);
        assert_eq!(view[0]["recorded"], true);
        assert_eq!(view[1]["waits_on"], json!(["reviewer"]));

        let mut bytes: Value = serde_json::from_slice(&template_bytes("branch")).unwrap();
        bytes["sections"][4]["value"][0]["stage_needs"] =
            json!([{"stage": "draft", "slots": ["ghost"]}]);
        let refused = parse(&serde_json::to_vec(&bytes).unwrap(), "t").unwrap_err();
        assert!(
            refused.message.contains("needs slot \"ghost\""),
            "{refused}"
        );
        bytes["sections"][4]["value"][0]["stage_needs"] =
            json!([{"stage": "review", "slots": ["pm"]}]);
        let refused = parse(&serde_json::to_vec(&bytes).unwrap(), "t").unwrap_err();
        assert!(refused.message.contains("stage \"review\""), "{refused}");
    }
}
