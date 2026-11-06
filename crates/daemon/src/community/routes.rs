//! The routing table (plan 202, roadmap 001 s08): "send it, don't do it".
//!
//! Built from every role's scope, one row per `scope.paths` entry and one per
//! `scope.rules` entry, each sent to `role:<id>`. `community/ROUTES.md` holds
//! the rows and any overlaps but no holder names, so a change of holder never
//! changes the file; the copy a seat is sent adds the live holders.
//!
//! Pure apart from reading role documents in-process: no launches.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::Value;

use super::layout::{COMMUNITY, ROUTES};
use super::writer::{Existing, NewFile};

/// Two roles claim overlapping paths.
pub const W_OVERLAP: &str = "W-ROUTES-OVERLAP";

/// What a row routes: a path glob or a rule in words.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// A `scope.paths` entry.
    Path,
    /// A `scope.rules` entry.
    Rule,
}

/// One row: work matching `what` goes to `role:<role>`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct Route {
    /// The role id.
    pub role: String,
    /// Path or rule.
    pub kind: Kind,
    /// The path glob or rule text.
    pub what: String,
}

/// Two roles whose path claims overlap.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Overlap {
    /// The first role, by id.
    pub role: String,
    /// Its path.
    pub path: String,
    /// The other role.
    pub other_role: String,
    /// The other role's path.
    pub other_path: String,
}

impl Overlap {
    /// The named warning.
    pub fn warning(&self) -> String {
        format!(
            "{W_OVERLAP}: {} (role:{}) overlaps {} (role:{})",
            self.path, self.role, self.other_path, self.other_role
        )
    }
}

/// The routing table.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Table {
    /// Every row, by role id, paths before rules, in document order.
    pub routes: Vec<Route>,
    /// Every pair of overlapping path claims from different roles.
    pub overlaps: Vec<Overlap>,
}

/// The rows one table has that another does not.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Change {
    /// Rows only in the new table.
    pub added: Vec<Route>,
    /// Rows only in the old table.
    pub removed: Vec<Route>,
}

impl Change {
    /// Whether no row changed.
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty()
    }
}

/// What a directory write does to the table.
#[derive(Clone, Debug, Default)]
pub struct Regenerated {
    /// `ROUTES.md`, when its bytes change.
    pub file: Option<NewFile>,
    /// The rows the write changes.
    pub change: Change,
    /// The table after the write.
    pub table: Table,
}

/// Build the table from `(id, role document)` pairs.
pub fn build(roles: &[(String, Value)]) -> Table {
    let mut routes: Vec<Route> = Vec::new();
    for (id, document) in roles {
        let scope = section(document, "scope");
        for (kind, key) in [(Kind::Path, "paths"), (Kind::Rule, "rules")] {
            let entries = scope
                .and_then(|scope| scope[key].as_array())
                .map(Vec::as_slice)
                .unwrap_or_default();
            for what in entries.iter().filter_map(Value::as_str) {
                let what = what.trim();
                let route = Route {
                    role: id.clone(),
                    kind,
                    what: what.to_string(),
                };
                if !what.is_empty() && !routes.contains(&route) {
                    routes.push(route);
                }
            }
        }
    }
    // Stable: document order within a role and kind.
    routes.sort_by(|a, b| a.role.cmp(&b.role).then(a.kind.cmp(&b.kind)));
    let paths: Vec<&Route> = routes.iter().filter(|r| r.kind == Kind::Path).collect();
    let mut overlaps = Vec::new();
    for (index, first) in paths.iter().enumerate() {
        for second in &paths[index + 1..] {
            if first.role != second.role && overlap(&first.what, &second.what) {
                overlaps.push(Overlap {
                    role: first.role.clone(),
                    path: first.what.clone(),
                    other_role: second.role.clone(),
                    other_path: second.what.clone(),
                });
            }
        }
    }
    Table { routes, overlaps }
}

/// One component of a path claim: literal, or the literal text before the
/// first glob character (the claim stops there).
enum Part<'a> {
    Exact(&'a str),
    Glob(&'a str),
}

fn parts(path: &str) -> Vec<Part<'_>> {
    let mut parts = Vec::new();
    for component in path
        .trim()
        .trim_start_matches("./")
        .split('/')
        .filter(|component| !component.is_empty() && *component != ".")
    {
        match component.find(['*', '?', '[', '{']) {
            Some(at) => {
                parts.push(Part::Glob(&component[..at]));
                break;
            }
            None => parts.push(Part::Exact(component)),
        }
    }
    parts
}

/// Whether two path claims can name one file: their literal components agree
/// up to where either ends or turns into a glob. A claim on a directory covers
/// everything under it.
pub fn overlap(a: &str, b: &str) -> bool {
    for pair in parts(a).iter().zip(parts(b).iter()) {
        match pair {
            (Part::Exact(x), Part::Exact(y)) => {
                if x != y {
                    return false;
                }
            }
            (Part::Glob(prefix), Part::Exact(name)) | (Part::Exact(name), Part::Glob(prefix)) => {
                return name.starts_with(prefix);
            }
            (Part::Glob(x), Part::Glob(y)) => return x.starts_with(y) || y.starts_with(x),
        }
    }
    true
}

/// The rows `new` adds to and removes from `old`.
pub fn diff(old: &Table, new: &Table) -> Change {
    Change {
        added: new
            .routes
            .iter()
            .filter(|route| !old.routes.contains(route))
            .cloned()
            .collect(),
        removed: old
            .routes
            .iter()
            .filter(|route| !new.routes.contains(route))
            .cloned()
            .collect(),
    }
}

/// `community/ROUTES.md`.
pub fn markdown(table: &Table) -> String {
    let mut out = String::from(
        "# Routing: send it, don't do it\n\n\
         Generated by pij from every role's scope. Do not edit it by hand: change a \
         role's scope and pij rewrites it. `pij directory routes` shows it with the \
         live holders.\n\n",
    );
    if table.routes.is_empty() {
        out.push_str("No role has a scope yet.\n");
    } else {
        out.push_str("| What | Send to |\n|---|---|\n");
        for route in &table.routes {
            out.push_str(&format!("| {} | role:{} |\n", what(route), route.role));
        }
    }
    if !table.overlaps.is_empty() {
        out.push_str("\n## Overlaps\n\n");
        for overlap in &table.overlaps {
            out.push_str(&format!(
                "- {} (role:{}) overlaps {} (role:{})\n",
                code(&overlap.path),
                overlap.role,
                code(&overlap.other_path),
                overlap.other_role
            ));
        }
    }
    out
}

/// The table as a seat is sent it: each row with the role's live holders
/// (`holders`, by role id), and any overlaps.
pub fn for_seat(project: &str, table: &Table, holders: &BTreeMap<String, Vec<String>>) -> String {
    let mut out = format!(
        "Routing table for {project}. Owned work: send it, don't do it. \
         `pij directory routes` shows it again.\n\n"
    );
    if table.routes.is_empty() {
        out.push_str("No role has a scope yet.\n");
    } else {
        out.push_str("| What | Send to | Holder now |\n|---|---|---|\n");
        for route in &table.routes {
            let now = holders
                .get(&route.role)
                .filter(|seats| !seats.is_empty())
                .map_or_else(|| "none live".to_string(), |seats| seats.join(", "));
            out.push_str(&format!(
                "| {} | role:{} | {} |\n",
                what(route),
                route.role,
                cell(&now)
            ));
        }
    }
    for overlap in &table.overlaps {
        out.push_str(&format!("\n{}", overlap.warning()));
    }
    out
}

/// The FYI the prime gets for a change.
pub fn change_notice(project: &str, commit: &str, change: &Change) -> String {
    let short = commit.get(..7).unwrap_or(commit);
    let mut out = format!("Routes changed in {project} (directory commit {short}):");
    for (sign, routes) in [("+", &change.added), ("-", &change.removed)] {
        for route in routes {
            out.push_str(&format!("\n{sign} {} -> role:{}", what(route), route.role));
        }
    }
    out.push_str(&format!(
        "\nFull table: pij directory routes --project {project}"
    ));
    out
}

/// Every role document under `<worktree>/community/roles/`, by id, read
/// in-process. A document that is not JSON contributes no rows.
pub fn read_roles(worktree: &Path) -> Vec<(String, Value)> {
    let dir = worktree.join(COMMUNITY).join("roles");
    let mut roles: Vec<(String, Value)> = std::fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let id = entry.file_name().into_string().ok()?;
            let bytes = std::fs::read(entry.path().join("role.dd.json")).ok()?;
            Some((id, serde_json::from_slice(&bytes).unwrap_or(Value::Null)))
        })
        .collect();
    roles.sort_by(|a, b| a.0.cmp(&b.0));
    roles
}

/// The table after `files` land on the roles in `worktree`, and `ROUTES.md`
/// when its bytes change. Reads only; the caller commits the file with its
/// own write, so a regeneration is never a commit of its own.
pub fn regenerate(worktree: &Path, files: &[NewFile]) -> Regenerated {
    let before = read_roles(worktree);
    let mut after = before.clone();
    for file in files {
        let Some(id) = role_of(&file.path) else {
            continue;
        };
        let document = serde_json::from_slice(&file.bytes).unwrap_or(Value::Null);
        match after.iter_mut().find(|(known, _)| *known == id) {
            Some(slot) => slot.1 = document,
            None => after.push((id, document)),
        }
    }
    after.sort_by(|a, b| a.0.cmp(&b.0));
    let (old, table) = (build(&before), build(&after));
    let bytes = markdown(&table).into_bytes();
    let file = (std::fs::read(worktree.join(ROUTES)).ok().as_deref() != Some(bytes.as_slice()))
        .then(|| NewFile {
            path: PathBuf::from(ROUTES),
            bytes,
            document: false,
            existing: Existing::Replace,
        });
    Regenerated {
        file,
        change: diff(&old, &table),
        table,
    }
}

/// The role id when `path` is `community/roles/<id>/role.dd.json`.
fn role_of(path: &Path) -> Option<String> {
    let rest = path.strip_prefix(Path::new(COMMUNITY).join("roles")).ok()?;
    let mut parts = rest.components();
    let id = parts.next()?.as_os_str().to_str()?.to_string();
    (parts.as_path() == Path::new("role.dd.json")).then_some(id)
}

fn what(route: &Route) -> String {
    match route.kind {
        Kind::Path => code(&route.what),
        Kind::Rule => cell(&route.what),
    }
}

fn code(path: &str) -> String {
    format!("`{}`", cell(&path.replace('`', "'")))
}

/// A Markdown table cell: one line, pipes escaped.
fn cell(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace('|', "\\|")
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
    use serde_json::json;

    fn role(paths: &[&str], rules: &[&str]) -> Value {
        json!({"dd": {"schema": "pij/role"}, "sections": [
            {"name": "meta", "value": {"id": "x"}},
            {"name": "scope", "value": {"paths": paths, "rules": rules}},
        ]})
    }

    fn roles(list: &[(&str, Value)]) -> Vec<(String, Value)> {
        list.iter()
            .map(|(id, doc)| ((*id).to_string(), doc.clone()))
            .collect()
    }

    #[test]
    fn rows_come_from_paths_then_rules_by_role() {
        let table = build(&roles(&[
            ("security", role(&[], &["A verdict on every PR."])),
            (
                "delivery",
                role(&["crates/daemon/src/delivery/**"], &["Owns sends."]),
            ),
        ]));
        let rows: Vec<(&str, Kind, &str)> = table
            .routes
            .iter()
            .map(|r| (r.role.as_str(), r.kind, r.what.as_str()))
            .collect();
        assert_eq!(
            rows,
            [
                ("delivery", Kind::Path, "crates/daemon/src/delivery/**"),
                ("delivery", Kind::Rule, "Owns sends."),
                ("security", Kind::Rule, "A verdict on every PR."),
            ]
        );
        assert!(table.overlaps.is_empty());
        let roles_without_scope = build(&roles(&[("bare", json!({"sections": []}))]));
        assert!(roles_without_scope.routes.is_empty());
    }

    #[test]
    fn overlapping_claims_from_two_roles_are_named() {
        for (a, b) in [
            ("crates/x/**", "crates/x/y.rs"),
            ("crates/x", "crates/x/y/z.rs"),
            ("docs/project/", "./docs/project/prds/*.json"),
            ("Dockerfile*", "Dockerfile.dev"),
            ("crates/**", "crates/daemon/src/delivery/**"),
            ("a/b", "a/b"),
        ] {
            assert!(overlap(a, b), "{a} vs {b}");
            assert!(overlap(b, a), "{b} vs {a}");
        }
        for (a, b) in [
            ("crates/x/**", "crates/y/**"),
            ("Dockerfile*", "docs/x"),
            ("docs/a.md", "docs/b.md"),
            ("src/foo*", "src/bar*"),
        ] {
            assert!(!overlap(a, b), "{a} vs {b}");
        }
        let table = build(&roles(&[
            ("a", role(&["crates/x/**"], &[])),
            ("b", role(&["crates/x/y.rs", "docs/**"], &[])),
            ("c", role(&["crates/y/**"], &[])),
        ]));
        assert_eq!(table.overlaps.len(), 1);
        assert_eq!(
            table.overlaps[0].warning(),
            "W-ROUTES-OVERLAP: crates/x/** (role:a) overlaps crates/x/y.rs (role:b)"
        );
        assert!(markdown(&table).contains("## Overlaps\n\n- `crates/x/**` (role:a) overlaps"));
        // One role claiming two overlapping paths is no conflict.
        assert!(
            build(&roles(&[("a", role(&["x/**", "x/y"], &[]))]))
                .overlaps
                .is_empty()
        );
    }

    #[test]
    fn cells_stay_one_line_with_pipes_escaped() {
        let table = build(&roles(&[("pm", role(&[], &["a | b\nnext  line"]))]));
        assert!(
            markdown(&table).contains("| a \\| b next line | role:pm |\n"),
            "{}",
            markdown(&table)
        );
    }

    #[test]
    fn a_rule_edit_is_one_removed_and_one_added_row() {
        let old = build(&roles(&[("pm", role(&["docs/**"], &["old"]))]));
        let new = build(&roles(&[("pm", role(&["docs/**"], &["new"]))]));
        let change = diff(&old, &new);
        assert_eq!(change.added.len(), 1);
        assert_eq!(change.added[0].what, "new");
        assert_eq!(change.removed.len(), 1);
        assert_eq!(change.removed[0].what, "old");
        assert!(diff(&new, &new).is_empty());
        let notice = change_notice("fleet", "0123456789abcdef", &change);
        assert_eq!(
            notice,
            "Routes changed in fleet (directory commit 0123456):\n+ new -> role:pm\n- old -> role:pm\nFull table: pij directory routes --project fleet"
        );
    }

    #[test]
    fn holders_reach_the_seat_copy_but_never_the_file() {
        let table = build(&roles(&[
            ("pm", role(&[], &["packets"])),
            ("qa", role(&[], &["tests"])),
        ]));
        let file = markdown(&table);
        assert!(!file.contains("pij-"));
        let mut holders = BTreeMap::new();
        holders.insert("pm".to_string(), vec!["pij-tidy-wren".to_string()]);
        let text = for_seat("fleet", &table, &holders);
        assert!(
            text.contains("| packets | role:pm | pij-tidy-wren |"),
            "{text}"
        );
        assert!(text.contains("| tests | role:qa | none live |"), "{text}");
    }

    #[test]
    fn regenerating_overlays_written_roles_and_skips_an_unchanged_file() {
        let dir = pij_testkit::fresh_dir("pij-routes-regenerate");
        let pm = dir.join(COMMUNITY).join("roles/pm");
        std::fs::create_dir_all(&pm).unwrap();
        std::fs::write(pm.join("role.dd.json"), role(&[], &["packets"]).to_string()).unwrap();
        let qa = NewFile {
            path: Path::new(COMMUNITY).join("roles/qa/role.dd.json"),
            bytes: role(&["tests/**"], &[]).to_string().into_bytes(),
            document: true,
            existing: Existing::Replace,
        };
        let memory = NewFile {
            path: Path::new(COMMUNITY).join("roles/qa/memory/INDEX.md"),
            bytes: b"# qa memory\n".to_vec(),
            document: false,
            existing: Existing::Replace,
        };
        let regenerated = regenerate(&dir, &[qa, memory]);
        let file = regenerated.file.expect("ROUTES.md is new");
        assert_eq!(file.path, Path::new(ROUTES));
        assert_eq!(regenerated.change.added.len(), 1);
        assert_eq!(regenerated.change.added[0].role, "qa");
        assert!(regenerated.change.removed.is_empty());
        std::fs::write(dir.join(ROUTES), &file.bytes).unwrap();
        // Written as it stands: nothing to regenerate.
        let qa = dir.join(COMMUNITY).join("roles/qa");
        std::fs::create_dir_all(&qa).unwrap();
        std::fs::write(
            qa.join("role.dd.json"),
            role(&["tests/**"], &[]).to_string(),
        )
        .unwrap();
        let again = regenerate(&dir, &[]);
        assert!(again.file.is_none());
        assert!(again.change.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
