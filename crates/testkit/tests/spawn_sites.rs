//! Every process the daemon launches is counted (plan 180, follow-up C).
//!
//! `pij-rs ping` reports `spawns: {total, by_op}` from `pij_core::spawns`, and
//! that total is only honest if every launch site records itself. This scans
//! the crates the daemon runs and fails when a function launches a process
//! without calling `spawns::record`, naming the file and function.
//!
//! A function launches a process when it builds a `Command` (`Command::new(`)
//! or calls a builder defined in the same file (a `fn ... -> Command`, like the
//! tmux adapters' `command()`). The builder itself is not a site: its callers
//! record. The CLI crate is not scanned: its launches happen in short-lived CLI
//! processes, not the daemon.

use std::path::{Path, PathBuf};

const DAEMON_CRATES: [&str; 8] = [
    "core",
    "daemon",
    "harnesses",
    "sidecars",
    "store",
    "tmux",
    "transport",
    "unisphere",
];

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("read a source dir") {
        let path = entry.expect("a dir entry").path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

/// The production part of a source file: test-only files are skipped and an
/// inline `#[cfg(test)] mod` ends the scan.
fn production_lines(path: &Path) -> Vec<String> {
    let name = path.file_name().unwrap().to_string_lossy();
    if name.contains("test") {
        return Vec::new();
    }
    let lines: Vec<String> = std::fs::read_to_string(path)
        .expect("read a source file")
        .lines()
        .map(str::to_string)
        .collect();
    let end = lines
        .windows(2)
        .position(|pair| {
            pair[0].trim() == "#[cfg(test)]"
                && (pair[1].starts_with("mod ") || pair[1].starts_with("pub mod "))
        })
        .unwrap_or(lines.len());
    lines[..end].to_vec()
}

fn fn_name(line: &str) -> Option<String> {
    let trimmed = line.trim_start();
    let rest = [
        "pub(crate) ",
        "pub(super) ",
        "pub ",
        "const ",
        "async ",
        "unsafe ",
    ]
    .iter()
    .fold(trimmed, |rest, prefix| {
        rest.strip_prefix(prefix).unwrap_or(rest)
    });
    let name = rest.strip_prefix("fn ")?;
    Some(
        name.chars()
            .take_while(|ch| ch.is_alphanumeric() || *ch == '_')
            .collect(),
    )
}

struct LaunchSite {
    file: String,
    function: String,
    recorded: bool,
}

/// Each function (from one `fn` line to the next) that launches a process.
fn launch_sites(path: &Path, root: &Path) -> Vec<LaunchSite> {
    let lines = production_lines(path);
    let starts: Vec<usize> = (0..lines.len())
        .filter(|&index| fn_name(&lines[index]).is_some())
        .collect();
    let functions: Vec<(String, String, bool)> = starts
        .iter()
        .enumerate()
        .map(|(position, &start)| {
            let end = starts.get(position + 1).copied().unwrap_or(lines.len());
            let signature = lines[start..end]
                .iter()
                .position(|line| line.contains('{'))
                .map_or(String::new(), |open| lines[start..=start + open].join(" "));
            (
                fn_name(&lines[start]).unwrap(),
                lines[start..end].join("\n"),
                signature.contains("-> Command"),
            )
        })
        .collect();
    // Builders defined in this file (`fn command(&self) -> Command`): calling
    // one launches a process, so its callers are launch sites.
    let builder_calls: Vec<String> = functions
        .iter()
        .filter(|(_, _, builder)| *builder)
        .map(|(name, _, _)| format!(".{name}()"))
        .collect();
    let mut sites = Vec::new();
    for (name, body, builder) in &functions {
        let launches = (body.contains("Command::new(") && !builder)
            || builder_calls
                .iter()
                .any(|call| body.contains(call.as_str()));
        if launches {
            sites.push(LaunchSite {
                file: path.strip_prefix(root).unwrap().display().to_string(),
                function: name.clone(),
                recorded: body.contains("spawns::record("),
            });
        }
    }
    sites
}

#[test]
fn every_daemon_launch_site_records_itself() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let root = root.canonicalize().expect("the workspace root");
    let mut files = Vec::new();
    for krate in DAEMON_CRATES {
        rust_files(&root.join("crates").join(krate).join("src"), &mut files);
    }
    let sites: Vec<LaunchSite> = files
        .iter()
        .flat_map(|path| launch_sites(path, &root))
        .collect();

    // Guard against a vacuous pass (a moved tree scans nothing).
    let known = [
        ("crates/harnesses/src/proc.rs", "run"),
        ("crates/tmux/src/lib.rs", "run"),
        ("crates/tmux/src/control.rs", "start"),
        ("crates/daemon/src/http/identity.rs", "peer_pid"),
    ];
    for (file, function) in known {
        assert!(
            sites
                .iter()
                .any(|site| site.file == file && site.function == function),
            "the scan no longer finds the launch in {file}::{function}; fix the scanner"
        );
    }
    assert!(sites.len() >= 20, "only {} launch sites found", sites.len());

    let unrecorded: Vec<String> = sites
        .iter()
        .filter(|site| !site.recorded)
        .map(|site| format!("{}::{}", site.file, site.function))
        .collect();
    assert!(
        unrecorded.is_empty(),
        "these functions launch a process without `pij_core::spawns::record(\"<program>.<site>\")`, \
         so `pij-rs ping` would under-count them:\n  {}",
        unrecorded.join("\n  ")
    );
}

/// Plan 180, follow-up D: a bare `ps` or `date` makes macOS `posix_spawnp` try
/// every PATH entry, and each miss is a kernel process (18 per launch with the
/// daemon's PATH). The daemon names them by absolute path instead.
#[test]
fn no_daemon_launch_searches_path_for_ps_or_date() {
    const BARE: [&str; 2] = ["Command::new(\"ps\")", "Command::new(\"date\")"];
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let root = root.canonicalize().expect("the workspace root");
    let mut files = Vec::new();
    for krate in DAEMON_CRATES {
        rust_files(&root.join("crates").join(krate).join("src"), &mut files);
    }
    let mut absolute = 0;
    let mut bare = Vec::new();
    for path in &files {
        for (index, line) in production_lines(path).iter().enumerate() {
            absolute += line.matches("Command::new(\"/bin/ps\")").count();
            if BARE.iter().any(|pattern| line.contains(pattern)) {
                bare.push(format!(
                    "{}:{}: {}",
                    path.strip_prefix(&root).unwrap().display(),
                    index + 1,
                    line.trim()
                ));
            }
        }
    }
    // Guard against a vacuous pass: the absolute-path launches must be visible.
    assert!(
        absolute >= 4,
        "found only {absolute} `/bin/ps` launches; fix the scanner"
    );
    assert!(
        bare.is_empty(),
        "these launches search PATH (one kernel process per PATH miss on macOS); use \
         \"/bin/ps\" or \"/bin/date\":\n  {}",
        bare.join("\n  ")
    );
}
