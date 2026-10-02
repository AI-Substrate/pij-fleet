//! The toolchain assert — gate stage 1 (workshop 001 R6a as amended).
//!
//! `rust-toolchain.toml` is a rustup mechanism, and a cargo that is not a rustup
//! shim ignores it **silently**. On the machine this port was built on, Homebrew's
//! rust 1.95.0 shadowed rustup's 1.98.0 on `PATH`, so the compiler and the linter
//! were already different versions before a line of code existed. A pin nobody
//! enforces is a comment.
//!
//! So the pin is read as DATA and compared against the compiler that is actually
//! running. Tenet 10's hazard — one toolchain shared by every unit, no edit-time
//! signal when it moves — becomes a red gate instead of a silent divergence.

use std::path::{Path, PathBuf};
use std::process::Command;

/// The version `rust-toolchain.toml` pins, read from the file rather than from a
/// constant that could drift away from it.
///
/// # Errors
/// A message naming what could not be read or parsed.
pub fn pinned_channel(workspace_root: &Path) -> Result<String, String> {
    let path = workspace_root.join("rust-toolchain.toml");
    let text = std::fs::read_to_string(&path)
        .map_err(|error| format!("could not read {}: {error}", path.display()))?;

    // `toml::from_str`, not `text.parse()`: in toml 0.9 the `FromStr` impl on
    // `Value` parses a VALUE, so a whole document (comments and all) fails at
    // line 1 column 1 with "unexpected content" — which reads like the file is
    // corrupt rather than like the wrong API was called. The gate caught this on
    // its first run against a file that is perfectly valid TOML.
    let value: toml::Value = toml::from_str(&text)
        .map_err(|error| format!("{} is not valid TOML: {error}", path.display()))?;

    value
        .get("toolchain")
        .and_then(|toolchain| toolchain.get("channel"))
        .and_then(toml::Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("{} has no [toolchain] channel", path.display()))
}

/// The `rustc` this gate is running under, e.g. `1.98.0`.
///
/// # Errors
/// A message naming why rustc could not be asked.
pub fn running_rustc(cargo: &Path) -> Result<String, String> {
    // Ask the rustc that BELONGS to this cargo, not whatever `rustc` PATH finds:
    // the whole failure being defended against is a PATH that resolves two
    // different toolchains.
    let rustc = cargo.with_file_name("rustc");
    let output = Command::new(&rustc)
        .arg("--version")
        .output()
        .map_err(|error| format!("could not run {}: {error}", rustc.display()))?;

    let text = String::from_utf8_lossy(&output.stdout);
    text.split_whitespace()
        .nth(1)
        .map(str::to_string)
        .ok_or_else(|| format!("could not read a version out of `{}`", text.trim()))
}

/// The cargo this gate should shell out to.
///
/// `$CARGO` when cargo invoked us — which is the rustup shim's resolution, not
/// PATH's — and the rustup shim by absolute path otherwise. Never a bare `cargo`,
/// per the prime's Q1 ruling.
pub fn cargo_path() -> PathBuf {
    if let Some(cargo) = std::env::var_os("CARGO") {
        return PathBuf::from(cargo);
    }
    if let Some(home) = std::env::var_os("HOME") {
        let shim = PathBuf::from(home).join(".cargo/bin/cargo");
        if shim.exists() {
            return shim;
        }
    }
    PathBuf::from("cargo")
}

/// Compare the running compiler against the pin.
///
/// # Errors
/// A message stating both versions and what to do — a mismatch here is not a
/// warning, because every downstream green becomes meaningless.
pub fn assert_pinned(workspace_root: &Path, cargo: &Path) -> Result<String, String> {
    let pinned = pinned_channel(workspace_root)?;
    let running = running_rustc(cargo)?;

    if running == pinned {
        return Ok(running);
    }

    Err(format!(
        "toolchain mismatch: rust-toolchain.toml pins {pinned}, but this gate is running rustc \
         {running}.\n  The pin is a RUSTUP mechanism — a cargo that is not a rustup shim ignores \
         it silently, which is exactly how this machine ran a 1.95.0 compiler with 1.98.0 \
         clippy.\n  Fix: put ~/.cargo/bin ahead of any other rust on PATH, or run the gate as \
         `~/.cargo/bin/cargo run -p pij-testkit --bin pij-gate`."
    ))
}
