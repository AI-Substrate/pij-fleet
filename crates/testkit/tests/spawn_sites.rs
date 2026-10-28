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

/// The function a line declares, whatever its attributes, visibility and
/// qualifiers: `#[…]`, `pub`, `pub(…)` of any kind, `const`, `async`, `unsafe`
/// and `extern "…"`, in any order. A form this misses would merge its body
/// into the previous function's segment (review of #84, S1).
fn fn_name(line: &str) -> Option<String> {
    let mut rest = line.trim_start();
    loop {
        if let Some(attribute) = rest.strip_prefix("#[") {
            rest = attribute.split_once(']')?.1.trim_start();
        } else if let Some(scoped) = rest.strip_prefix("pub(") {
            rest = scoped.split_once(')')?.1.trim_start();
        } else if let Some(abi) = rest.strip_prefix("extern \"") {
            rest = abi.split_once('"')?.1.trim_start();
        } else if let Some(next) = ["pub ", "const ", "async ", "unsafe ", "extern "]
            .iter()
            .find_map(|prefix| rest.strip_prefix(prefix))
        {
            rest = next.trim_start();
        } else {
            break;
        }
    }
    let name = rest.strip_prefix("fn ")?;
    Some(
        name.chars()
            .take_while(|ch| ch.is_alphanumeric() || *ch == '_')
            .collect(),
    )
}

/// Whether the function declared at `start` is `#[cfg(test)]`, by an attribute
/// on its own line or the same one.
fn test_only(lines: &[String], start: usize) -> bool {
    lines[start].trim_start().starts_with("#[cfg(test)]")
        || lines[..start]
            .iter()
            .rev()
            .take_while(|line| {
                let line = line.trim_start();
                line.starts_with("///") || line.starts_with("#[")
            })
            .any(|line| line.trim() == "#[cfg(test)]")
}

/// The line holding the closing brace of the function declared at `start`.
/// `None` when the braces never balance, so the caller scans the whole segment.
fn closing_line(lines: &[String], start: usize, end: usize) -> Option<usize> {
    let mut depth = 0_i32;
    let mut opened = false;
    for (index, line) in lines.iter().enumerate().take(end).skip(start) {
        for ch in line.chars() {
            match ch {
                '{' => {
                    depth += 1;
                    opened = true;
                }
                '}' => depth -= 1,
                _ => {}
            }
            if opened && depth == 0 {
                return Some(index);
            }
        }
    }
    None
}

struct LaunchSite {
    file: String,
    function: String,
    recorded: bool,
}

/// Each function (from one `fn` line to the next) that launches a process.
fn launch_sites(path: &Path, root: &Path) -> Vec<LaunchSite> {
    let file = path.strip_prefix(root).unwrap().display().to_string();
    sites_in(&file, &production_lines(path))
}

fn sites_in(file: &str, lines: &[String]) -> Vec<LaunchSite> {
    let starts: Vec<usize> = (0..lines.len())
        .filter(|&index| fn_name(&lines[index]).is_some())
        .collect();
    // Each segment runs from one `fn` line to the next. A `#[cfg(test)]`
    // function (an accessor a test reads) never runs in the daemon, so its own
    // body is skipped, but only up to its closing brace: whatever follows it in
    // the segment is still production code and is scanned under its own name.
    let mut segments: Vec<(String, String, bool)> = Vec::new();
    for (position, &start) in starts.iter().enumerate() {
        let end = starts.get(position + 1).copied().unwrap_or(lines.len());
        let name = fn_name(&lines[start]).unwrap();
        let from = if test_only(lines, start) {
            match closing_line(lines, start, end) {
                Some(close) if close + 1 < end => {
                    segments.push((
                        format!("<after cfg(test) fn {name}>"),
                        lines[close + 1..end].join("\n"),
                        false,
                    ));
                    continue;
                }
                Some(_) => continue,
                None => start,
            }
        } else {
            start
        };
        let signature = lines[from..end]
            .iter()
            .position(|line| line.contains('{'))
            .map_or(String::new(), |open| lines[from..=from + open].join(" "));
        segments.push((
            name,
            lines[from..end].join("\n"),
            signature.contains("-> Command"),
        ));
    }
    // Builders defined in this file (`fn command(&self, …) -> Command`): calling
    // one launches a process, so its callers are launch sites.
    let builder_calls: Vec<String> = segments
        .iter()
        .filter(|(_, _, builder)| *builder)
        .map(|(name, _, _)| format!(".{name}("))
        .collect();
    let mut sites = Vec::new();
    for (name, body, builder) in &segments {
        let launches = (body.contains("Command::new(") && !builder)
            || builder_calls
                .iter()
                .any(|call| body.contains(call.as_str()));
        if launches {
            sites.push(LaunchSite {
                file: file.to_string(),
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

/// Plan 182 (pij-fleet#74): `git` and `tmux` live wherever the user installed
/// them, so they cannot be named by a fixed path. The daemon launches them
/// through `pij_core::program::launch`, which resolves PATH once and launches
/// the path it found. A bare name would search PATH again on every launch (12
/// kernel processes per `git` or `tmux` launch with the daemon's PATH on macOS).
///
/// A TRIPWIRE, NOT A PROOF: it matches literal source text only. A bare launch
/// spelled any other way (`Command::new(binary)`, `Command::new(TMUX)`,
/// `Command::new(OsStr::new("git"))`) passes it. What it does catch is the shape
/// every one of these launches had before plan 182.
#[test]
fn no_daemon_launch_searches_path_for_git_or_tmux() {
    const BARE: [&str; 3] = [
        "Command::new(\"git\")",
        "Command::new(\"tmux\")",
        // The tmux adapters' configurable binary, launched unresolved.
        "Command::new(&self.binary)",
    ];
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let root = root.canonicalize().expect("the workspace root");
    let mut files = Vec::new();
    for krate in DAEMON_CRATES {
        rust_files(&root.join("crates").join(krate).join("src"), &mut files);
    }
    let mut resolved = 0;
    let mut bare = Vec::new();
    for path in &files {
        for (index, line) in production_lines(path).iter().enumerate() {
            resolved += line.matches("pij_core::program::launch").count();
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
    assert!(
        bare.is_empty(),
        "these launches search PATH for git or tmux on every launch; use \
         `pij_core::program::launch(\"git\", |git| Command::new(git)...)`:\n  {}",
        bare.join("\n  ")
    );
    // Guard against a vacuous pass: three git launches and three tmux launches
    // (`run`, `run_with_stdin`, the control client's `start`).
    assert!(
        resolved >= 6,
        "found only {resolved} resolved git/tmux launches; fix the scanner"
    );
}

/// Review of #84, S1: skipping a `#[cfg(test)]` function must never hide a
/// production launch after it. Each case puts an unrecorded launch right after a
/// test-only accessor, in a form the scanner might not read as its own `fn`.
#[test]
fn a_cfg_test_function_cannot_hide_a_launch() {
    // (source, the name the unrecorded launch must be reported under)
    let cases = [
        // The reviewer's injected case: `fn_name` must read every fn form.
        (
            "#[cfg(test)] fn test_accessor() {}\n\
             pub(in crate::orchestration) fn launch_unrecorded() { let _ = Command::new(\"/bin/ls\").output(); }",
            "launch_unrecorded",
        ),
        (
            "#[cfg(test)]\nfn accessor() {}\npub(self) fn launch() {\n    let _ = Command::new(\"/bin/ls\").output();\n}",
            "launch",
        ),
        (
            "#[cfg(test)]\nfn accessor() {}\nextern \"C\" fn launch() {\n    let _ = Command::new(\"/bin/ls\").output();\n}",
            "launch",
        ),
        (
            "#[cfg(test)]\nfn accessor() {}\npub(crate) const unsafe extern \"C\" fn launch() {\n    let _ = Command::new(\"/bin/ls\").output();\n}",
            "launch",
        ),
        // No `fn` line at all: only the accessor's closing brace can end its segment.
        (
            "#[cfg(test)]\nfn accessor() {\n    let _ = 1;\n}\nconst LAUNCH: fn() = || {\n    let _ = Command::new(\"/bin/ls\").status();\n};",
            "<after cfg(test) fn accessor>",
        ),
    ];
    for (source, expected) in cases {
        let lines: Vec<String> = source.lines().map(str::to_string).collect();
        let sites = sites_in("case.rs", &lines);
        let names: Vec<&str> = sites.iter().map(|site| site.function.as_str()).collect();
        assert!(
            sites
                .iter()
                .any(|site| !site.recorded && site.function == expected),
            "a cfg(test) accessor hid or misattributed this launch (expected {expected}):\n{source}\nsites: {names:?}"
        );
    }
}
