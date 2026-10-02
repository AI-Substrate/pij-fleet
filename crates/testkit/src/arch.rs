//! Mechanical enforcement of the crate graph (workshop 001 R2).
//!
//! Cargo already refuses cycles and undeclared imports. What it cannot refuse is
//! an edge the *architecture* forbids — `tokio` in `pij-core`, `sqlx` in
//! `pij-cli`, a mocking framework anywhere. This module closes that gap:
//! [`allowlist`] is the ratified graph as data, [`check`] is the verdict, and
//! `crates/testkit/src/bin/arch_check.rs` is the command that runs it.
//!
//! [`check`] is **pure** over a [`Graph`], which is what makes the negative proof
//! re-runnable: `tests/arch_drift.rs` judges a committed metadata fixture that
//! contains a forbidden edge and asserts RED for ever, rather than relying on
//! someone repeating a violate-and-revert ritual by hand.
//!
//! Shape adapted from flowspace3's `fs3-testkit::arch` (plan 001, read-only
//! prior art vendored under `assets/inputs/`) — kind-awareness included, because
//! "dev-only" carried in a TOML comment is not enforcement.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::process::Command;

use serde::Deserialize;

/// Why the check could not reach a verdict — distinct from a [`Violation`],
/// which is the graph being wrong rather than the instrument failing.
#[derive(Debug, thiserror::Error)]
pub enum ArchError {
    /// `cargo metadata` could not be run, or exited non-zero.
    #[error("could not run `cargo metadata`: {0}")]
    Metadata(String),
    /// The metadata JSON did not have the shape this check reads.
    #[error("could not parse `cargo metadata` output: {0}")]
    ParseMetadata(#[from] serde_json::Error),
    /// The committed allow-list is not valid TOML for this shape.
    #[error("could not parse the architecture allow-list: {0}")]
    ParseAllowlist(#[from] toml::de::Error),
}

/// Which dependency table an edge was declared in. The distinction is
/// architectural: a test-only edge that gets promoted ships in the binary.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum DepKind {
    /// `[dependencies]` — ships.
    Normal,
    /// `[dev-dependencies]` — tests, benches, examples.
    Dev,
    /// `[build-dependencies]` — build scripts.
    Build,
}

impl DepKind {
    /// The table name as Cargo spells it.
    pub const fn as_str(self) -> &'static str {
        match self {
            DepKind::Normal => "dependencies",
            DepKind::Dev => "dev-dependencies",
            DepKind::Build => "build-dependencies",
        }
    }
}

impl std::fmt::Display for DepKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One direct dependency edge.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Dep {
    /// Crate name as declared.
    pub name: String,
    /// The table that declared it.
    pub kind: DepKind,
}

/// One allow-list entry: `"serde"` may ship, `"tokio@dev"` is test-only,
/// `"cc@build"` is build-script-only. An unknown suffix fails the parse rather
/// than being swallowed into a crate name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rule {
    /// The crate this rule permits.
    pub dep: String,
    /// The most privileged table it may appear in.
    pub kind: DepKind,
}

impl Rule {
    /// Does this rule permit an edge actually declared in `actual`?
    ///
    /// Privilege runs one way: cleared to ship implies cleared for tests. The
    /// converse is the whole point — a `@dev` rule never permits a shipped edge.
    pub const fn permits(&self, actual: DepKind) -> bool {
        matches!(
            (self.kind, actual),
            (DepKind::Normal, DepKind::Normal | DepKind::Dev)
                | (DepKind::Dev, DepKind::Dev)
                | (DepKind::Build, DepKind::Build)
        )
    }
}

impl<'de> Deserialize<'de> for Rule {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        let (dep, kind) = match raw.split_once('@') {
            None => (raw.as_str(), DepKind::Normal),
            Some((dep, "dev")) => (dep, DepKind::Dev),
            Some((dep, "build")) => (dep, DepKind::Build),
            Some((_, unknown)) => {
                return Err(serde::de::Error::custom(format!(
                    "allow-list entry `{raw}`: unknown dependency kind `@{unknown}` \
                     — use `@dev`, `@build`, or no suffix for a shipped [dependencies] edge"
                )));
            }
        };
        if dep.is_empty() {
            return Err(serde::de::Error::custom(format!(
                "allow-list entry `{raw}` names no crate"
            )));
        }
        Ok(Rule {
            dep: dep.to_string(),
            kind,
        })
    }
}

/// One workspace crate and its direct edges.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CrateDeps {
    /// Package name, e.g. `pij-core`.
    pub name: String,
    /// Direct edges across every dependency table.
    pub deps: Vec<Dep>,
}

/// The workspace's direct-dependency graph — all the check reasons over.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Graph {
    /// Workspace members only; transitive dependencies are not this check's job.
    pub crates: Vec<CrateDeps>,
}

impl Graph {
    /// Member names, so an internal edge is told from an external one by
    /// membership rather than by guessing at a `pij-` prefix.
    pub fn member_names(&self) -> BTreeSet<&str> {
        self.crates.iter().map(|c| c.name.as_str()).collect()
    }

    /// Build a graph from `cargo metadata --no-deps --format-version 1`.
    ///
    /// # Errors
    /// [`ArchError::ParseMetadata`] when the JSON is not cargo metadata.
    pub fn from_cargo_metadata(json: &str) -> Result<Self, ArchError> {
        let metadata: Metadata = serde_json::from_str(json)?;
        let members: BTreeSet<&str> = metadata
            .workspace_members
            .iter()
            .map(String::as_str)
            .collect();

        let mut crates: Vec<CrateDeps> = metadata
            .packages
            .iter()
            .filter(|package| members.contains(package.id.as_str()))
            .map(|package| {
                let mut deps: Vec<Dep> = package
                    .dependencies
                    .iter()
                    .map(|dependency| Dep {
                        name: dependency.name.clone(),
                        kind: match dependency.kind.as_deref() {
                            Some("dev") => DepKind::Dev,
                            Some("build") => DepKind::Build,
                            _ => DepKind::Normal,
                        },
                    })
                    .collect();
                deps.sort();
                deps.dedup();
                CrateDeps {
                    name: package.name.clone(),
                    deps,
                }
            })
            .collect();
        crates.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(Graph { crates })
    }
}

/// The architecture, as data. See `crates/testkit/arch-allowlist.toml`.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Allowlist {
    /// Crates refused in every table of every crate.
    #[serde(default)]
    pub banned_everywhere: Vec<String>,
    /// Per-crate permitted edges, keyed by package name.
    pub crates: BTreeMap<String, CrateRules>,
}

/// The edges one crate is allowed to have.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CrateRules {
    /// Permitted workspace-internal edges.
    #[serde(default)]
    pub internal: Vec<Rule>,
    /// Permitted direct external edges.
    #[serde(default)]
    pub external: Vec<Rule>,
    /// Why this crate has the edges it has. Required: a row without a reason is
    /// how an allow-list decays into a list of whatever happened to be added.
    pub why: String,
}

impl CrateRules {
    fn rule_for(&self, dep: &str, internal: bool) -> Option<&Rule> {
        let table = if internal {
            &self.internal
        } else {
            &self.external
        };
        table.iter().find(|rule| rule.dep == dep)
    }
}

/// A refused edge, or an allow-list that has fallen out of step with the tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Violation {
    /// A workspace crate the allow-list does not describe.
    UndescribedCrate {
        /// The undescribed member.
        crate_name: String,
    },
    /// An allow-list entry whose crate no longer exists.
    StaleAllowlistEntry {
        /// The stale entry.
        crate_name: String,
    },
    /// A permitted edge that the crate no longer has.
    ///
    /// The check used to be one-directional — it judged every real edge against
    /// the list but never the list against reality — so a dependency that was
    /// removed stayed PRE-APPROVED, and reintroducing it later would pass without
    /// the reviewed line the allow-list exists to force. Found in review.
    StaleRule {
        /// The crate whose rule is unused.
        crate_name: String,
        /// The dependency nobody depends on any more.
        dep: String,
        /// Which table the rule permits it in.
        kind: DepKind,
    },
    /// A workspace-internal edge the architecture refuses.
    ForbiddenInternal {
        /// The crate that declared it.
        crate_name: String,
        /// The member it depends on.
        dep: String,
        /// The table that declared it.
        kind: DepKind,
    },
    /// An external edge this crate's allow-list does not carry.
    ForbiddenExternal {
        /// The crate that declared it.
        crate_name: String,
        /// The external crate.
        dep: String,
        /// The table that declared it.
        kind: DepKind,
    },
    /// A crate refused workspace-wide.
    BannedEverywhere {
        /// The crate that declared it.
        crate_name: String,
        /// The refused crate.
        dep: String,
        /// The table that declared it.
        kind: DepKind,
    },
    /// A permitted edge declared in a table it is not permitted in — typically a
    /// dev-only dependency promoted into the shipped binary.
    WrongDependencyKind {
        /// The crate that declared it.
        crate_name: String,
        /// The crate it depends on.
        dep: String,
        /// The table it was actually declared in.
        kind: DepKind,
        /// The most privileged table the allow-list permits.
        allowed: DepKind,
    },
}

impl std::fmt::Display for Violation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Violation::UndescribedCrate { crate_name } => write!(
                f,
                "{crate_name}: workspace member has no entry in crates/testkit/arch-allowlist.toml \
                 — add one, with a `why`, describing the edges it may have"
            ),
            Violation::StaleAllowlistEntry { crate_name } => write!(
                f,
                "{crate_name}: named in crates/testkit/arch-allowlist.toml but absent from the \
                 workspace — remove the entry or restore the crate"
            ),
            Violation::StaleRule {
                crate_name,
                dep,
                kind,
            } => write!(
                f,
                "{crate_name} -> {dep} ({kind}): allow-listed but no longer a dependency — remove \
                 the row from crates/testkit/arch-allowlist.toml, so re-adding the edge costs the \
                 reviewed line it is supposed to cost"
            ),
            Violation::ForbiddenInternal {
                crate_name,
                dep,
                kind,
            } => write!(
                f,
                "{crate_name} -> {dep} ({kind}): crate-graph edge refused by workshop 001 R2. \
                 Direction is core <- everything; daemon and cli are the only composition roots."
            ),
            Violation::ForbiddenExternal {
                crate_name,
                dep,
                kind,
            } => write!(
                f,
                "{crate_name} -> {dep} ({kind}): not in this crate's allow-list. If the edge is \
                 genuinely architectural, add `{dep}` to [crates.{crate_name}].external in \
                 crates/testkit/arch-allowlist.toml, with a rationale; if it is not, put the \
                 code in the crate that owns the concern."
            ),
            Violation::BannedEverywhere {
                crate_name,
                dep,
                kind,
            } => write!(
                f,
                "{crate_name} -> {dep} ({kind}): refused workspace-wide. Doubles come from \
                 pij-testkit's shipped fakes, never a mocking framework (tenet 5)."
            ),
            Violation::WrongDependencyKind {
                crate_name,
                dep,
                kind,
                allowed,
            } => write!(
                f,
                "{crate_name} -> {dep}: declared in [{kind}] but the allow-list permits it only \
                 in [{allowed}]. {advice}",
                advice = kind_advice(dep, *kind, *allowed)
            ),
        }
    }
}

/// The actionable half of a [`Violation::WrongDependencyKind`] message, rendered
/// from the ACTUAL/ALLOWED pair rather than from `allowed` alone — advice that
/// points at a line which is already correct costs a round trip to disbelieve.
fn kind_advice(dep: &str, actual: DepKind, allowed: DepKind) -> String {
    match (actual, allowed) {
        (DepKind::Normal, DepKind::Dev) => format!(
            "A dev-only edge that gets promoted ships in the binary; if that is deliberate, \
             change `{dep}@dev` to `{dep}` in the allow-list and say why in review."
        ),
        (DepKind::Normal, DepKind::Build) => format!(
            "A build-script edge that gets promoted ships in the binary; if that is deliberate, \
             change `{dep}@build` to `{dep}` in the allow-list and say why in review."
        ),
        (DepKind::Build, _) => format!(
            "If the build-script edge is intentional, write `{dep}@build`; if not, move {dep} out \
             of [build-dependencies]."
        ),
        (DepKind::Dev, _) => format!(
            "If the test-only edge is intentional, write `{dep}@dev`; if not, move {dep} out of \
             [dev-dependencies]."
        ),
        // `check` never constructs this pair — a shipped rule permits a shipped
        // edge — so say the true general thing rather than invent a suffix.
        (DepKind::Normal, DepKind::Normal) => {
            format!("Reconcile `{dep}` in the allow-list with the table it is declared in.")
        }
    }
}

/// The committed allow-list, compiled in so the check works from any cwd.
///
/// # Errors
/// [`ArchError::ParseAllowlist`] when the committed allow-list is malformed.
pub fn allowlist() -> Result<Allowlist, ArchError> {
    Ok(toml::from_str(include_str!("../arch-allowlist.toml"))?)
}

/// Judge a graph against an allow-list. An empty result means no drift.
///
/// Pure — which is what makes the negative proof re-runnable.
pub fn check(graph: &Graph, allowlist: &Allowlist) -> Vec<Violation> {
    let members = graph.member_names();
    let banned: BTreeSet<&str> = allowlist
        .banned_everywhere
        .iter()
        .map(String::as_str)
        .collect();

    let mut violations = Vec::new();

    for krate in &graph.crates {
        let Some(rules) = allowlist.crates.get(&krate.name) else {
            violations.push(Violation::UndescribedCrate {
                crate_name: krate.name.clone(),
            });
            continue;
        };

        for dep in &krate.deps {
            let crate_name = krate.name.clone();
            let dep_name = dep.name.clone();

            if banned.contains(dep.name.as_str()) {
                violations.push(Violation::BannedEverywhere {
                    crate_name,
                    dep: dep_name,
                    kind: dep.kind,
                });
                continue;
            }

            let internal = members.contains(dep.name.as_str());
            match rules.rule_for(&dep.name, internal) {
                None if internal => violations.push(Violation::ForbiddenInternal {
                    crate_name,
                    dep: dep_name,
                    kind: dep.kind,
                }),
                None => violations.push(Violation::ForbiddenExternal {
                    crate_name,
                    dep: dep_name,
                    kind: dep.kind,
                }),
                Some(rule) if !rule.permits(dep.kind) => {
                    violations.push(Violation::WrongDependencyKind {
                        crate_name,
                        dep: dep_name,
                        kind: dep.kind,
                        allowed: rule.kind,
                    });
                }
                Some(_) => {}
            }
        }
    }

    // An entry that outlives its crate permits nothing and hides a rename, so it
    // is drift too.
    for name in allowlist.crates.keys() {
        if !members.contains(name.as_str()) {
            violations.push(Violation::StaleAllowlistEntry {
                crate_name: name.clone(),
            });
        }
    }

    // ...and so is a RULE that outlives its edge: permission nobody is using is
    // permission nobody reviewed recently.
    for krate in &graph.crates {
        let Some(rules) = allowlist.crates.get(&krate.name) else {
            continue;
        };
        for (rule, internal) in rules
            .internal
            .iter()
            .map(|rule| (rule, true))
            .chain(rules.external.iter().map(|rule| (rule, false)))
        {
            let used = krate
                .deps
                .iter()
                .any(|dep| dep.name == rule.dep && members.contains(dep.name.as_str()) == internal);
            if !used {
                violations.push(Violation::StaleRule {
                    crate_name: krate.name.clone(),
                    dep: rule.dep.clone(),
                    kind: rule.kind,
                });
            }
        }
    }

    violations
}

/// Absolute path to the workspace root manifest.
///
/// Resolved at RUNTIME, and that is the whole point. The first version used
/// `env!("CARGO_MANIFEST_DIR")`, which bakes the directory the binary was
/// COMPILED in: a `pij-gate` built once inside a temporary export tree then
/// judged that tree for ever, from any working directory. It surfaced as six
/// "No such file or directory" stages pointing at a `/private/tmp/...` path that
/// no longer existed — but the dangerous shape is the other one, where the stale
/// tree still exists and the gate reports GREEN for a checkout nobody is looking
/// at. Found by the reviewer's own probe run, 2026-08-28.
///
/// Order: `$CARGO_MANIFEST_DIR` when cargo set it for THIS run, then the current
/// directory, then the compile-time path — and each candidate must actually
/// exist and declare `[workspace]` before it is used.
pub fn workspace_manifest_path() -> PathBuf {
    let candidates = [
        std::env::var_os("CARGO_MANIFEST_DIR").map(PathBuf::from),
        std::env::current_dir().ok(),
        Some(PathBuf::from(env!("CARGO_MANIFEST_DIR"))),
    ];

    for start in candidates.into_iter().flatten() {
        for ancestor in start.ancestors() {
            let candidate = ancestor.join("Cargo.toml");
            if let Ok(text) = std::fs::read_to_string(&candidate)
                && text.contains("[workspace]")
            {
                return candidate;
            }
        }
    }

    // Nothing resolvable: return a path that fails loudly and names itself,
    // rather than a plausible-looking guess.
    PathBuf::from("Cargo.toml")
}

/// Read the live workspace graph by shelling out to `cargo metadata`.
///
/// Uses `$CARGO` when set, so the check runs on the same pinned toolchain that
/// invoked it rather than on whatever `cargo` PATH happens to resolve to — the
/// hazard that produced R6a-AMEND-1 on this very machine.
///
/// # Errors
/// [`ArchError::Metadata`] when cargo cannot be run or exits non-zero;
/// [`ArchError::ParseMetadata`] when its output is unreadable.
pub fn workspace_graph() -> Result<Graph, ArchError> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let output = Command::new(cargo)
        .args(["metadata", "--no-deps", "--format-version", "1"])
        .arg("--manifest-path")
        .arg(workspace_manifest_path())
        .output()
        .map_err(|e| ArchError::Metadata(e.to_string()))?;

    if !output.status.success() {
        return Err(ArchError::Metadata(
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        ));
    }
    Graph::from_cargo_metadata(&String::from_utf8_lossy(&output.stdout))
}

// --- the slice of `cargo metadata` this check reads -------------------------

#[derive(Deserialize)]
struct Metadata {
    packages: Vec<MetaPackage>,
    workspace_members: Vec<String>,
}

#[derive(Deserialize)]
struct MetaPackage {
    name: String,
    id: String,
    #[serde(default)]
    dependencies: Vec<MetaDependency>,
}

#[derive(Deserialize)]
struct MetaDependency {
    name: String,
    #[serde(default)]
    kind: Option<String>,
}
