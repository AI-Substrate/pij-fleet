//! gh: found once at daemon start, like ddocs (ruling C10), and launched once
//! per `pij delivered` call by absolute path (plan 196, ruling C23).
//!
//! `PIJ_GH` wins when set; otherwise the first `gh` on PATH. The delivery verb
//! talks to it through [`PullRequests`], injected through `Services`, so tests
//! run with a fake and never reach GitHub.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Deserialize;

use super::Refusal;
use super::ddocs::resolve_tool;

/// gh is not installed, or not executable.
pub const E_GH_MISSING: &str = "E-RS-GH-MISSING";
/// gh is not signed in for the daemon's user.
pub const E_GH_AUTH: &str = "E-RS-GH-AUTH";
/// gh ran and refused, or printed something unreadable.
pub const E_GH: &str = "E-RS-GH";

/// What to do when gh is missing.
pub const INSTALL_STEP: &str = "Install the GitHub CLI so `gh` is on the daemon's PATH, or set PIJ_GH to its absolute path; then restart the daemon (`just bounce-rs`).";

/// What to do when gh is not signed in.
pub const AUTH_STEP: &str = "Run `gh auth login` as the daemon's user, then retry.";

/// The fields one `gh pr view` asks for.
pub const FIELDS: &str = "number,state,mergeCommit,body,files,url";

/// One pull request, as `pij delivered` needs it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PullRequest {
    /// The PR number.
    pub number: u64,
    /// GitHub's state: `OPEN`, `CLOSED` or `MERGED`.
    pub state: String,
    /// The merge commit, once merged.
    pub merge_commit: Option<String>,
    /// The PR body.
    pub body: String,
    /// Repository-relative paths the PR changed.
    pub files: Vec<String>,
    /// The PR's URL.
    pub url: Option<String>,
}

impl PullRequest {
    /// Whether GitHub says it merged.
    pub fn merged(&self) -> bool {
        self.state == "MERGED"
    }
}

/// Read one pull request. Blocking: call from `spawn_blocking`.
pub trait PullRequests: Send + Sync {
    /// The PR `number` of the repository checked out at `repo`.
    ///
    /// # Errors
    /// [`E_GH_MISSING`], [`E_GH_AUTH`] or [`E_GH`], each with its next step.
    fn view(&self, repo: &Path, number: u64) -> Result<PullRequest, Refusal>;

    /// The gh launched, for ping and `just doctor`; `None` when there is none.
    fn path(&self) -> Option<&Path> {
        None
    }
}

/// The gh this daemon will use, decided once at start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Gh {
    /// An executable at an absolute path.
    Found(PathBuf),
    /// Why there is none.
    Missing(String),
}

impl Gh {
    /// Resolve from the process environment. Call once, at daemon start.
    pub fn from_env() -> Self {
        Self::resolve(
            std::env::var_os("PIJ_GH").as_deref(),
            std::env::var_os("PATH").as_deref(),
            &std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/")),
        )
    }

    /// Resolve from explicit `PIJ_GH` and `PATH` values, as seen from `cwd`.
    pub fn resolve(pij_gh: Option<&OsStr>, path: Option<&OsStr>, cwd: &Path) -> Self {
        match resolve_tool("gh", "PIJ_GH", pij_gh, path, cwd) {
            Ok(found) => Self::Found(found),
            Err(reason) => Self::Missing(reason),
        }
    }
}

impl PullRequests for Gh {
    fn path(&self) -> Option<&Path> {
        match self {
            Self::Found(path) => Some(path),
            Self::Missing(_) => None,
        }
    }

    fn view(&self, repo: &Path, number: u64) -> Result<PullRequest, Refusal> {
        let program = match self {
            Self::Found(path) => path,
            Self::Missing(reason) => {
                return Err(Refusal::new(E_GH_MISSING, reason.clone()).with_next(INSTALL_STEP));
            }
        };
        // `--repo` read from the checkout's own config, so gh has no reason to
        // launch git; without it gh finds the repository from `repo`.
        let named = github_repo(repo);
        pij_core::spawns::record("gh.delivered");
        let output = pij_core::program::launch(program, |gh| {
            let mut command = Command::new(gh);
            command
                .current_dir(repo)
                .env("GH_PROMPT_DISABLED", "1")
                .env("NO_COLOR", "1")
                .args(["pr", "view", &number.to_string(), "--json", FIELDS]);
            if let Some(named) = &named {
                command.args(["--repo", named]);
            }
            command.output()
        })
        .map_err(|error| {
            Refusal::new(
                E_GH_MISSING,
                format!("could not run {}: {error}", program.display()),
            )
            .with_next(INSTALL_STEP)
        })?;
        if !output.status.success() {
            return Err(failure(&String::from_utf8_lossy(&output.stderr)));
        }
        parse(&output.stdout)
    }
}

/// gh's refusal, named: an auth failure gets the sign-in step.
fn failure(stderr: &str) -> Refusal {
    let said = stderr.trim();
    let lower = said.to_ascii_lowercase();
    if lower.contains("gh auth login") || lower.contains("authentication") {
        return Refusal::new(E_GH_AUTH, format!("gh is not signed in: {said}")).with_next(AUTH_STEP);
    }
    Refusal::new(E_GH, format!("gh pr view failed: {said}"))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Wire {
    number: u64,
    state: String,
    merge_commit: Option<Oid>,
    #[serde(default)]
    body: String,
    #[serde(default)]
    files: Vec<WireFile>,
    url: Option<String>,
}

#[derive(Deserialize)]
struct Oid {
    oid: String,
}

#[derive(Deserialize)]
struct WireFile {
    path: String,
}

/// `gh pr view --json number,state,mergeCommit,body,files,url` output.
///
/// # Errors
/// [`E_GH`] when it is not that shape.
pub fn parse(stdout: &[u8]) -> Result<PullRequest, Refusal> {
    let wire: Wire = serde_json::from_slice(stdout)
        .map_err(|error| Refusal::new(E_GH, format!("gh printed an unreadable PR: {error}")))?;
    Ok(PullRequest {
        number: wire.number,
        state: wire.state,
        merge_commit: wire.merge_commit.map(|oid| oid.oid),
        body: wire.body,
        files: wire.files.into_iter().map(|file| file.path).collect(),
        url: wire.url,
    })
}

/// `owner/name` of the checkout's `origin` on github.com, read in-process
/// from its git config; `None` when there is no such remote.
fn github_repo(repo: &Path) -> Option<String> {
    let config = std::fs::read_to_string(super::root::common_dir(repo)?.join("config")).ok()?;
    origin_on_github(&config)
}

/// The `[remote "origin"]` url's `owner/name` when it is on github.com.
fn origin_on_github(config: &str) -> Option<String> {
    let mut in_origin = false;
    for line in config.lines().map(str::trim) {
        if line.starts_with('[') {
            in_origin = line == r#"[remote "origin"]"#;
            continue;
        }
        let Some(url) = line
            .strip_prefix("url")
            .map(str::trim_start)
            .and_then(|rest| rest.strip_prefix('='))
            .map(str::trim)
            .filter(|_| in_origin)
        else {
            continue;
        };
        let path = url
            .strip_prefix("git@github.com:")
            .or_else(|| url.strip_prefix("ssh://git@github.com/"))
            .or_else(|| url.strip_prefix("https://github.com/"))?;
        let path = path.trim_end_matches('/').trim_end_matches(".git");
        let (owner, name) = path.split_once('/')?;
        let plain = |part: &str| {
            !part.is_empty()
                && part
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        };
        return (plain(owner) && plain(name)).then(|| format!("{owner}/{name}"));
    }
    None
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    fn tool(dir: &Path, name: &str, script: &str) -> PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, script).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[test]
    fn pij_gh_wins_over_path_and_must_be_an_absolute_executable() {
        let root = pij_testkit::fresh_dir("pij-gh-resolve").canonicalize().unwrap();
        let on_path = tool(&root.join("bin"), "gh", "#!/bin/sh\n");
        let chosen = tool(&root.join("pinned"), "gh-pinned", "#!/bin/sh\n");
        let path = std::env::join_paths([root.join("bin")]).unwrap();
        assert_eq!(
            Gh::resolve(Some(chosen.as_os_str()), Some(&path), &root),
            Gh::Found(chosen)
        );
        assert_eq!(Gh::resolve(None, Some(&path), &root), Gh::Found(on_path));
        let Gh::Missing(reason) = Gh::resolve(Some(OsStr::new("gh")), Some(&path), &root) else {
            panic!("a relative PIJ_GH is missing");
        };
        assert_eq!(reason, "PIJ_GH=gh is not an absolute path");
        let empty = root.join("empty");
        std::fs::create_dir_all(&empty).unwrap();
        let none = Gh::resolve(None, Some(empty.as_os_str()), &root);
        assert_eq!(
            none,
            Gh::Missing("gh is not on the daemon's PATH and PIJ_GH is unset".into())
        );
        let refusal = none.view(&root, 1).expect_err("no gh");
        assert_eq!(refusal.code, E_GH_MISSING);
        assert_eq!(refusal.next.as_deref(), Some(INSTALL_STEP));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_launched_gh_is_read_and_its_refusals_are_named() {
        let root = pij_testkit::fresh_dir("pij-gh-launch").canonicalize().unwrap();
        let ok = tool(
            &root.join("ok"),
            "gh",
            "#!/bin/sh\necho '{\"number\":7,\"state\":\"MERGED\",\"mergeCommit\":{\"oid\":\"abc\"},\"body\":\"b\",\"files\":[{\"path\":\"a/b.rs\"}],\"url\":\"u\"}'\n",
        );
        let pr = Gh::Found(ok).view(&root, 7).expect("a fake gh answers");
        assert!(pr.merged());
        assert_eq!(pr.merge_commit.as_deref(), Some("abc"));
        assert_eq!(pr.files, ["a/b.rs"]);
        let auth = tool(
            &root.join("auth"),
            "gh",
            "#!/bin/sh\necho 'To get started with GitHub CLI, please run:  gh auth login' >&2\nexit 4\n",
        );
        let refusal = Gh::Found(auth).view(&root, 7).expect_err("not signed in");
        assert_eq!(refusal.code, E_GH_AUTH);
        assert_eq!(refusal.next.as_deref(), Some(AUTH_STEP));
        let gone = tool(
            &root.join("gone"),
            "gh",
            "#!/bin/sh\necho 'GraphQL: Could not resolve to a PullRequest' >&2\nexit 1\n",
        );
        assert_eq!(Gh::Found(gone).view(&root, 7).expect_err("no PR").code, E_GH);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn an_open_pr_parses_without_a_merge_commit() {
        let pr = parse(br#"{"number":3,"state":"OPEN","mergeCommit":null,"body":"","files":[]}"#)
            .unwrap();
        assert!(!pr.merged());
        assert_eq!(pr.merge_commit, None);
        assert_eq!(parse(b"nope").unwrap_err().code, E_GH);
    }

    #[test]
    fn origin_on_github_reads_ssh_and_https_and_ignores_other_remotes() {
        let config = "[core]\n\tbare = false\n[remote \"upstream\"]\n\turl = git@github.com:up/stream.git\n[remote \"origin\"]\n\turl = git@github.com:acme/widget.git\n";
        assert_eq!(origin_on_github(config).as_deref(), Some("acme/widget"));
        assert_eq!(
            origin_on_github("[remote \"origin\"]\n url = https://github.com/acme/widget\n")
                .as_deref(),
            Some("acme/widget")
        );
        assert_eq!(
            origin_on_github("[remote \"origin\"]\n url = https://example.com/acme/widget\n"),
            None
        );
        assert_eq!(origin_on_github("[core]\n"), None);
    }
}
