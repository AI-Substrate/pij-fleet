//! The prime's operating picture (plan 203, roadmap 001 s09): what each role
//! is waiting on and what the prime might do next. Pure: the view gathers the
//! store rows, the live seats and the documents, and these functions shape
//! them. Nothing here reads, writes or launches.

use std::collections::HashSet;
use std::path::Path;

use pij_core::directory::shell_word;
use pij_store::{Assignment, CommunityOutput, RoleRequest};
use serde_json::{Value, json};

use super::templates::Catalog;

/// How many of a role's most recent closes may pass without a memory note
/// before the view prompts for one.
pub const MEMORY_CLOSES: usize = 2;

/// Where a project keeps its roadmaps, relative to its repo (s13's layout).
pub const ROADMAPS: &str = "docs/project/roadmaps";

/// The standing prompt to notice a need no role covers (PEC, C26).
pub const NOTICE: &str = "Is the work asking for something no role here covers? Name the need and give it a role, from a template when one fits or as a new definition: pij directory add <id> --why \"<the need>\". The community is whatever the work needs now.";

/// `role`'s queued requests: `{count, oldest_at}`, or `null` when none waits.
pub fn waiting(role: &str, queued: &[RoleRequest]) -> Value {
    let mine: Vec<&RoleRequest> = queued.iter().filter(|r| r.role == role).collect();
    match mine.iter().map(|r| r.created_at).min() {
        Some(oldest) => json!({"count": mine.len(), "oldest_at": oldest}),
        None => Value::Null,
    }
}

/// `role`'s newest output, or `null`.
pub fn latest_output(role: &str, outputs: &[CommunityOutput]) -> Value {
    outputs
        .iter()
        .find(|o| o.role == role)
        .map_or(Value::Null, |o| {
            json!({
                "kind": o.kind, "subject": o.subject, "head": o.head, "result": o.result,
                "writer": o.writer, "at": o.created_at, "path": o.path,
            })
        })
}

/// The next actions for one role (req-12): no live holder while requests
/// wait; two live holders on a one-holder role; its last [`MEMORY_CLOSES`]
/// closes left no memory note. `rows` are the role's holdings, `live` the
/// registered, untombstoned seats, `memory_latest` its newest note's date.
pub fn actions(
    project: &str,
    role: &str,
    holders: &str,
    rows: &[&Assignment],
    live: &HashSet<String>,
    waiting: usize,
    memory_latest: Option<&str>,
) -> Vec<Value> {
    let p = shell_word(project);
    let r = shell_word(role);
    let mut out = Vec::new();
    let mut holding: Vec<&Assignment> = rows
        .iter()
        .copied()
        .filter(|row| row.to.is_none() && live.contains(&row.seat))
        .collect();
    holding.sort_by(|a, b| a.from.cmp(&b.from));

    if holding.is_empty() && waiting > 0 {
        let mut commands = Vec::new();
        let last = rows
            .iter()
            .max_by(|a, b| {
                a.to.as_deref()
                    .unwrap_or(&a.from)
                    .cmp(b.to.as_deref().unwrap_or(&b.from))
            })
            .map(|row| row.seat.as_str());
        if let Some(last) = last {
            commands.push(format!("pij-rs revive {}", shell_word(last)));
        }
        commands.push(format!(
            "pij directory assign {r} <seat> --why \"<why>\" --project {p}"
        ));
        out.push(json!({
            "role": role,
            "condition": "no-live-holder",
            "says": format!(
                "{role} has no live holder and {waiting} request(s) waiting; revive its last holder or assign a seat (assigning delivers the queue)"
            ),
            "commands": commands,
        }));
    }

    if holders == "one" && holding.len() > 1 {
        let seats: Vec<&str> = holding.iter().map(|row| row.seat.as_str()).collect();
        let newest = holding.len() - 1;
        out.push(json!({
            "role": role,
            "condition": "two-live-holders",
            "says": format!(
                "{role} is a one-holder role with {} live holders ({}); release all but one (newest kept here)",
                seats.len(),
                seats.join(", ")
            ),
            "commands": holding[..newest].iter().map(|row| format!(
                "pij directory release {r} {} --why \"a one-holder role had {} live holders\" --project {p}",
                shell_word(&row.seat),
                seats.len()
            )).collect::<Vec<_>>(),
        }));
    }

    let mut closes: Vec<&str> = rows
        .iter()
        .filter(|row| row.release_why.as_deref() == Some("close"))
        .filter_map(|row| row.to.as_deref())
        .collect();
    closes.sort_unstable();
    let recent = &closes[closes.len().saturating_sub(MEMORY_CLOSES)..];
    let after_memory =
        |to: &&str| memory_latest.is_none_or(|date| to.get(..10).is_some_and(|day| day > date));
    if recent.len() == MEMORY_CLOSES && recent.iter().all(after_memory) {
        out.push(json!({
            "role": role,
            "condition": "memory-stale",
            "says": format!(
                "{role}'s last {MEMORY_CLOSES} closes left no memory note (newest note: {}); ask a holder, or write what you know",
                memory_latest.unwrap_or("none")
            ),
            "commands": [format!(
                "pij directory memory add {r} --title \"<what was learned>\" --hook \"<when to read it>\" --body-file - --project {p}"
            )],
        }));
    }
    out
}

/// Each role a not-complete roadmap step needs (`needs: ["role:<id>"]`), with
/// the steps that need it, `<roadmap> <step>`. Reads the project repo's
/// roadmap documents; an unreadable one is skipped.
pub fn roadmap_needs(repo: &Path) -> Vec<(String, Vec<String>)> {
    let mut files: Vec<_> = std::fs::read_dir(repo.join(ROADMAPS))
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.to_str().is_some_and(|p| p.ends_with(".dd.json")))
        .collect();
    files.sort();
    let mut needs: Vec<(String, Vec<String>)> = Vec::new();
    for file in files {
        let Some(document) = std::fs::read(&file)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        else {
            continue;
        };
        let name = file
            .file_name()
            .and_then(|n| n.to_str())
            .map_or("", |n| n.trim_end_matches(".dd.json"))
            .to_string();
        let steps = document["sections"]
            .as_array()
            .and_then(|sections| sections.iter().find(|s| s["name"] == "steps"))
            .and_then(|s| s["value"].as_array())
            .cloned()
            .unwrap_or_default();
        for step in steps.iter().filter(|step| step["state"] != "complete") {
            let label = format!("{name} {}", step["id"].as_str().unwrap_or("?"));
            for role in step["needs"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|need| need.as_str()?.strip_prefix("role:"))
            {
                match needs.iter_mut().find(|(id, _)| id == role) {
                    Some((_, steps)) => steps.push(label.clone()),
                    None => needs.push((role.to_string(), vec![label.clone()])),
                }
            }
        }
    }
    needs
}

/// Fold the roadmap needs into the missing-role `prompts` (req-24, req-27):
/// a needed role already prompted gains `needed_by`; any other needed role
/// that does not exist gets a prompt of kind `needed`, from its template when
/// one ships, else as a new definition to fill in.
pub fn fold_needs(
    project: &str,
    prompts: &mut Vec<Value>,
    needs: &[(String, Vec<String>)],
    roles: &[(String, Option<String>)],
    catalog: &Catalog,
) {
    let p = shell_word(project);
    for (role, steps) in needs {
        let exists = roles
            .iter()
            .any(|(id, template)| id == role || template.as_deref() == Some(role.as_str()));
        if exists {
            continue;
        }
        if let Some(prompt) = prompts.iter_mut().find(|prompt| prompt["role"] == *role) {
            prompt["needed_by"] = json!(steps);
            continue;
        }
        let r = shell_word(role);
        let why = format!("\"{} needs role:{role}\"", steps.join(", "));
        let command = if catalog.latest(role).is_some() {
            format!("pij directory add {r} --from template:{r} --why {why} --project {p}")
        } else {
            format!(
                "pij directory add {r} --title \"<title>\" --kind general --lifecycle long-lived --holders one --rule \"<what it always does>\" --why {why} --project {p}"
            )
        };
        prompts.push(json!({
            "role": role,
            "kind": "needed",
            "needed_by": steps,
            "command": command,
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(seat: &str, from: &str, to: Option<&str>, why: Option<&str>) -> Assignment {
        Assignment {
            project: "fleet".into(),
            role: "reviewer".into(),
            instance: String::new(),
            seat: seat.into(),
            harness: None,
            session: None,
            machine: None,
            from: from.into(),
            to: to.map(Into::into),
            assigned_by: "prime".into(),
            released_by: None,
            why: "w".into(),
            release_why: why.map(Into::into),
            end_pending: false,
        }
    }

    fn live(seats: &[&str]) -> HashSet<String> {
        seats.iter().map(|s| (*s).to_string()).collect()
    }

    fn conditions(actions: &[Value]) -> Vec<&str> {
        actions
            .iter()
            .map(|a| a["condition"].as_str().unwrap())
            .collect()
    }

    #[test]
    fn no_live_holder_with_requests_waiting_names_revive_and_assign() {
        let old = row(
            "pij-old",
            "2026-11-01T00:00:00Z",
            Some("2026-11-02T00:00:00Z"),
            Some("release"),
        );
        let dead = row("pij-dead", "2026-11-03T00:00:00Z", None, None);
        let rows = [&old, &dead];
        let acts = actions("fleet", "reviewer", "one", &rows, &live(&[]), 2, None);
        assert_eq!(conditions(&acts), ["no-live-holder"]);
        assert_eq!(
            acts[0]["commands"],
            json!([
                "pij-rs revive pij-dead",
                "pij directory assign reviewer <seat> --why \"<why>\" --project fleet"
            ])
        );
        assert!(actions("fleet", "reviewer", "one", &rows, &live(&[]), 0, None).is_empty());
        assert!(
            actions(
                "fleet",
                "reviewer",
                "one",
                &rows,
                &live(&["pij-dead"]),
                2,
                None
            )
            .is_empty()
        );
    }

    #[test]
    fn two_live_holders_on_a_one_holder_role_release_all_but_the_newest() {
        let a = row("pij-a", "2026-11-01T00:00:00Z", None, None);
        let b = row("pij-b", "2026-11-02T00:00:00Z", None, None);
        let rows = [&b, &a];
        let acts = actions(
            "fleet",
            "reviewer",
            "one",
            &rows,
            &live(&["pij-a", "pij-b"]),
            0,
            None,
        );
        assert_eq!(conditions(&acts), ["two-live-holders"]);
        assert_eq!(
            acts[0]["commands"],
            json!([
                "pij directory release reviewer pij-a --why \"a one-holder role had 2 live holders\" --project fleet"
            ])
        );
        assert!(
            actions(
                "fleet",
                "reviewer",
                "many",
                &rows,
                &live(&["pij-a", "pij-b"]),
                0,
                None
            )
            .is_empty()
        );
        assert!(
            actions(
                "fleet",
                "reviewer",
                "one",
                &rows,
                &live(&["pij-a"]),
                0,
                None
            )
            .is_empty()
        );
    }

    #[test]
    fn memory_is_stale_only_when_the_last_two_closes_postdate_the_newest_note() {
        let c1 = row(
            "pij-1",
            "2026-11-01T00:00:00Z",
            Some("2026-11-03T09:00:00Z"),
            Some("close"),
        );
        let c2 = row(
            "pij-2",
            "2026-11-03T10:00:00Z",
            Some("2026-11-05T09:00:00Z"),
            Some("close"),
        );
        let rows = [&c1, &c2];
        let stale = |latest| {
            conditions(&actions(
                "fleet",
                "reviewer",
                "one",
                &rows,
                &live(&[]),
                0,
                latest,
            ))
            .contains(&"memory-stale")
        };
        assert!(stale(None));
        assert!(stale(Some("2026-11-02")));
        assert!(
            !stale(Some("2026-11-03")),
            "a note on a close's day covers it"
        );
        assert!(
            !actions("fleet", "reviewer", "one", &rows[..1], &live(&[]), 0, None)
                .iter()
                .any(|a| a["condition"] == "memory-stale"),
            "one close is not a pattern"
        );
        let acts = actions("fleet", "reviewer", "one", &rows, &live(&[]), 0, None);
        assert!(
            acts[0]["commands"][0]
                .as_str()
                .unwrap()
                .starts_with("pij directory memory add reviewer ")
        );
    }

    #[test]
    fn a_needed_role_is_prompted_from_its_template_or_as_a_new_definition() {
        let catalog = Catalog::shipped();
        let needs = vec![
            ("security".to_string(), vec!["001 s09".to_string()]),
            (
                "docs-system".to_string(),
                vec!["001 s09".to_string(), "001 s10".to_string()],
            ),
            ("coder".to_string(), vec!["001 s10".to_string()]),
        ];
        let mut prompts = vec![json!({"role": "security", "kind": "recommended", "command": "x"})];
        let roles = vec![("coder".to_string(), None)];
        fold_needs("fleet", &mut prompts, &needs, &roles, &catalog);
        assert_eq!(prompts[0]["needed_by"], json!(["001 s09"]));
        assert_eq!(
            prompts.len(),
            2,
            "coder exists; security was already prompted"
        );
        assert_eq!(prompts[1]["role"], "docs-system");
        assert_eq!(prompts[1]["kind"], "needed");
        let command = prompts[1]["command"].as_str().unwrap();
        let expected_template = catalog.latest("docs-system").is_some();
        assert_eq!(
            command.contains("--from template:docs-system"),
            expected_template,
            "{command}"
        );
        assert!(
            command.contains("--why \"001 s09, 001 s10 needs role:docs-system\""),
            "{command}"
        );
        assert!(command.ends_with("--project fleet"), "{command}");
    }

    #[test]
    fn roadmap_needs_skip_complete_steps() {
        let dir = pij_testkit::fresh_dir("picture-needs");
        let roadmaps = dir.join(ROADMAPS);
        std::fs::create_dir_all(&roadmaps).unwrap();
        std::fs::write(
            roadmaps.join("001-x.dd.json"),
            json!({"sections": [{"name": "steps", "value": [
                {"id": "s01", "state": "complete", "needs": ["role:old"]},
                {"id": "s02", "state": "planned", "needs": ["role:qa", "role:docs"]},
                {"id": "s03", "needs": ["role:qa"]}
            ]}]})
            .to_string(),
        )
        .unwrap();
        std::fs::write(roadmaps.join("broken.dd.json"), "{").unwrap();
        assert_eq!(
            roadmap_needs(&dir),
            vec![
                (
                    "qa".to_string(),
                    vec!["001-x s02".to_string(), "001-x s03".to_string()]
                ),
                ("docs".to_string(), vec!["001-x s02".to_string()]),
            ]
        );
    }
}
