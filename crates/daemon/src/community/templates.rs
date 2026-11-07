//! Role templates (plan 194, roadmap 001 s03): every shipped version of every
//! template under `templates/roles/`, compiled in like the schema copies, so a
//! template needs no install and no read launches anything. Pure: no IO.
//!
//! Templates are examples, not a checklist (ruling C18): the view prompts for
//! the core roles and for security until each exists, and creates nothing
//! (ruling C22), in the Pij Emergent Communities wording (ruling C26).

use std::borrow::Cow;
use std::fmt::Write as _;

use serde_json::{Value, json};

use super::Refusal;
use super::picture::{Move, Recommendation, Signal};

/// The core roles a community is prompted for until each exists (C22).
pub const CORE: [&str; 5] = ["prime", "pm", "pa", "coder", "reviewer"];
/// The recommended role, prompted until one exists (C22).
pub const SECURITY: &str = "security";
/// What the view says above its prompts (rulings C18, C26).
pub const GUIDANCE: &str = "Pij Emergent Communities (PEC): the prime and the humans build whatever community the work needs now, and reshape it as the work changes. The templates are examples, not a checklist: start from one when it fits and edit it freely, or write a role no template describes. Nothing here is created for you.";

/// One version of one template.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Template {
    /// The template name, the folder under `templates/roles/`.
    pub name: String,
    /// The version, the `v<n>` folder.
    pub version: u32,
    /// The `role.dd.json` bytes.
    pub bytes: Cow<'static, [u8]>,
}

impl Template {
    /// A template version from its parts (tests build fixture catalogs with it).
    pub fn new(name: &str, version: u32, bytes: impl Into<Cow<'static, [u8]>>) -> Self {
        Self {
            name: name.to_string(),
            version,
            bytes: bytes.into(),
        }
    }
}

macro_rules! shipped {
    ($(($name:literal, $version:literal)),* $(,)?) => {
        [$((
            $name,
            $version,
            include_bytes!(concat!(
                "../../../../templates/roles/", $name, "/v", $version, "/role.dd.json"
            )) as &'static [u8],
        )),*]
    };
}

/// Every shipped `templates/roles/<name>/v<n>/role.dd.json`. A version is
/// never edited after it ships; a new one is a new row here.
const SHIPPED: [(&str, u32, &[u8]); 28] = shipped![
    ("coder", 1),
    ("coder", 2),
    ("coder", 3),
    ("designer", 1),
    ("devcontainer", 1),
    ("docs-keeper", 1),
    ("infra", 1),
    ("note-taker", 1),
    ("pa", 1),
    ("pa", 2),
    ("pa", 3),
    ("pm", 1),
    ("pm", 2),
    ("pm", 3),
    ("prime", 1),
    ("prime", 2),
    ("prime", 3),
    ("qa", 1),
    ("release", 1),
    ("researcher", 1),
    ("reviewer", 1),
    ("reviewer", 2),
    ("reviewer", 3),
    ("security", 1),
    ("security", 2),
    ("security", 3),
    ("topic-expert", 1),
    ("transcriber", 1),
];

/// A set of templates, sorted by name then version.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Catalog {
    templates: Vec<Template>,
}

impl Default for Catalog {
    fn default() -> Self {
        Self::shipped()
    }
}

impl Catalog {
    /// The templates compiled into this build.
    pub fn shipped() -> Self {
        Self::new(
            SHIPPED
                .iter()
                .map(|(name, version, bytes)| Template::new(name, *version, *bytes))
                .collect(),
        )
    }

    /// A catalog of the given templates.
    pub fn new(mut templates: Vec<Template>) -> Self {
        templates.sort_by(|a, b| (&a.name, a.version).cmp(&(&b.name, b.version)));
        Self { templates }
    }

    /// Every template version, sorted.
    pub fn all(&self) -> &[Template] {
        &self.templates
    }

    /// Each template name once, sorted.
    pub fn names(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self.templates.iter().map(|t| t.name.as_str()).collect();
        names.dedup();
        names
    }

    /// The versions of `name`, ascending; empty for an unknown name.
    pub fn versions(&self, name: &str) -> Vec<u32> {
        self.templates
            .iter()
            .filter(|t| t.name == name)
            .map(|t| t.version)
            .collect()
    }

    /// The latest version of `name`.
    pub fn latest(&self, name: &str) -> Option<u32> {
        self.versions(name).last().copied()
    }

    /// `name` at `version`, or its latest version.
    ///
    /// # Errors
    /// `E-RS-ARG` naming the unknown template or version and listing what exists.
    pub fn get(&self, name: &str, version: Option<u32>) -> Result<&Template, Refusal> {
        let versions = self.versions(name);
        if versions.is_empty() {
            return Err(Refusal::new(
                "E-RS-ARG",
                format!(
                    "no role template named {name:?}; templates: {}",
                    self.names().join(", ")
                ),
            )
            .with_next("pij directory templates"));
        }
        let wanted = version.unwrap_or(*versions.last().expect("not empty"));
        self.templates
            .iter()
            .find(|t| t.name == name && t.version == wanted)
            .ok_or_else(|| {
                Refusal::new(
                    "E-RS-ARG",
                    format!(
                        "template {name:?} has no v{wanted}; versions: {}",
                        versions
                            .iter()
                            .map(|v| format!("v{v}"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                )
                .with_next("pij directory templates")
            })
    }

    /// The document for `name[@version]`, ready to become role `id`: `meta.id`
    /// is `id`, `meta.template` and `meta.template_version` name where it came
    /// from, and the template's own changelog is dropped (the writer stamps
    /// the role's first row).
    ///
    /// # Errors
    /// [`Self::get`]'s refusal, or `E-RS-ARG` when a shipped template is not a
    /// role document (a build fault the embedding test also catches).
    pub fn role_from(&self, name: &str, version: Option<u32>, id: &str) -> Result<Value, Refusal> {
        let template = self.get(name, version)?;
        let mut document: Value = serde_json::from_slice(&template.bytes).map_err(|error| {
            Refusal::new(
                "E-RS-ARG",
                format!("template {name}@v{} is not JSON: {error}", template.version),
            )
        })?;
        let Some(sections) = document["sections"].as_array_mut() else {
            return Err(Refusal::new(
                "E-RS-ARG",
                format!("template {name}@v{} has no sections", template.version),
            ));
        };
        sections.retain(|section| section["name"] != "changelog");
        let Some(meta) = sections
            .iter_mut()
            .find(|section| section["name"] == "meta")
            .and_then(|section| section["value"].as_object_mut())
        else {
            return Err(Refusal::new(
                "E-RS-ARG",
                format!("template {name}@v{} has no meta", template.version),
            ));
        };
        meta.insert("id".into(), json!(id));
        meta.insert("template".into(), json!(name));
        meta.insert(
            "template_version".into(),
            json!(template.version.to_string()),
        );
        Ok(document)
    }

    /// `pij directory templates`: each template with its versions and the
    /// latest version's title, kind and summary.
    pub fn listing(&self) -> Value {
        let rows: Vec<Value> = self
            .names()
            .into_iter()
            .map(|name| {
                let latest = self.get(name, None).expect("a listed name exists");
                let document: Value = serde_json::from_slice(&latest.bytes).unwrap_or(Value::Null);
                let meta = meta_of(&document);
                json!({
                    "name": name,
                    "versions": self.versions(name),
                    "latest": latest.version,
                    "title": meta["title"],
                    "kind": meta["kind"],
                    "summary": meta["summary"],
                    "core": CORE.contains(&name),
                })
            })
            .collect();
        json!(rows)
    }

    /// `pij directory templates --show <name>[@vN]` (plan 204, F23): one
    /// template's exact embedded body and its path in pij's repository, so a
    /// prime can read it before `add --from template:`. Launches nothing.
    ///
    /// # Errors
    /// [`parse_spec`]'s or [`Self::get`]'s refusal, naming what exists.
    pub fn show(&self, spec: &str) -> Result<Value, Refusal> {
        let (name, version) = parse_spec(spec)?;
        let template = self.get(&name, version)?;
        Ok(json!({"template": {
            "name": template.name,
            "version": template.version,
            "latest": self.latest(&name),
            "path": format!("templates/roles/{}/v{}/role.dd.json", template.name, template.version),
            "body": String::from_utf8_lossy(&template.bytes),
        }}))
    }

    /// Where a role made from a template stands against the latest version:
    /// `{name, version, latest, behind}`, or `None` for a role from no
    /// template. A template this build does not ship has `latest: null` and is
    /// never `behind`.
    pub fn drift(&self, meta: &Value) -> Option<Value> {
        let name = meta["template"].as_str()?;
        let version = meta["template_version"].as_str().unwrap_or_default();
        let latest = self.latest(name);
        let behind = match (parse_version(version), latest) {
            (Some(from), Some(latest)) => latest > from,
            _ => false,
        };
        Some(json!({"name": name, "version": version, "latest": latest, "behind": behind}))
    }

    /// The upstream change between `name@from` and its latest version, as a
    /// unified diff computed in-process; empty when `from` is the latest.
    ///
    /// # Errors
    /// [`Self::get`]'s refusal for an unknown template or version.
    pub fn diff_to_latest(&self, name: &str, from: u32) -> Result<(u32, String), Refusal> {
        let old = self.get(name, Some(from))?;
        let new = self.get(name, None)?;
        if new.version <= old.version {
            return Ok((new.version, String::new()));
        }
        let text = diff(
            &String::from_utf8_lossy(&old.bytes),
            &String::from_utf8_lossy(&new.bytes),
            &format!("{name}@v{}", old.version),
            &format!("{name}@v{}", new.version),
        );
        Ok((new.version, text))
    }
}

/// `<name>`, `<name>@v<n>` or `<name>@<n>`.
///
/// # Errors
/// `E-RS-ARG` for an empty name or a version that is not a whole number.
pub fn parse_spec(spec: &str) -> Result<(String, Option<u32>), Refusal> {
    let (name, version) = match spec.split_once('@') {
        Some((name, version)) => (
            name,
            Some(parse_version(version).ok_or_else(|| {
                Refusal::new(
                    "E-RS-ARG",
                    format!("template version {version:?} must look like v1"),
                )
            })?),
        ),
        None => (spec, None),
    };
    if name.is_empty() {
        return Err(Refusal::new(
            "E-RS-ARG",
            "--from template:<name>[@v<n>] needs a template name",
        ));
    }
    Ok((name.to_string(), version))
}

/// `1` or `v1`.
pub fn parse_version(text: &str) -> Option<u32> {
    text.strip_prefix('v').unwrap_or(text).parse().ok()
}

/// The core-role and security prompts for `project`'s directory, whose roles
/// are `(id, meta.template)`: a role counts for a template when its id or its
/// template is that name (C22). Each prompt carries a runnable add line that
/// names the project, so replaying it from another repo's cwd still writes
/// to this directory (review F001), and the signal it rests on (C28): an
/// independent judgment for the reviewer and security, else a core role.
pub fn recommendations(project: &str, roles: &[(String, Option<String>)]) -> Vec<Recommendation> {
    let has = |name: &str| {
        roles
            .iter()
            .any(|(id, template)| id == name || template.as_deref() == Some(name))
    };
    CORE.iter()
        .map(|name| (*name, "core"))
        .chain(std::iter::once((SECURITY, "recommended")))
        .filter(|(name, _)| !has(name))
        .map(|(name, kind)| {
            let signal = if name == SECURITY || name == "reviewer" {
                Signal::Independent
            } else {
                Signal::Core
            };
            Recommendation::new(
                name,
                kind,
                Move::Create,
                signal,
                super::picture::command(&[
                    "pij",
                    "directory",
                    "add",
                    name,
                    "--from",
                    &format!("template:{name}"),
                    "--why",
                    &format!("{kind} role from the {name} template"),
                    "--project",
                    project,
                ]),
            )
        })
        .collect()
}

/// [`recommendations`] as the view's JSON.
pub fn prompts(project: &str, roles: &[(String, Option<String>)]) -> Vec<Value> {
    recommendations(project, roles)
        .iter()
        .map(Recommendation::json)
        .collect()
}

fn meta_of(document: &Value) -> Value {
    document["sections"]
        .as_array()
        .and_then(|sections| sections.iter().find(|section| section["name"] == "meta"))
        .map_or(Value::Null, |meta| meta["value"].clone())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Op {
    Same,
    Removed,
    Added,
}

/// A unified diff of two texts by line, three lines of context.
pub fn diff(old: &str, new: &str, old_label: &str, new_label: &str) -> String {
    let a: Vec<&str> = old.lines().collect();
    let b: Vec<&str> = new.lines().collect();
    // Longest common subsequence, filled from the end.
    let mut lcs = vec![vec![0usize; b.len() + 1]; a.len() + 1];
    for i in (0..a.len()).rev() {
        for j in (0..b.len()).rev() {
            lcs[i][j] = if a[i] == b[j] {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }
    // (op, old index, new index, line)
    let mut ops: Vec<(Op, usize, usize, &str)> = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < a.len() || j < b.len() {
        if i < a.len() && j < b.len() && a[i] == b[j] {
            ops.push((Op::Same, i, j, a[i]));
            i += 1;
            j += 1;
        } else if j < b.len() && (i == a.len() || lcs[i][j + 1] > lcs[i + 1][j]) {
            ops.push((Op::Added, i, j, b[j]));
            j += 1;
        } else {
            ops.push((Op::Removed, i, j, a[i]));
            i += 1;
        }
    }
    const CONTEXT: usize = 3;
    let changed: Vec<usize> = (0..ops.len()).filter(|&k| ops[k].0 != Op::Same).collect();
    if changed.is_empty() {
        return String::new();
    }
    let mut out = format!("--- {old_label}\n+++ {new_label}\n");
    let mut start = 0;
    while start < changed.len() {
        let mut end = start;
        while end + 1 < changed.len() && changed[end + 1] - changed[end] <= 2 * CONTEXT {
            end += 1;
        }
        let from = changed[start].saturating_sub(CONTEXT);
        let to = (changed[end] + CONTEXT + 1).min(ops.len());
        let hunk = &ops[from..to];
        let old_count = hunk.iter().filter(|op| op.0 != Op::Added).count();
        let new_count = hunk.iter().filter(|op| op.0 != Op::Removed).count();
        let _ = writeln!(
            out,
            "@@ -{},{old_count} +{},{new_count} @@",
            hunk[0].1 + usize::from(old_count > 0),
            hunk[0].2 + usize::from(new_count > 0),
        );
        for (op, _, _, line) in hunk {
            let mark = match op {
                Op::Same => ' ',
                Op::Removed => '-',
                Op::Added => '+',
            };
            let _ = writeln!(out, "{mark}{line}");
        }
        start = end + 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};

    fn files_under(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                files_under(&path, out);
            } else {
                out.push(path);
            }
        }
    }

    /// AC3: every file under templates/roles/ (but the README) is embedded,
    /// byte for byte, and nothing embedded is missing from the tree.
    #[test]
    fn every_template_file_is_embedded_byte_for_byte() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../templates/roles");
        let mut files = Vec::new();
        files_under(&root, &mut files);
        let on_disk: BTreeMap<String, Vec<u8>> = files
            .into_iter()
            .map(|path| {
                let relative = path.strip_prefix(&root).unwrap().display().to_string();
                (relative, std::fs::read(&path).unwrap())
            })
            .filter(|(relative, _)| relative != "README.md")
            .collect();
        let embedded: BTreeMap<String, Vec<u8>> = Catalog::shipped()
            .all()
            .iter()
            .map(|t| {
                (
                    format!("{}/v{}/role.dd.json", t.name, t.version),
                    t.bytes.to_vec(),
                )
            })
            .collect();
        assert_eq!(
            on_disk.keys().collect::<Vec<_>>(),
            embedded.keys().collect::<Vec<_>>(),
            "add every templates/roles/<name>/v<n>/role.dd.json to SHIPPED, and nothing else lives there"
        );
        assert_eq!(on_disk, embedded);
        for t in Catalog::shipped().all() {
            let document: Value = serde_json::from_slice(&t.bytes).unwrap();
            let meta = meta_of(&document);
            assert_eq!(meta["template"], t.name.as_str(), "{}", t.name);
            assert_eq!(
                meta["template_version"],
                t.version.to_string(),
                "{}",
                t.name
            );
        }
    }

    fn fixture() -> Catalog {
        let doc = |rule: &str| {
            format!(
                "{{\n\t\"dd\": {{\"schema\": \"pij/role\"}},\n\t\"sections\": [\n\t\t{{\"name\": \"meta\", \"value\": {{\"id\": \"qa\", \"template\": \"qa\"}}}},\n\t\t{{\"name\": \"scope\", \"value\": {{\"rules\": [\"{rule}\"]}}}},\n\t\t{{\"name\": \"changelog\", \"value\": [{{\"date\": \"d\", \"by\": \"b\", \"why\": \"w\"}}]}}\n\t]\n}}\n"
            )
            .into_bytes()
        };
        Catalog::new(vec![
            Template::new("qa", 2, doc("Test twice.")),
            Template::new("qa", 1, doc("Test once.")),
        ])
    }

    #[test]
    fn get_picks_the_latest_and_refuses_unknown_names_and_versions_by_name() {
        let catalog = fixture();
        assert_eq!(catalog.versions("qa"), [1, 2]);
        assert_eq!(catalog.get("qa", None).unwrap().version, 2);
        assert_eq!(catalog.get("qa", Some(1)).unwrap().version, 1);
        let unknown = catalog.get("nope", None).unwrap_err();
        assert_eq!(unknown.code, "E-RS-ARG");
        assert!(unknown.message.contains("\"nope\"") && unknown.message.contains("qa"));
        let version = catalog.get("qa", Some(7)).unwrap_err();
        assert!(
            version.message.contains("v7") && version.message.contains("v1, v2"),
            "{}",
            version.message
        );
        assert_eq!(parse_spec("pm").unwrap(), ("pm".into(), None));
        assert_eq!(parse_spec("pm@v2").unwrap(), ("pm".into(), Some(2)));
        assert_eq!(parse_spec("pm@3").unwrap(), ("pm".into(), Some(3)));
        assert!(parse_spec("pm@latest").is_err() && parse_spec("@v1").is_err());
    }

    #[test]
    fn a_role_from_a_template_takes_its_id_and_records_where_it_came_from() {
        let document = fixture().role_from("qa", Some(1), "qa-web").unwrap();
        let meta = meta_of(&document);
        assert_eq!(meta["id"], "qa-web");
        assert_eq!(meta["template"], "qa");
        assert_eq!(meta["template_version"], "1");
        assert!(
            document["sections"]
                .as_array()
                .unwrap()
                .iter()
                .all(|section| section["name"] != "changelog"),
            "the template's changelog is not the role's"
        );
        assert!(Catalog::shipped().role_from("pm", None, "pm-001").is_ok());
    }

    /// Plan 204 (F23): --show gives the exact embedded body and its path, and
    /// refuses an unknown name or version by name.
    #[test]
    fn show_gives_the_exact_body_and_path() {
        let catalog = fixture();
        let shown = catalog.show("qa@v1").unwrap();
        let template = &shown["template"];
        assert_eq!(template["version"], 1);
        assert_eq!(template["latest"], 2);
        assert_eq!(template["path"], "templates/roles/qa/v1/role.dd.json");
        assert_eq!(
            template["body"].as_str().unwrap().as_bytes(),
            &*catalog.get("qa", Some(1)).unwrap().bytes
        );
        assert_eq!(catalog.show("qa").unwrap()["template"]["version"], 2);
        assert!(catalog.show("nope").unwrap_err().message.contains("qa"));
        assert!(
            catalog
                .show("qa@v9")
                .unwrap_err()
                .message
                .contains("v1, v2")
        );
        assert_eq!(catalog.show("qa@next").unwrap_err().code, "E-RS-ARG");
    }

    #[test]
    fn drift_is_behind_only_when_a_later_version_ships() {
        let catalog = fixture();
        let behind = catalog
            .drift(&json!({"template": "qa", "template_version": "1"}))
            .unwrap();
        assert_eq!(behind["behind"], true);
        assert_eq!(behind["latest"], 2);
        let current = catalog
            .drift(&json!({"template": "qa", "template_version": "2"}))
            .unwrap();
        assert_eq!(current["behind"], false);
        assert!(catalog.drift(&json!({"id": "own"})).is_none());
        let foreign = catalog
            .drift(&json!({"template": "zz", "template_version": "1"}))
            .unwrap();
        assert_eq!(foreign["behind"], false);
        assert!(foreign["latest"].is_null());
    }

    #[test]
    fn the_diff_shows_the_upstream_change_and_nothing_when_current() {
        let catalog = fixture();
        let (latest, text) = catalog.diff_to_latest("qa", 1).unwrap();
        assert_eq!(latest, 2);
        assert!(text.starts_with("--- qa@v1\n+++ qa@v2\n@@ "), "{text}");
        assert!(
            text.contains("-\t\t{\"name\": \"scope\", \"value\": {\"rules\": [\"Test once.\"]}},")
        );
        assert!(
            text.contains("+\t\t{\"name\": \"scope\", \"value\": {\"rules\": [\"Test twice.\"]}},")
        );
        assert_eq!(catalog.diff_to_latest("qa", 2).unwrap().1, "");
        assert_eq!(diff("a\nb\nc\n", "a\nb\nc\n", "x", "y"), "");
        assert_eq!(
            diff("a\nb\nc\n", "a\nB\nc\n", "x", "y"),
            "--- x\n+++ y\n@@ -1,3 +1,3 @@\n a\n-b\n+B\n c\n"
        );
    }

    #[test]
    fn prompts_name_each_missing_core_role_and_security_until_it_exists() {
        let all = prompts("fleet", &[]);
        let names: Vec<_> = all.iter().map(|p| p["role"].as_str().unwrap()).collect();
        assert_eq!(
            names,
            ["prime", "pm", "pa", "coder", "reviewer", "security"]
        );
        assert_eq!(
            all[1]["command"],
            "pij directory add pm --from template:pm --why 'core role from the pm template' --project fleet"
        );
        assert!(
            prompts("my fleet", &[])[0]["command"]
                .as_str()
                .unwrap()
                .ends_with(" --project 'my fleet'")
        );
        let some = prompts(
            "fleet",
            &[
                ("pm-001".into(), Some("pm".into())),
                ("security".into(), None),
                ("docs-system".into(), None),
            ],
        );
        let names: Vec<_> = some.iter().map(|p| p["role"].as_str().unwrap()).collect();
        assert_eq!(names, ["prime", "pa", "coder", "reviewer"]);
        let from_template = prompts("fleet", &[("appsec".into(), Some("security".into()))]);
        assert!(from_template.iter().all(|p| p["role"] != "security"));
    }

    /// A version is never edited after it ships (review of #136, S136001
    /// added v2s rather than editing v1s): each shipped version's bytes are
    /// pinned. A new version adds a row here; an edited one fails.
    #[test]
    fn a_shipped_version_is_never_edited() {
        use sha2::{Digest, Sha256};
        const PINNED: [(&str, u32, &str); 28] = [
            (
                "coder",
                1,
                "03a067e96b789574b8237315bfb3ca9cf31107cc4c08483c44fcadd08e915073",
            ),
            (
                "coder",
                2,
                "9462fd31371f189597fbf41fc06baef239bfec61564dcf1eb683dc8f49f6d1d1",
            ),
            (
                "coder",
                3,
                "10c92c3b6a65130b5b4d610ee0a7ff8b187a59a395a4a263bb455f0755edc974",
            ),
            (
                "designer",
                1,
                "dbbe15ff6d4c955654b29d56ba84b4f3d19eb1377c8a3792a5b30493914881bd",
            ),
            (
                "devcontainer",
                1,
                "9327a7f0a9ef34bdd8bca7e38757c02a338e5ebcf869c10a357788f45e184664",
            ),
            (
                "docs-keeper",
                1,
                "8c60821e6710d273f69f67e87ad980b9b9906facc1f5564dbd38081f41dcba52",
            ),
            (
                "infra",
                1,
                "be21115ed2b5ca0c2b34514147e610259b92f775b859d5ecbb16506c8d797a52",
            ),
            (
                "note-taker",
                1,
                "1ee2157ae53179dc381cab2376469f9bfebc3d6c80880d3c3bc091f55469d908",
            ),
            (
                "pa",
                1,
                "6da2384b210340d288b1469e0ff6c936df5b1f84e99472919345e122c4ecccd8",
            ),
            (
                "pa",
                2,
                "e3209c7abd4d6de318313e17118f1699e827a9a36e7ee2c146ba40a580fbbe49",
            ),
            (
                "pa",
                3,
                "c3dab9977f055142adc6d33f292996d8d9f270fce2f9e1d334330978e69d7cae",
            ),
            (
                "pm",
                1,
                "ee809ccec2e700374a36e611abf111c3e26ab9ec0b6c4327156005ed80b14577",
            ),
            (
                "pm",
                2,
                "7c7acaa3ace20b02559696b75a76b6c0cff1ee693a44d9c3dfb41d8e8673551a",
            ),
            (
                "pm",
                3,
                "6ca65faa5bd77e9cd1f34dfe82fbfbe96cee7a8b1d76c570b20e08593d14d8f7",
            ),
            (
                "prime",
                1,
                "95f695f0ff4ea6d81ff3b1ec262dad76b2ac002c4d347db36b77bee455f918bb",
            ),
            (
                "prime",
                2,
                "f54fce81be1e3a0280bd7a6b1cd3cc74bbdfa9f2c9d18a78a9428de5b98dbb56",
            ),
            (
                "prime",
                3,
                "62139e8f77e92d5b32f16cde1865c9902a7616773b7b5a7a009760f3e9091cdc",
            ),
            (
                "qa",
                1,
                "812f4f2e2be864211a521d660aa398af95e7af9ac2b2a8a45ee8b91e48872587",
            ),
            (
                "release",
                1,
                "bd1a4a96d082e10dc63c1e4d86dc08af484643f1183b82a03029ff02f78bcd11",
            ),
            (
                "researcher",
                1,
                "6a2721eefb2d2ccd7673e59b3e2758c099eca384371ef9c1fab190817a235f5a",
            ),
            (
                "reviewer",
                1,
                "5f0681eb3acb6310e4ad9da63e59e51470bd348a662ef8546cd6f43d6270741e",
            ),
            (
                "reviewer",
                2,
                "68d3ef499cc0c9a1e875b59407beaf3e5cd9eab2723564778d75d02185b4d56c",
            ),
            (
                "reviewer",
                3,
                "84b10c2cc51c91dbe16e61ecbff6bc361bbb75dae6a667b714ebd775cc83b0be",
            ),
            (
                "security",
                1,
                "749c1b1a5cf649d21aa6812382ec9430f66a013e117454f1e8fe743c1b745536",
            ),
            (
                "security",
                2,
                "a1e7ac96297e3e90c18378256e474dcb9dfc021f8da88c05e1446279fc730991",
            ),
            (
                "security",
                3,
                "310761dcfd0a740085fb0ad30778f557cdd371c55c4d46c0b9cac2f63081a534",
            ),
            (
                "topic-expert",
                1,
                "47ab35387785701aa0e14600ff76b259aeccdc2d766f6faaa2d43433be909dfa",
            ),
            (
                "transcriber",
                1,
                "0a2797a4538a34d90a0fb7f38c39a3a0f808f03da6cc85c29fed48b0d48a86db",
            ),
        ];
        let catalog = Catalog::shipped();
        assert_eq!(
            catalog.all().len(),
            PINNED.len(),
            "pin every new version here"
        );
        for (name, version, pinned) in PINNED {
            let template = catalog
                .get(name, Some(version))
                .expect("a pinned version ships");
            let digest = Sha256::digest(&template.bytes);
            let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
            assert_eq!(
                hex, pinned,
                "{name}@v{version} was edited after it shipped; add a new version"
            );
        }
    }
}
