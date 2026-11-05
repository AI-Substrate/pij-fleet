//! Find the standing governance worktree: the Rust port of
//! harness/scripts/government-root.sh.
//!
//! The root is the one registered worktree with `prime-governance` checked
//! out, never main's stub. It costs three git launches, and is resolved per
//! write, never on a timer.

use std::path::{Path, PathBuf};

use super::git::{Locks, git};
use super::{E_GOV_ABSENT, Refusal};

/// The branch the community directory lives on.
pub const GOVERNANCE_BRANCH: &str = "prime-governance";

/// What government-root.sh prints after every refusal.
const BOOTSTRAP: &str = "Bootstrap: /pij prime — follow skills/pij/references/prime/rituals/bootstrap.md § 3 (restore an existing branch; never recreate it).";

/// The standing governance worktree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GovernanceRoot {
    /// The worktree's top level: git runs here, ddocs finds `.dd/` here.
    pub worktree: PathBuf,
    /// `<worktree>/.harness/government`, the `<government-root>`.
    pub government: PathBuf,
}

/// Resolve the standing governance worktree for the repository holding `repo`.
///
/// # Errors
/// [`E_GOV_ABSENT`] with the bootstrap step, whenever there is not exactly one
/// usable standing worktree.
pub fn resolve(repo: &Path) -> Result<GovernanceRoot, Refusal> {
    let lines = read(
        repo,
        &[
            "rev-parse",
            "--path-format=absolute",
            "--show-toplevel",
            "--git-common-dir",
        ],
    )
    .map_err(|message| absent(&message, None))?;
    let mut lines = lines.lines();
    let (Some(top), Some(common)) = (lines.next(), lines.next()) else {
        return Err(absent(
            &format!("{} is not inside a git repository", repo.display()),
            None,
        ));
    };
    let top = PathBuf::from(top);
    let common = canonical(Path::new(common));
    let refuse = |message: String| absent(&message, Some(&top));

    let listing = read(&top, &["worktree", "list", "--porcelain", "-z"]).map_err(&refuse)?;
    let wanted = format!("branch refs/heads/{GOVERNANCE_BRANCH}");
    let mut path = None;
    let mut roots = Vec::new();
    for field in listing.split('\0') {
        if let Some(worktree) = field.strip_prefix("worktree ") {
            path = Some(PathBuf::from(worktree));
        } else if field == wanted
            && let Some(path) = &path
        {
            roots.push(path.clone());
        }
    }
    let [root] = roots.as_slice() else {
        return Err(refuse(format!(
            "expected one standing {GOVERNANCE_BRANCH} worktree; found {}",
            roots.len()
        )));
    };
    if !root.is_dir() || !root.join(".git").exists() {
        return Err(refuse(format!(
            "standing worktree is unavailable: {}",
            root.display()
        )));
    }
    let facts = read(
        root,
        &[
            "rev-parse",
            "--path-format=absolute",
            "--git-common-dir",
            "--symbolic-full-name",
            "HEAD",
        ],
    )
    .map_err(|_| refuse(format!("cannot read standing worktree: {}", root.display())))?;
    let mut facts = facts.lines();
    if facts.next().map(|dir| canonical(Path::new(dir))) != Some(common) {
        return Err(refuse(format!(
            "standing path belongs to a different repository: {}",
            root.display()
        )));
    }
    match facts.next() {
        Some(branch) if branch == format!("refs/heads/{GOVERNANCE_BRANCH}") => {}
        Some("HEAD") | None => {
            return Err(refuse(format!(
                "standing worktree is detached: {}",
                root.display()
            )));
        }
        Some(_) => {
            return Err(refuse(format!(
                "standing worktree changed branch: {}",
                root.display()
            )));
        }
    }
    let government = root.join(super::layout::GOVERNMENT);
    if !government.is_dir() {
        return Err(refuse(format!(
            "government directory is unavailable: {}",
            government.display()
        )));
    }
    Ok(GovernanceRoot {
        worktree: root.clone(),
        government,
    })
}

/// [`resolve`] without launching anything, for the view (plan 195, AC7): it
/// reads git's own worktree records (`<common>/HEAD`, and `HEAD` and `gitdir`
/// under `<common>/worktrees/*`) to find the one worktree with the governance
/// branch checked out.
///
/// # Errors
/// [`E_GOV_ABSENT`] with the bootstrap step, as [`resolve`].
pub fn find(repo: &Path) -> Result<GovernanceRoot, Refusal> {
    let refuse = |message: String| {
        Refusal::new(E_GOV_ABSENT, format!("government-root: {message}")).with_next(BOOTSTRAP)
    };
    let common = common_dir(repo)
        .ok_or_else(|| refuse(format!("{} is not inside a git repository", repo.display())))?;
    let wanted = format!("ref: refs/heads/{GOVERNANCE_BRANCH}");
    let checked_out =
        |head: &Path| std::fs::read_to_string(head).is_ok_and(|text| text.trim() == wanted);
    let mut roots = Vec::new();
    if checked_out(&common.join("HEAD"))
        && let Some(main) = common.parent()
    {
        roots.push(main.to_path_buf());
    }
    for entry in std::fs::read_dir(common.join("worktrees"))
        .into_iter()
        .flatten()
        .flatten()
    {
        let record = entry.path();
        if checked_out(&record.join("HEAD"))
            && let Ok(gitdir) = std::fs::read_to_string(record.join("gitdir"))
            && let Some(root) = Path::new(gitdir.trim()).parent()
        {
            roots.push(root.to_path_buf());
        }
    }
    let [root] = roots.as_slice() else {
        return Err(refuse(format!(
            "expected one standing {GOVERNANCE_BRANCH} worktree; found {}",
            roots.len()
        )));
    };
    let government = root.join(super::layout::GOVERNMENT);
    if !government.is_dir() {
        return Err(refuse(format!(
            "government directory is unavailable: {}",
            government.display()
        )));
    }
    Ok(GovernanceRoot {
        worktree: canonical(root),
        government: canonical(&government),
    })
}

/// The repository's common git directory, found by walking up from `start`.
pub(crate) fn common_dir(start: &Path) -> Option<PathBuf> {
    let mut dir = Some(start);
    while let Some(current) = dir {
        let dotgit = current.join(".git");
        if dotgit.is_dir() {
            return Some(canonical(&dotgit));
        }
        if let Ok(text) = std::fs::read_to_string(&dotgit) {
            let gitdir = current.join(text.trim().strip_prefix("gitdir:")?.trim());
            let common = std::fs::read_to_string(gitdir.join("commondir"))
                .map(|relative| gitdir.join(relative.trim()))
                .unwrap_or(gitdir);
            return Some(canonical(&common));
        }
        dir = current.parent();
    }
    None
}

fn read(dir: &Path, args: &[&str]) -> Result<String, String> {
    let output =
        git(dir, args, Locks::Read).map_err(|error| format!("could not run git: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "git {} failed in {}: {}",
            args.join(" "),
            dir.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    String::from_utf8(output.stdout).map_err(|_| "git printed a non-UTF-8 path".to_string())
}

fn canonical(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

/// The refusal, with government-root.sh's bootstrap step. When the branch
/// exists but no worktree is registered, it also names the `worktree add`.
fn absent(message: &str, repo: Option<&Path>) -> Refusal {
    let mut next = BOOTSTRAP.to_string();
    if let Some(repo) = repo
        && read(
            repo,
            &[
                "show-ref",
                "--verify",
                "--quiet",
                &format!("refs/heads/{GOVERNANCE_BRANCH}"),
            ],
        )
        .is_ok()
    {
        next.push_str(&format!(
            "\nIf no worktree is registered, choose an unused permanent path and run:\n  git -C {} worktree add <standing-worktree> {GOVERNANCE_BRANCH}\nFor an unavailable registered worktree, restore its directory/mount first; do not prune or replace it blindly.",
            repo.display()
        ));
    }
    Refusal::new(E_GOV_ABSENT, format!("government-root: {message}")).with_next(next)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::community::test_repo::Scratch;

    #[test]
    fn it_finds_the_standing_worktree_from_main_or_from_itself() {
        let scratch = Scratch::new("pij-community-root");
        let expected = GovernanceRoot {
            worktree: scratch.gov.clone(),
            government: scratch.gov.join(".harness/government"),
        };
        assert_eq!(resolve(&scratch.main), Ok(expected.clone()));
        assert_eq!(resolve(&scratch.gov.join(".harness")), Ok(expected));
    }

    #[test]
    fn no_worktree_refuses_with_the_worktree_add_step() {
        let scratch = Scratch::new("pij-community-root-none");
        scratch.git(
            &scratch.main,
            &[
                "worktree",
                "remove",
                "--force",
                scratch.gov.to_str().unwrap(),
            ],
        );
        let refusal = resolve(&scratch.main).expect_err("no standing worktree");
        assert_eq!(refusal.code, E_GOV_ABSENT);
        assert!(refusal.message.contains("found 0"), "{refusal}");
        let next = refusal.next.expect("a next step");
        assert!(next.starts_with(BOOTSTRAP), "{next}");
        assert!(
            next.contains("worktree add <standing-worktree> prime-governance"),
            "{next}"
        );
    }

    #[test]
    fn a_repo_without_the_branch_refuses_with_only_the_bootstrap_step() {
        let scratch = Scratch::bare_main("pij-community-root-nobranch");
        let refusal = resolve(&scratch.main).expect_err("no branch");
        assert_eq!(refusal.next.as_deref(), Some(BOOTSTRAP));
    }

    #[test]
    fn a_detached_or_unavailable_or_hollow_worktree_refuses() {
        let scratch = Scratch::new("pij-community-root-detached");
        scratch.git(&scratch.gov, &["checkout", "--detach", "-q"]);
        // Detached, the worktree no longer lists the branch at all.
        assert!(
            resolve(&scratch.main)
                .unwrap_err()
                .message
                .contains("found 0")
        );
        scratch.git(&scratch.gov, &["checkout", "-q", GOVERNANCE_BRANCH]);

        std::fs::remove_dir_all(scratch.gov.join(".harness/government")).unwrap();
        let refusal = resolve(&scratch.main).unwrap_err();
        assert!(
            refusal
                .message
                .contains("government directory is unavailable"),
            "{refusal}"
        );

        std::fs::remove_dir_all(&scratch.gov).unwrap();
        let refusal = resolve(&scratch.main).unwrap_err();
        assert!(
            refusal.message.contains("standing worktree is unavailable"),
            "{refusal}"
        );
    }

    /// The view's in-process lookup (no launch: `find` calls no git) agrees
    /// with the git-backed resolver.
    #[test]
    fn find_agrees_with_resolve() {
        let scratch = Scratch::new("pij-community-find");
        for start in [scratch.main.clone(), scratch.gov.join(".harness")] {
            assert_eq!(find(&start).unwrap(), resolve(&start).unwrap(), "{start:?}");
        }
        scratch.git(
            &scratch.main,
            &[
                "worktree",
                "remove",
                "--force",
                scratch.gov.to_str().unwrap(),
            ],
        );
        let refusal = find(&scratch.main).unwrap_err();
        assert_eq!(refusal.code, E_GOV_ABSENT);
        assert!(refusal.message.contains("found 0"), "{refusal}");
    }

    #[test]
    fn outside_any_repository_it_refuses_by_name() {
        let dir = pij_testkit::fresh_dir("pij-community-root-norepo");
        let refusal = resolve(&dir).unwrap_err();
        assert_eq!(refusal.code, E_GOV_ABSENT);
        assert_eq!(refusal.next.as_deref(), Some(BOOTSTRAP));
        let _ = std::fs::remove_dir_all(dir);
    }
}
