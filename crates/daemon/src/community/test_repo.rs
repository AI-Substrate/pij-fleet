//! Scratch git repositories for the community tests: a `main` clone and a
//! standing `prime-governance` worktree, never the live governance branch.

use std::path::{Path, PathBuf};
use std::process::Command;

pub(crate) struct Scratch {
    pub root: PathBuf,
    pub main: PathBuf,
    pub gov: PathBuf,
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

impl Scratch {
    /// A repo with one commit on main and no governance branch.
    pub fn bare_main(name: &str) -> Self {
        let root = pij_testkit::fresh_dir(name)
            .canonicalize()
            .expect("canonical root");
        let main = root.join("main");
        let gov = root.join("gov");
        std::fs::create_dir_all(&main).unwrap();
        let scratch = Self { root, main, gov };
        let main = scratch.main.clone();
        scratch.git(&main, &["init", "-q", "-b", "main"]);
        for (key, value) in [
            ("user.name", "scratch"),
            ("user.email", "scratch@example.invalid"),
            ("commit.gpgsign", "false"),
            ("core.hooksPath", "/dev/null"),
        ] {
            scratch.git(&main, &["config", key, value]);
        }
        std::fs::write(main.join("README.md"), "main\n").unwrap();
        scratch.git(&main, &["add", "README.md"]);
        scratch.git(&main, &["commit", "-q", "-m", "main"]);
        scratch
    }

    /// [`Self::bare_main`] plus an orphan `prime-governance` branch checked
    /// out in a standing worktree with `.harness/government/README.md`.
    pub fn new(name: &str) -> Self {
        let scratch = Self::bare_main(name);
        let main = scratch.main.clone();
        let tree = scratch.git(&main, &["mktree"]);
        let commit = scratch.git(&main, &["commit-tree", tree.trim(), "-m", "governance"]);
        scratch.git(
            &main,
            &["update-ref", "refs/heads/prime-governance", commit.trim()],
        );
        scratch.git(
            &main,
            &[
                "worktree",
                "add",
                "-q",
                scratch.gov.to_str().unwrap(),
                "prime-governance",
            ],
        );
        let government = scratch.gov.join(".harness/government");
        std::fs::create_dir_all(&government).unwrap();
        std::fs::write(government.join("README.md"), "government\n").unwrap();
        scratch.git(&scratch.gov, &["add", ".harness/government/README.md"]);
        scratch.git(&scratch.gov, &["commit", "-q", "-m", "government"]);
        scratch
    }

    /// Run git in `dir`, asserting success; returns stdout.
    pub fn git(&self, dir: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env_remove("GIT_DIR")
            .env_remove("GIT_INDEX_FILE")
            .output()
            .expect("run git");
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).expect("utf-8")
    }
}
