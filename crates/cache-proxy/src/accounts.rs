//! The second-human-account check (PRD 004 req-11, matrix row 19). The proxy's
//! loopback port is reachable from every account on the Mac, so a second human
//! account deserves a warning. `pij-rs cache-proxy status` runs it, and the
//! doctor row (s03) calls [`check`]. One `/usr/bin/dscl` launch per manual run;
//! never on a timer (blueprint k-6).

use std::process::{Command, Stdio};

/// macOS's directory tool, by absolute path.
pub const DSCL: &str = "/usr/bin/dscl";

/// The launch-counter name for the dscl read.
pub const SPAWN_OP: &str = "dscl.cache_proxy_accounts";

/// macOS gives human accounts uids from 501 up.
const FIRST_HUMAN_UID: i64 = 501;

/// The human accounts in `dscl . -list /Users UniqueID` output: uid 501 and up,
/// minus `_`-prefixed service accounts and `nobody`-style system names.
#[must_use]
pub fn human_accounts(listing: &str) -> Vec<String> {
    listing
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let name = fields.next()?;
            let uid: i64 = fields.next()?.parse().ok()?;
            let system = name.starts_with('_') || matches!(name, "nobody" | "root" | "daemon");
            (uid >= FIRST_HUMAN_UID && !system).then(|| name.to_string())
        })
        .collect()
}

/// The warning for `accounts`, if there is more than one.
#[must_use]
pub fn warning(accounts: &[String]) -> Option<String> {
    (accounts.len() > 1).then(|| {
        format!(
            "more than one human account on this Mac ({}): the cache proxy's loopback port is reachable from every account",
            accounts.join(", ")
        )
    })
}

/// Run `/usr/bin/dscl . -list /Users UniqueID` once and parse it.
///
/// # Errors
/// dscl is missing (not macOS) or failed.
pub fn check() -> Result<Vec<String>, String> {
    pij_core::spawns::record(SPAWN_OP);
    let output = Command::new(DSCL)
        .args([".", "-list", "/Users", "UniqueID"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map_err(|error| format!("{DSCL}: {error}"))?;
    if !output.status.success() {
        return Err(format!("{DSCL} failed ({})", output.status));
    }
    Ok(human_accounts(&String::from_utf8_lossy(&output.stdout)))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ONE_HUMAN: &str = "_amavisd                 83\n\
        _analyticsd              263\n\
        daemon                   1\n\
        printservice             500\n\
        alice                    501\n\
        nobody                   -2\n\
        root                     0\n";

    #[test]
    fn one_human_account_gives_no_warning() {
        let accounts = human_accounts(ONE_HUMAN);
        assert_eq!(accounts, ["alice"]);
        assert_eq!(warning(&accounts), None);
    }

    #[test]
    fn a_second_human_account_gives_the_warning() {
        let listing =
            format!("{ONE_HUMAN}guest-user               502\n_unknown 99999\nmalformed\n");
        let accounts = human_accounts(&listing);
        assert_eq!(accounts, ["alice", "guest-user"]);
        let warning = warning(&accounts).expect("two human accounts warn");
        assert!(warning.contains("alice, guest-user"), "{warning}");
        assert!(warning.contains("loopback port is reachable"), "{warning}");
    }
}
