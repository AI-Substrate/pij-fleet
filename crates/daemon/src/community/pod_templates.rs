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
const SHIPPED: &[Shipped] = &[Shipped {
    name: "build",
    version: 1,
    files: &[pod_file!("build", "1", "pod-template.dd.json")],
}];

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

#[cfg(test)]
mod tests {
    use super::*;

    /// Plan 254 ac-0001: a version is never edited after it ships. A new
    /// version adds a row here; an edited one fails.
    #[test]
    fn a_shipped_pod_template_is_never_edited() {
        use sha2::{Digest, Sha256};
        const PINNED: [(&str, u32, &str); 1] = [(
            "build",
            1,
            "c88dc7c5ca69ea7e078b3867c4904f888a53edf568500573b13d95d289f150eb",
        )];
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
}
