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

/// Where a folder sits in git, read from the files git itself reads: no
/// launch (F202).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Place {
    /// The repository's canonical common git directory, shared by every
    /// worktree of one repository.
    Repo(PathBuf),
    /// The folder is missing, or nothing from it up to `/` is a repository.
    NotARepo,
    /// A layout this reader does not decide (a bare repository, a garbled or
    /// dangling `.git`, a `.git` directory with no `HEAD`): ask git.
    Unknown,
}

/// Walk up from `start` as git's discovery does: at each level a `.git`
/// directory is the common dir (main checkout), a `.git` file names the
/// gitdir (`gitdir: <path>`, relative to its folder), whose `commondir`
/// (relative to the gitdir) names the common dir if present. A level that is
/// itself a git directory (bare, or inside `.git`) is left to git. One
/// difference, stated: the walk does not stop at a filesystem boundary.
pub(crate) fn place(start: &Path) -> Place {
    if !start.is_dir() {
        return Place::NotARepo;
    }
    for level in start.ancestors() {
        let dotgit = level.join(".git");
        let Ok(meta) = std::fs::metadata(&dotgit) else {
            if looks_like_git_dir(level) {
                return Place::Unknown;
            }
            continue;
        };
        if meta.is_dir() {
            return if dotgit.join("HEAD").is_file() {
                Place::Repo(canonical(&dotgit))
            } else {
                Place::Unknown
            };
        }
        let Some(gitdir) = std::fs::read_to_string(&dotgit).ok().and_then(|text| {
            let named = text.trim().strip_prefix("gitdir:")?.trim().to_string();
            (!named.is_empty()).then(|| level.join(named))
        }) else {
            return Place::Unknown;
        };
        let common = match std::fs::read_to_string(gitdir.join("commondir")) {
            Ok(named) if !named.trim().is_empty() => gitdir.join(named.trim()),
            Ok(_) => return Place::Unknown,
            Err(_) => gitdir,
        };
        return if common.is_dir() {
            Place::Repo(canonical(&common))
        } else {
            Place::Unknown
        };
    }
    Place::NotARepo
}

/// A folder git would take as a git directory itself.
fn looks_like_git_dir(dir: &Path) -> bool {
    dir.join("HEAD").is_file() && dir.join("objects").is_dir() && dir.join("refs").is_dir()
}

/// The repository's common git directory, found by walking up from `start`.
pub(crate) fn common_dir(start: &Path) -> Option<PathBuf> {
    match place(start) {
        Place::Repo(dir) => Some(dir),
        Place::NotARepo | Place::Unknown => None,
    }
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

    fn git_says(scratch: &Scratch, dir: &Path) -> PathBuf {
        let printed = scratch.git(
            dir,
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        );
        PathBuf::from(printed.trim()).canonicalize().unwrap()
    }

    /// F202: the in-process read names the common dir git names, for the
    /// main checkout, a folder inside a worktree, and a linked worktree
    /// whose gitdir is absolute (as git writes it) or relative.
    #[test]
    fn place_names_the_common_dir_git_names() {
        let scratch = Scratch::new("pij-community-place");
        let common = scratch.main.join(".git");
        let inside = scratch.gov.join(".harness/government");
        for dir in [&scratch.main, &inside, &scratch.gov] {
            assert_eq!(place(dir), Place::Repo(common.clone()), "{dir:?}");
            assert_eq!(place(dir), Place::Repo(git_says(&scratch, dir)), "{dir:?}");
        }
        let pointer = std::fs::read_to_string(scratch.gov.join(".git")).unwrap();
        let gitdir = PathBuf::from(pointer.trim().strip_prefix("gitdir:").unwrap().trim());
        assert!(gitdir.is_absolute(), "git wrote {pointer}");
        let commondir = std::fs::read_to_string(gitdir.join("commondir")).unwrap();
        assert!(
            !Path::new(commondir.trim()).is_absolute(),
            "relative commondir: {commondir}"
        );

        std::fs::write(
            scratch.gov.join(".git"),
            "gitdir: ../main/.git/worktrees/gov\n",
        )
        .unwrap();
        assert_eq!(place(&scratch.gov), Place::Repo(common.clone()));
        assert_eq!(
            place(&scratch.gov),
            Place::Repo(git_says(&scratch, &scratch.gov))
        );
    }

    /// A folder no repository holds, or one that is gone, is no repository.
    /// Each layout the reader does not decide goes to git; none panics.
    #[test]
    fn place_leaves_what_it_cannot_read_to_git() {
        let scratch = Scratch::bare_main("pij-community-place-odd");
        let dir = |name: &str| {
            let path = scratch.root.join(name);
            std::fs::create_dir_all(&path).unwrap();
            path
        };
        assert_eq!(place(&dir("plain")), Place::NotARepo);
        assert_eq!(place(&scratch.root.join("gone")), Place::NotARepo);

        let gitfile = |name: &str, contents: &[u8]| {
            let path = dir(name);
            std::fs::write(path.join(".git"), contents).unwrap();
            path
        };
        let worktrees = scratch.main.join(".git/worktrees/half");
        std::fs::create_dir_all(&worktrees).unwrap();
        std::fs::write(worktrees.join("HEAD"), "ref: refs/heads/main\n").unwrap();
        std::fs::write(worktrees.join("commondir"), "../missing\n").unwrap();
        let undecided = [
            gitfile("garbled", b"not a pointer\n"),
            gitfile("empty-pointer", b"gitdir:   \n"),
            gitfile("not-utf8", &[0x67, 0xff, 0xfe]),
            gitfile("dangling", b"gitdir: ../nowhere\n"),
            gitfile(
                "dangling-commondir",
                format!("gitdir: {}\n", worktrees.display()).as_bytes(),
            ),
        ];
        for path in &undecided {
            assert_eq!(place(path), Place::Unknown, "{path:?}");
        }

        let hollow = dir("hollow");
        std::fs::create_dir_all(hollow.join(".git")).unwrap();
        assert_eq!(place(&hollow), Place::Unknown);

        let bare = scratch.root.join("bare.git");
        scratch.git(
            &scratch.root,
            &["init", "-q", "--bare", bare.to_str().unwrap()],
        );
        assert_eq!(place(&bare), Place::Unknown);
        assert_eq!(place(&bare.join("refs/heads")), Place::Unknown);
    }
}
