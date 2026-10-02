use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use pij_cli::{DaemonClient, IdentityRequest};
use serde_json::Value;

use pij_core::model::SeatDescriptor;

pub(crate) fn fail(error: impl std::fmt::Display) -> ExitCode {
    eprintln!(
        "pij commit-trailers: {error}; explain any expected omitted trailers in the commit body"
    );
    ExitCode::FAILURE
}

pub(crate) async fn run(client: &DaemonClient, request: &IdentityRequest) -> ExitCode {
    let response = client.whoami(request).await;
    if !response.ok {
        return fail(
            response
                .meta
                .as_deref()
                .unwrap_or("Pij-Seat and Pij-Prime omitted: could not resolve the current seat"),
        );
    }
    let Some(seat) = response.data else {
        return fail("Pij-Seat and Pij-Prime omitted: whoami returned no seat");
    };
    let response = client.local_seats().await;
    let mut warnings = Vec::new();
    let rows = if response.ok {
        response.data.map(|roster| roster.seats).unwrap_or_else(|| {
            warnings.push(
                "local registry returned no rows; ancestor identities are unavailable".to_string(),
            );
            Vec::new()
        })
    } else {
        warnings.push(format!(
            "local registry unavailable: {}",
            response.meta.as_deref().unwrap_or("no reason given")
        ));
        Vec::new()
    };
    let plan = match std::env::current_dir() {
        Ok(cwd) => working_plan(&cwd, &mut warnings),
        Err(error) => {
            warnings.push(format!(
                "Pij-Plan omitted: cannot read the working directory: {error}"
            ));
            None
        }
    };
    // Rust exposes neither a prime-designation getter nor a current-assignment
    // getter yet. Never substitute stale legacy registry snapshots for either.
    let mut trailers = derive(&seat, &rows, None, plan.as_deref());
    trailers.warnings.extend(warnings);
    print!("{}", trailers.text);
    if let Some(tier) = trailers.prime_tier {
        eprintln!("# Pij-Prime derivation: {tier}");
    }
    for warning in trailers.warnings {
        eprintln!("pij commit-trailers: {warning}");
    }
    ExitCode::SUCCESS
}

fn git(cwd: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .current_dir(cwd)
        .args(args)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8(output.stdout).ok())?
        .map(|text| text.trim().to_string())
}

fn path_plan(path: &Path) -> Option<&str> {
    path.file_name()
        .and_then(|name| name.to_str())
        .and_then(plan_name)
}

fn working_plan(cwd: &Path, warnings: &mut Vec<String>) -> Option<String> {
    // Git resolves symlinks (notably macOS /var -> /private/var); compare like paths.
    let canonical = cwd.canonicalize().ok();
    let cwd = canonical.as_deref().unwrap_or(cwd);
    let root = git(cwd, &["rev-parse", "--show-toplevel"]).map(PathBuf::from);
    let root = root.as_deref().unwrap_or(cwd);
    // The current tree wins over its branch, which wins over flow context.
    for directory in cwd.ancestors().take_while(|path| path.starts_with(root)) {
        if let Some(plan) = path_plan(directory) {
            return Some(plan.to_string());
        }
    }
    if let Some(branch) = git(cwd, &["symbolic-ref", "--quiet", "--short", "HEAD"])
        && let Some(plan) = path_plan(Path::new(&branch))
    {
        return Some(plan.to_string());
    }
    for directory in cwd.ancestors().take_while(|path| path.starts_with(root)) {
        for name in ["the-flow.json", ".the-flow-state.json"] {
            let path = directory.join(name);
            let text = match std::fs::read_to_string(&path) {
                Ok(text) => text,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => {
                    warnings.push(format!(
                        "Pij-Plan unresolved from {}: {error}",
                        path.display()
                    ));
                    continue;
                }
            };
            match serde_json::from_str::<Value>(&text) {
                Ok(flow) => {
                    if let Some(plan) = flow_plan(&flow, warnings) {
                        return Some(plan);
                    }
                }
                Err(error) => warnings.push(format!(
                    "Pij-Plan unresolved from {}: {error}",
                    path.display()
                )),
            }
        }
    }
    None
}

fn flow_plan(flow: &Value, warnings: &mut Vec<String>) -> Option<String> {
    for pointer in ["/nav/bag/plan_dir", "/plan_dir"] {
        if let Some(value) = flow.pointer(pointer) {
            if value.is_null() {
                continue;
            }
            if let Some(plan) = value.as_str().and_then(|value| path_plan(Path::new(value))) {
                return Some(plan.to_string());
            }
            warnings.push(format!("Pij-Plan omitted: flow {pointer} is not an ordinal or NNN-slug plan path; explain this omission in the commit body"));
        }
    }
    None
}

#[derive(Default)]
struct Trailers {
    text: String,
    warnings: Vec<String>,
    prime_tier: Option<&'static str>,
}

impl Trailers {
    fn add(&mut self, key: &str, value: &str) {
        if value.is_empty() || value.chars().any(char::is_whitespace) {
            self.warnings.push(format!(
                "{key} omitted: the recorded value is empty or contains whitespace"
            ));
        } else {
            writeln!(self.text, "{key}: {value}").expect("write to String");
        }
    }
}

fn derive(
    seat: &SeatDescriptor,
    registry: &[SeatDescriptor],
    designated: Option<&str>,
    plan: Option<&str>,
) -> Trailers {
    let mut result = Trailers::default();
    // Tier 1: the nearest explicit prime, including the committer itself.
    let mut prime = (seat.role.as_deref() == Some("prime")).then_some(seat.id.as_str());
    let mut root = None;
    let mut current = seat;
    let mut visited = BTreeSet::from([&seat.id]);
    while prime.is_none() {
        let Some(id) = current.parent.as_ref() else {
            root = Some(current.id.as_str());
            break;
        };
        if !visited.insert(id) {
            result.warnings.push(format!(
                "ancestor cycle at {id}; using the repository designation if available"
            ));
            break;
        }
        let Some(ancestor) = registry.iter().find(|row| &row.id == id) else {
            result.warnings.push(format!("ancestor {id} is absent from the local registry; using the repository designation if available"));
            break;
        };
        if ancestor.role.as_deref() == Some("prime") {
            prime = Some(ancestor.id.as_str());
        }
        current = ancestor;
    }
    // Tier 2: an authoritative repository designation, when available.
    // Tier 3: the recorded-parent root (interim prime ruling, 2026-09-07).
    let resolved = prime
        .map(|id| (id, "role=prime"))
        .or_else(|| designated.map(|id| (id, "repository designation")))
        .or_else(|| root.map(|id| (id, "recorded-parent root (interim)")));
    let prime = resolved.map(|(id, _)| id);
    result.prime_tier = resolved.map(|(_, tier)| tier);
    let is_prime = prime == Some(seat.id.as_str());
    if !is_prime {
        result.add("Pij-Seat", seat.id.as_str());
    }
    if let Some(prime) = prime {
        result.add("Pij-Prime", prime);
    } else {
        result.warnings.push("Pij-Prime omitted: no prime ancestor, repository designation, or complete parent-chain root could be derived; explain this omission in the commit body".to_string());
    }
    if let Some(plan) = plan {
        result.add("Pij-Plan", plan);
    }
    result
}

fn plan_name(value: &str) -> Option<&str> {
    let (ordinal, slug) = value
        .split_once('-')
        .map_or((value, None), |(ordinal, slug)| (ordinal, Some(slug)));
    if ordinal.len() < 3 || !ordinal.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    if slug.is_some_and(|slug| {
        slug.split('-')
            .any(|part| part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_alphanumeric()))
    }) {
        return None;
    }
    Some(value)
}

#[cfg(test)]
mod tests {
    use pij_core::model::{Harness, SeatId};
    use pij_core::ports::{Registry, SeatFilter};
    use pij_testkit::fakes::FakeRegistry;

    use super::*;

    fn seat(id: &str, parent: Option<&str>, role: Option<&str>) -> SeatDescriptor {
        let mut seat = SeatDescriptor::new(id, Harness::Omp, "/repo");
        seat.parent = parent.map(SeatId::from);
        seat.role = role.map(str::to_string);
        seat
    }

    #[tokio::test]
    async fn prime_self_omits_seat_and_no_plan_has_no_placeholder() {
        let prime = seat("pij-prime", None, Some("prime"));
        let registry = FakeRegistry::new().with_seat(prime.clone());
        let rows = registry.list(SeatFilter::default()).await.unwrap();
        let trailers = derive(&prime, &rows, None, None);
        assert_eq!(trailers.text, "Pij-Prime: pij-prime\n");
        assert!(trailers.warnings.is_empty());
        assert_eq!(
            derive(&prime, &rows, None, Some("123-example")).text,
            "Pij-Prime: pij-prime\nPij-Plan: 123-example\n"
        );
    }

    #[tokio::test]
    async fn nearest_prime_wins_over_outer_prime_and_designation() {
        let coder = seat("pij-coder", Some("pij-pm"), None);
        let pm = seat("pij-pm", Some("pij-prime"), Some("pm"));
        let prime = seat("pij-prime", Some("pij-outer"), Some("prime"));
        let registry = FakeRegistry::new()
            .with_seat(coder.clone())
            .with_seat(pm)
            .with_seat(prime)
            .with_seat(seat("pij-outer", None, Some("prime")));
        let rows = registry.list(SeatFilter::default()).await.unwrap();
        let trailers = derive(&coder, &rows, Some("pij-designated"), Some("123"));
        assert_eq!(
            trailers.text,
            "Pij-Seat: pij-coder\nPij-Prime: pij-prime\nPij-Plan: 123\n"
        );
        assert!(trailers.warnings.is_empty());
        let mut direct = coder;
        direct.parent = Some("pij-prime".into());
        assert_eq!(
            derive(&direct, &rows, None, None).text,
            "Pij-Seat: pij-coder\nPij-Prime: pij-prime\n"
        );
    }

    #[tokio::test]
    async fn orphan_omits_unknown_prime_until_the_recorded_chain_is_complete() {
        let orphan = seat("pij-orphan", Some("pij-missing"), None);
        let registry = FakeRegistry::new().with_seat(orphan.clone());
        let rows = registry.list(SeatFilter::default()).await.unwrap();
        let trailers = derive(&orphan, &rows, None, None);
        assert_eq!(trailers.text, "Pij-Seat: pij-orphan\n");
        assert!(
            trailers
                .warnings
                .iter()
                .any(|warning| warning.contains("Pij-Prime omitted"))
        );
        let parent = seat("pij-missing", None, None);
        assert_eq!(
            derive(&orphan, &[parent], None, None).text,
            "Pij-Seat: pij-orphan\nPij-Prime: pij-missing\n"
        );
    }

    #[tokio::test]
    async fn designated_prime_covers_null_roles_and_unrelated_worktrees() {
        let prime = seat("pij-prime", None, None);
        let coder = seat("pij-coder", Some("pij-prime"), None);
        let registry = FakeRegistry::new()
            .with_seat(prime.clone())
            .with_seat(coder.clone());
        let rows = registry.list(SeatFilter::default()).await.unwrap();
        assert_eq!(
            derive(&prime, &rows, Some("pij-prime"), None).text,
            "Pij-Prime: pij-prime\n"
        );
        assert_eq!(
            derive(&coder, &rows, Some("pij-prime"), None).text,
            "Pij-Seat: pij-coder\nPij-Prime: pij-prime\n"
        );
    }

    #[test]
    fn recorded_root_is_prime_and_designation_takes_precedence() {
        let root = seat("pij-root", None, None);
        assert_eq!(derive(&root, &[], None, None).text, "Pij-Prime: pij-root\n");
        assert_eq!(
            derive(&root, &[], Some("pij-designated"), None).text,
            "Pij-Seat: pij-root\nPij-Prime: pij-designated\n"
        );
        let coder = seat("pij-coder", Some("pij-root"), None);
        assert_eq!(
            derive(&coder, &[root], None, None).text,
            "Pij-Seat: pij-coder\nPij-Prime: pij-root\n"
        );
    }

    #[test]
    fn cycles_terminate_and_fall_back_without_inventing_a_prime() {
        let coder = seat("pij-coder", Some("pij-pm"), None);
        let pm = seat("pij-pm", Some("pij-coder"), None);
        let trailers = derive(&coder, &[pm], Some("pij-prime"), None);
        assert_eq!(trailers.text, "Pij-Seat: pij-coder\nPij-Prime: pij-prime\n");
        assert!(
            trailers
                .warnings
                .iter()
                .any(|warning| warning.contains("cycle"))
        );
        let mut self_cycle = coder;
        self_cycle.parent = Some(self_cycle.id.clone());
        assert_eq!(
            derive(&self_cycle, &[], None, None).text,
            "Pij-Seat: pij-coder\n"
        );
    }

    #[test]
    fn malformed_values_cannot_inject_trailers() {
        let coder = seat("pij-coder", None, None);
        let trailers = derive(&coder, &[], Some("pij-prime\nPij-Plan: 999"), None);
        assert_eq!(trailers.text, "Pij-Seat: pij-coder\n");
        assert!(
            trailers
                .warnings
                .iter()
                .any(|warning| warning.contains("Pij-Prime omitted"))
        );
        for invalid in [
            "",
            "none",
            "12",
            "123-",
            "123-two--words",
            "123-x\nPij-Prime:",
            "issue-123",
            "123 name",
        ] {
            assert_eq!(plan_name(invalid), None, "{invalid:?}");
        }
        for valid in ["123", "123-example", "0123-a-new-plan"] {
            assert_eq!(plan_name(valid), Some(valid));
        }
    }

    #[test]
    fn current_tree_then_branch_then_flow_determine_plan() {
        let root = pij_testkit::fresh_dir("pij-trailer-plan");
        let nested = root.join("123-tree/src");
        std::fs::create_dir_all(&nested).unwrap();
        assert!(git(&root, &["init", "--initial-branch=124-branch"]).is_some());
        std::fs::write(
            root.join("the-flow.json"),
            r#"{"nav":{"bag":{"plan_dir":"docs/plans/125-flow"}}}"#,
        )
        .unwrap();
        let mut warnings = Vec::new();
        assert_eq!(
            working_plan(&nested, &mut warnings).as_deref(),
            Some("123-tree")
        );
        assert_eq!(
            working_plan(&root, &mut warnings).as_deref(),
            Some("124-branch")
        );
        assert!(git(&root, &["symbolic-ref", "HEAD", "refs/heads/no-plan"]).is_some());
        assert_eq!(
            working_plan(&root, &mut warnings).as_deref(),
            Some("125-flow")
        );
        assert!(warnings.is_empty());
        std::fs::remove_file(root.join("the-flow.json")).unwrap();
        assert_eq!(working_plan(&root, &mut warnings), None);
        std::fs::write(
            root.join(".the-flow-state.json"),
            r#"{"plan_dir":"docs/plans/126-legacy"}"#,
        )
        .unwrap();
        assert_eq!(
            working_plan(&root, &mut warnings).as_deref(),
            Some("126-legacy")
        );
    }

    #[test]
    fn malformed_flow_is_diagnosed_and_no_plan_is_not_a_placeholder() {
        let root = pij_testkit::fresh_dir("pij-trailer-malformed-flow");
        assert!(git(&root, &["init", "--initial-branch=no-plan"]).is_some());
        std::fs::write(root.join("the-flow.json"), "{").unwrap();
        let mut warnings = Vec::new();
        assert_eq!(working_plan(&root, &mut warnings), None);
        assert!(
            warnings
                .iter()
                .any(|warning| warning.contains("the-flow.json"))
        );
        warnings.clear();
        assert_eq!(
            flow_plan(
                &serde_json::json!({"nav":{"bag":{"plan_dir":"not-a-plan"}}}),
                &mut warnings
            ),
            None
        );
        assert!(
            warnings
                .iter()
                .any(|warning| warning.contains("Pij-Plan omitted"))
        );
        warnings.clear();
        assert_eq!(
            flow_plan(&serde_json::json!({"nav":{"bag":{}}}), &mut warnings),
            None
        );
        assert!(warnings.is_empty());
    }
}
