//! Seat types (PRD 006, blueprint 005 c-type-catalog): pij's fixed set of ten,
//! each shipped as a `pij/type` definition and a seed template under
//! `templates/types/<type>/v<n>/`, compiled in like the role templates. Pure
//! except [`seed`], which reads one governance file.
//!
//! A type's rules are checked when a role document is written, never enforced
//! on a running seat (blueprint d-3).

use std::path::Path;
use std::sync::OnceLock;

use serde_json::{Value, json};

use super::Refusal;
use super::layout::COMMUNITY;

/// No type on a role that must have one.
pub const E_TYPE_MISSING: &str = "E-RS-TYPE-MISSING";
/// A type name pij does not ship.
pub const E_TYPE_UNKNOWN: &str = "E-RS-TYPE-UNKNOWN";
/// A field the role's type requires is missing.
pub const E_TYPE_FIELD: &str = "E-RS-TYPE-FIELD";
/// A role changes a fixed rule of its type, or names a rule it does not have.
pub const E_TYPE_RULE: &str = "E-RS-TYPE-RULE";

/// The ten types, in the order pij lists them.
pub const TYPES: [&str; 10] = [
    "prime",
    "pa",
    "project-manager",
    "coder",
    "reviewer",
    "worker",
    "knowledge",
    "steward",
    "watcher",
    "human-window",
];

macro_rules! shipped {
    ($(($name:literal, $version:literal)),* $(,)?) => {
        [$((
            $name,
            $version,
            include_bytes!(concat!(
                "../../../../templates/types/", $name, "/v", $version, "/type.dd.json"
            )) as &'static [u8],
            include_str!(concat!(
                "../../../../templates/types/", $name, "/v", $version, "/seed.md"
            )),
        )),*]
    };
}

/// Every shipped type version. A version is never edited after it ships.
const SHIPPED: [(&str, u32, &[u8], &str); 10] = shipped![
    ("prime", 1),
    ("pa", 1),
    ("project-manager", 1),
    ("coder", 1),
    ("reviewer", 1),
    ("worker", 1),
    ("knowledge", 1),
    ("steward", 1),
    ("watcher", 1),
    ("human-window", 1),
];

/// Shipped role templates whose type is clear (blueprint d-7). A template not
/// here supplies no type, so a role made from it needs `--type`.
pub const TEMPLATE_TYPES: [(&str, &str); 10] = [
    ("coder", "coder"),
    ("note-taker", "worker"),
    ("pa", "pa"),
    ("pm", "project-manager"),
    ("prime", "prime"),
    ("qa", "reviewer"),
    ("reviewer", "reviewer"),
    ("security", "reviewer"),
    ("topic-expert", "knowledge"),
    ("transcriber", "worker"),
];

/// A role field a type may require, and the flag that gives it.
const REQUIRED_FIELDS: [(&str, &str); 2] =
    [("keeps", "--keeps"), ("sole_writer_of", "--sole-writer-of")];

/// One rule of a type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rule {
    /// Stable id a role's `type_changes` names.
    pub id: String,
    /// The rule in words.
    pub text: String,
    /// Whether a role may change it (with a reason).
    pub fixed: bool,
}

/// One shipped type definition.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TypeDef {
    /// The type id.
    pub id: String,
    /// The shipped version.
    pub version: u32,
    /// The definition document.
    pub document: Value,
    /// The shipped seed text.
    pub seed: &'static str,
    /// The role fields the type requires (`keeps`, `sole_writer_of`).
    pub requires: Vec<String>,
    /// Its rules.
    pub rules: Vec<Rule>,
}

impl TypeDef {
    fn section(&self, name: &str) -> &Value {
        section(&self.document, name).unwrap_or(&Value::Null)
    }

    /// `meta.line`: one line on what the type is for.
    pub fn line(&self) -> &str {
        self.section("meta")["line"].as_str().unwrap_or_default()
    }

    /// Whether the type carries authority of its own (prime, pa, project-manager).
    pub fn authority(&self) -> bool {
        self.section("meta")["authority"] == "carries"
    }

    /// `pij directory types <type>`: everything the definition says.
    pub fn show(&self) -> Value {
        json!({
            "id": self.id,
            "version": self.version,
            "title": self.section("meta")["title"],
            "authority": self.authority(),
            "line": self.line(),
            "purpose": self.section("purpose"),
            "lifecycle": self.section("lifecycle"),
            "keeps": self.section("keeps"),
            "requires": self.requires,
            "rules": self.rules.iter().map(|rule| json!({
                "id": rule.id,
                "text": rule.text,
                "mode": if rule.fixed { "fixed" } else { "default" },
            })).collect::<Vec<_>>(),
            "path": format!("templates/types/{}/v{}/type.dd.json", self.id, self.version),
        })
    }
}

fn section<'a>(document: &'a Value, name: &str) -> Option<&'a Value> {
    document["sections"]
        .as_array()?
        .iter()
        .find(|section| section["name"] == name)
        .map(|section| &section["value"])
}

fn parse(id: &str, version: u32, bytes: &[u8], seed: &'static str) -> TypeDef {
    let document: Value =
        serde_json::from_slice(bytes).expect("a shipped type definition is JSON (tested)");
    let requires = section(&document, "requires")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let rules = section(&document, "rules")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|rule| Rule {
                    id: rule["id"].as_str().unwrap_or_default().to_string(),
                    text: rule["text"].as_str().unwrap_or_default().to_string(),
                    fixed: rule["mode"] == "fixed",
                })
                .collect()
        })
        .unwrap_or_default();
    TypeDef {
        id: id.to_string(),
        version,
        document,
        seed,
        requires,
        rules,
    }
}

/// Every shipped type, latest version, in [`TYPES`] order.
pub fn all() -> &'static [TypeDef] {
    static CATALOG: OnceLock<Vec<TypeDef>> = OnceLock::new();
    CATALOG.get_or_init(|| {
        TYPES
            .iter()
            .filter_map(|id| {
                SHIPPED
                    .iter()
                    .filter(|(name, ..)| name == id)
                    .max_by_key(|(_, version, ..)| *version)
                    .map(|(name, version, bytes, seed)| parse(name, *version, bytes, seed))
            })
            .collect()
    })
}

/// The ten types, one line each, for a refusal or the listing.
pub fn menu() -> String {
    all()
        .iter()
        .map(|def| format!("  {:<15} {}", def.id, def.line()))
        .collect::<Vec<_>>()
        .join("\n")
}

/// One type by id.
///
/// # Errors
/// [`E_TYPE_UNKNOWN`] listing the ten.
pub fn get(id: &str) -> Result<&'static TypeDef, Refusal> {
    all().iter().find(|def| def.id == id).ok_or_else(|| {
        Refusal::new(
            E_TYPE_UNKNOWN,
            format!("{id:?} is not a pij seat type; the ten are:\n{}", menu()),
        )
        .with_next("pij directory types")
    })
}

/// `pij directory types`: the ten, with their line and authority.
pub fn listing() -> Value {
    json!(
        all()
            .iter()
            .map(|def| json!({
                "id": def.id,
                "title": def.section("meta")["title"],
                "authority": def.authority(),
                "line": def.line(),
                "version": def.version,
            }))
            .collect::<Vec<_>>()
    )
}

/// The type a shipped role template supplies, if any.
pub fn template_type(name: &str) -> Option<&'static str> {
    TEMPLATE_TYPES
        .iter()
        .find(|(template, _)| *template == name)
        .map(|(_, kind)| *kind)
}

/// The governance path of a repo's own seed for `type_id`.
pub fn seed_path(type_id: &str) -> std::path::PathBuf {
    Path::new(COMMUNITY)
        .join("seeds")
        .join(format!("{type_id}.md"))
}

/// A governance seed that exists but cannot be read.
pub const E_SEED_UNREADABLE: &str = "E-RS-SEED-UNREADABLE";

/// The seed pij would use for `type_id`: the governance copy under
/// `worktree` when there is one, else the shipped one, with where it came from.
/// Only a missing governance copy falls back to the shipped one; any other
/// read error refuses, so a repo's seed is never silently replaced.
///
/// # Errors
/// [`E_TYPE_UNKNOWN`]; [`E_SEED_UNREADABLE`] naming the path.
pub fn seed(worktree: Option<&Path>, type_id: &str) -> Result<Value, Refusal> {
    let def = get(type_id)?;
    let local = seed_path(type_id);
    if let Some(top) = worktree {
        match std::fs::read_to_string(top.join(&local)) {
            Ok(text) => {
                return Ok(json!({
                    "type": def.id,
                    "source": "governance",
                    "path": local.display().to_string(),
                    "text": text,
                }));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(Refusal::new(
                    E_SEED_UNREADABLE,
                    format!(
                        "this repo's seed for {type_id} at {} exists but cannot be read: {error}",
                        local.display()
                    ),
                )
                .with_next(format!(
                    "Fix or replace it: pij directory seed {type_id} --file <path> --why <text>"
                )));
            }
        }
    }
    Ok(json!({
        "type": def.id,
        "source": "shipped",
        "path": format!("templates/types/{}/v{}/seed.md", def.id, def.version),
        "version": def.version,
        "text": def.seed,
    }))
}

fn meta(document: &Value) -> &Value {
    section(document, "meta").unwrap_or(&Value::Null)
}

/// `meta.type` of a role document, when it names one.
pub fn of_role(document: &Value) -> Option<&str> {
    meta(document)["type"].as_str()
}

/// Check a role document against its type (PRD 006 req-01, req-03, req-06).
/// `require` is false only for an edit of a role written before types, which
/// may stay untyped until `retype` (req-07).
///
/// # Errors
/// [`E_TYPE_MISSING`] listing the ten; [`E_TYPE_UNKNOWN`]; [`E_TYPE_FIELD`]
/// naming the flag; [`E_TYPE_RULE`] for a change to a fixed or unknown rule,
/// or one with no reason.
pub fn check_role(id: &str, document: &Value, require: bool) -> Result<(), Refusal> {
    let Some(type_id) = of_role(document) else {
        if !require {
            return Ok(());
        }
        return Err(Refusal::new(
            E_TYPE_MISSING,
            format!(
                "role {id:?} names no type; every role is one of pij's ten seat types:\n{}",
                menu()
            ),
        )
        .with_next(format!(
            "Name one: pij directory add {id} --type <type> ... (pij directory types <type> explains each)"
        )));
    };
    let def = get(type_id)?;
    for field in &def.requires {
        let given = meta(document)[field.as_str()]
            .as_str()
            .is_some_and(|value| !value.trim().is_empty());
        if !given {
            let flag = REQUIRED_FIELDS
                .iter()
                .find(|(name, _)| name == field)
                .map_or(field.as_str(), |(_, flag)| flag);
            return Err(Refusal::new(
                E_TYPE_FIELD,
                format!("role {id:?} is of type {type_id}, which requires {flag} <value>"),
            )
            .with_next(format!("pij directory types {type_id}")));
        }
    }
    for change in section(document, "type_changes")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
    {
        let rule_id = change["rule"].as_str().unwrap_or_default();
        let Some(rule) = def.rules.iter().find(|rule| rule.id == rule_id) else {
            let rules: Vec<&str> = def.rules.iter().map(|rule| rule.id.as_str()).collect();
            return Err(Refusal::new(
                E_TYPE_RULE,
                format!(
                    "role {id:?}: type {type_id} has no rule {rule_id:?}; its rules are {}",
                    rules.join(", ")
                ),
            )
            .with_next(format!("pij directory types {type_id}")));
        };
        if rule.fixed {
            return Err(Refusal::new(
                E_TYPE_RULE,
                format!(
                    "role {id:?}: rule {rule_id:?} of type {type_id} is fixed and no role may change it ({})",
                    rule.text
                ),
            ));
        }
        if change["why"]
            .as_str()
            .is_none_or(|why| why.trim().is_empty())
        {
            return Err(Refusal::new(
                E_TYPE_RULE,
                format!(
                    "role {id:?}: the change to rule {rule_id:?} needs a reason: --type-change {rule_id}=<why>"
                ),
            ));
        }
    }
    Ok(())
}

/// The type `retype` would give a role written before types (PRD 006
/// req-07, plan 238), and why: its template's (the catalog's table), else its
/// id's (`prime`, `pa`, `pm` or `pm-<n>`, `coder*`, `reviewer*`, `security*`,
/// `qa*`). `None` when neither names one.
pub fn propose(id: &str, template: Option<&str>) -> Option<(&'static str, String)> {
    if let Some(name) = template
        && let Some(kind) = template_type(name)
    {
        return Some((kind, format!("from template {name}")));
    }
    let pm = id
        .strip_prefix("pm-")
        .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()));
    let kind = match id {
        "prime" => "prime",
        "pa" => "pa",
        "pm" => "project-manager",
        _ if pm => "project-manager",
        _ if id.starts_with("coder") => "coder",
        _ if ["reviewer", "security", "qa"]
            .iter()
            .any(|prefix| id.starts_with(prefix)) =>
        {
            "reviewer"
        }
        _ => return None,
    };
    Some((kind, format!("from the id {id}")))
}

/// One untyped role `retype` looked at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Retype {
    /// Sure: the type and why.
    Decided {
        /// The role id.
        id: String,
        /// The type it gets.
        kind: &'static str,
        /// Where the type came from.
        source: String,
    },
    /// Not sure: why, and the edit that types it.
    Undecided {
        /// The role id.
        id: String,
        /// Why retype left it.
        reason: String,
        /// The `pij directory edit` line that types it.
        command: String,
    },
}

impl Retype {
    /// The view of one item in `retype`'s answer.
    pub fn json(&self) -> Value {
        match self {
            Self::Decided { id, kind, source } => {
                json!({"id": id, "type": kind, "source": source})
            }
            Self::Undecided {
                id,
                reason,
                command,
            } => json!({"id": id, "reason": reason, "command": command}),
        }
    }
}

/// `document` with `meta.type` set to `kind`.
pub fn with_type(mut document: Value, kind: &str) -> Value {
    if let Some(meta) = document["sections"].as_array_mut().and_then(|sections| {
        sections
            .iter_mut()
            .find(|section| section["name"] == "meta")
    }) {
        meta["value"]["type"] = json!(kind);
    }
    document
}

/// `retype`'s plan over `(id, document)` pairs: every active role with no
/// type, decided or not. A typed or retired role is left out. A proposal the
/// role cannot meet (knowledge with no `keeps`) is left undecided.
pub fn retype_plan(project: &str, roles: &[(String, Value)]) -> Vec<Retype> {
    let edit = |id: &str, kind: &str, extra: &[&str]| {
        let set = format!("type={kind}");
        let mut words = vec!["pij", "directory", "edit", id, "--set", set.as_str()];
        words.extend_from_slice(extra);
        words.extend_from_slice(&["--why", "<why>", "--project", project]);
        super::picture::command(&words)
    };
    roles
        .iter()
        .filter(|(_, document)| of_role(document).is_none())
        .filter(|(_, document)| !super::role::standing(document).retired())
        .map(|(id, document)| {
            let Some((kind, source)) = propose(id, meta(document)["template"].as_str()) else {
                return Retype::Undecided {
                    id: id.clone(),
                    reason: "no template or id pattern names a type".to_string(),
                    command: edit(id, "<type>", &[]),
                };
            };
            match check_role(id, &with_type(document.clone(), kind), true) {
                Ok(()) => Retype::Decided {
                    id: id.clone(),
                    kind,
                    source,
                },
                Err(refusal) => {
                    let extra: &[&str] = match kind {
                        "knowledge" => &["--keeps", "<artefact>"],
                        "steward" => &["--sole-writer-of", "<thing>"],
                        _ => &[],
                    };
                    Retype::Undecided {
                        id: id.clone(),
                        reason: format!("{kind} ({source}), but {}", refusal.message),
                        command: edit(id, kind, extra),
                    }
                }
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn role(meta: Value, changes: Option<Value>) -> Value {
        let mut sections = vec![json!({"name": "meta", "value": meta})];
        if let Some(changes) = changes {
            sections.push(json!({"name": "type_changes", "value": changes}));
        }
        json!({"sections": sections})
    }

    #[test]
    fn every_type_ships_a_definition_and_a_seed_in_order() {
        let ids: Vec<&str> = all().iter().map(|def| def.id.as_str()).collect();
        assert_eq!(ids, TYPES);
        for def in all() {
            assert_eq!(meta(&def.document)["id"], def.id.as_str());
            assert!(!def.line().is_empty(), "{}", def.id);
            assert!(!def.seed.trim().is_empty(), "{}", def.id);
            assert!(!def.rules.is_empty(), "{}", def.id);
        }
        let authority: Vec<&str> = all()
            .iter()
            .filter(|def| def.authority())
            .map(|def| def.id.as_str())
            .collect();
        assert_eq!(authority, ["prime", "pa", "project-manager"]);
        assert_eq!(get("knowledge").unwrap().requires, ["keeps"]);
        assert_eq!(get("steward").unwrap().requires, ["sole_writer_of"]);
    }

    #[test]
    fn every_template_type_names_a_shipped_type() {
        for (_, kind) in TEMPLATE_TYPES {
            assert!(TYPES.contains(&kind), "{kind}");
        }
    }

    #[test]
    fn a_role_with_no_type_is_refused_with_the_ten_unless_it_predates_types() {
        let untyped = role(json!({"id": "x"}), None);
        let refusal = check_role("x", &untyped, true).unwrap_err();
        assert_eq!(refusal.code, E_TYPE_MISSING);
        for id in TYPES {
            assert!(refusal.message.contains(id), "{id}");
        }
        assert!(check_role("x", &untyped, false).is_ok());
        let unknown = role(json!({"type": "wizard"}), None);
        assert_eq!(
            check_role("x", &unknown, true).unwrap_err().code,
            E_TYPE_UNKNOWN
        );
    }

    #[test]
    fn a_type_asks_for_the_field_it_requires() {
        let refusal = check_role("x", &role(json!({"type": "knowledge"}), None), true).unwrap_err();
        assert_eq!(refusal.code, E_TYPE_FIELD);
        assert!(refusal.message.contains("--keeps"), "{}", refusal.message);
        let refusal = check_role("x", &role(json!({"type": "steward"}), None), true).unwrap_err();
        assert!(
            refusal.message.contains("--sole-writer-of"),
            "{}",
            refusal.message
        );
        let kept = role(
            json!({"type": "knowledge", "keeps": "graphs/area.json"}),
            None,
        );
        assert!(check_role("x", &kept, true).is_ok());
    }

    #[test]
    fn a_default_rule_changes_with_a_reason_and_a_fixed_one_never() {
        let coder = role(
            json!({"type": "coder"}),
            Some(json!([{"rule": "compact-after-job", "why": "keep it"}])),
        );
        let refusal = check_role("x", &coder, true).unwrap_err();
        assert_eq!(refusal.code, E_TYPE_RULE);
        assert!(refusal.message.contains("fixed"), "{}", refusal.message);
        let security = role(
            json!({"type": "reviewer"}),
            Some(json!([{"rule": "compact-after-job", "why": "carries open findings"}])),
        );
        assert!(check_role("x", &security, true).is_ok());
        let unreasoned = role(
            json!({"type": "reviewer"}),
            Some(json!([{"rule": "compact-after-job", "why": " "}])),
        );
        assert_eq!(
            check_role("x", &unreasoned, true).unwrap_err().code,
            E_TYPE_RULE
        );
        let unknown = role(
            json!({"type": "reviewer"}),
            Some(json!([{"rule": "nope", "why": "w"}])),
        );
        assert!(
            check_role("x", &unknown, true)
                .unwrap_err()
                .message
                .contains("independent")
        );
    }

    #[test]
    fn the_seed_is_the_governance_copy_when_there_is_one() {
        let shipped = seed(None, "coder").unwrap();
        assert_eq!(shipped["source"], "shipped");
        assert_eq!(shipped["path"], "templates/types/coder/v1/seed.md");
        let dir = pij_testkit::fresh_dir("types-seed");
        let local = dir.join(seed_path("coder"));
        std::fs::create_dir_all(local.parent().unwrap()).unwrap();
        std::fs::write(&local, "Our coders run just ci.\n").unwrap();
        let ours = seed(Some(&dir), "coder").unwrap();
        assert_eq!(ours["source"], "governance");
        assert_eq!(ours["text"], "Our coders run just ci.\n");
        assert_eq!(seed(Some(&dir), "pa").unwrap()["source"], "shipped");
        // Review of #344: only a missing copy falls back; an unreadable one
        // (here a directory where the file should be) refuses by name.
        std::fs::create_dir_all(dir.join(seed_path("watcher"))).unwrap();
        let refusal = seed(Some(&dir), "watcher").unwrap_err();
        assert_eq!(refusal.code, E_SEED_UNREADABLE);
        assert!(
            refusal.message.contains("seeds/watcher.md"),
            "{}",
            refusal.message
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn retype_proposes_from_the_template_then_the_id_and_leaves_the_rest() {
        let role = |id: &str, meta: Value| {
            let mut value = json!({"id": id});
            for (key, item) in meta.as_object().unwrap() {
                value[key] = item.clone();
            }
            (
                id.to_string(),
                json!({"sections": [{"name": "meta", "value": value}]}),
            )
        };
        let roles = [
            role("sec", json!({"template": "security"})),
            role("pm-004", json!({})),
            role("coder-b", json!({})),
            role("historian", json!({})),
            role("expert", json!({"template": "topic-expert"})),
            role("typed", json!({"type": "worker"})),
            role("qa-old", json!({"state": "retired"})),
        ];
        let plan = retype_plan("fleet", &roles);
        let decided: Vec<(&str, &str)> = plan
            .iter()
            .filter_map(|item| match item {
                Retype::Decided { id, kind, .. } => Some((id.as_str(), *kind)),
                Retype::Undecided { .. } => None,
            })
            .collect();
        assert_eq!(
            decided,
            [
                ("sec", "reviewer"),
                ("pm-004", "project-manager"),
                ("coder-b", "coder")
            ]
        );
        let undecided: Vec<(&str, &str)> = plan
            .iter()
            .filter_map(|item| match item {
                Retype::Undecided { id, command, .. } => Some((id.as_str(), command.as_str())),
                Retype::Decided { .. } => None,
            })
            .collect();
        assert_eq!(
            undecided,
            [
                (
                    "historian",
                    "pij directory edit historian --set 'type=<type>' --why '<why>' --project fleet"
                ),
                (
                    "expert",
                    "pij directory edit expert --set type=knowledge --keeps '<artefact>' --why '<why>' --project fleet"
                ),
            ]
        );
        assert_eq!(propose("pm-x", None), None, "pm-<n> only");
    }
}
