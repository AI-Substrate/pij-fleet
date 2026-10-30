//! Which tap sinks one tmux server's taps own (plan 185, F5).
//!
//! A sink's name is only its pane id, and pane ids are per server, so a file in
//! the tap root says nothing about which server wrote it. Each adapter records
//! every sink it creates in its own server's ledger, before the sink exists. The
//! orphan sweep then only ever considers its own ledger: a default-server
//! observer cannot remove a `-L` server's tap when the two daemons have distinct
//! state dirs. In one shared tap root the same pane id is the same file in both
//! ledgers, so ownership by name cannot separate them.

use std::collections::BTreeSet;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

const LEDGER_DIR: &str = ".owners";
/// Written after the default server's one-time claim of pre-ledger sinks.
const LEGACY_CLAIMED: &str = "legacy-claimed";

/// One server's durable list of sink file names, one per line.
pub(crate) struct TapLedger {
    dir: PathBuf,
    path: PathBuf,
}

impl TapLedger {
    pub(crate) fn for_server(tap_root: &Path, server: Option<&str>) -> Self {
        let dir = tap_root.join(LEDGER_DIR);
        let name = match server {
            None => "default.list".to_string(),
            Some(server) => format!("L-{}.list", file_safe(server)),
        };
        Self {
            path: dir.join(name),
            dir,
        }
    }

    /// Whether the one-time claim of pre-ledger sinks has completed here.
    pub(crate) fn legacy_claimed(&self) -> bool {
        self.dir.join(LEGACY_CLAIMED).is_file()
    }

    pub(crate) fn mark_legacy_claimed(&self) -> io::Result<()> {
        fs::create_dir_all(&self.dir)?;
        fs::write(self.dir.join(LEGACY_CLAIMED), "")
    }

    /// Names listed by every OTHER server's ledger in this tap root.
    pub(crate) fn others(&self) -> io::Result<BTreeSet<String>> {
        let entries = match fs::read_dir(&self.dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(BTreeSet::new()),
            Err(error) => return Err(error),
        };
        let mut names = BTreeSet::new();
        for entry in entries {
            let path = entry?.path();
            if path == self.path || path.extension().and_then(|ext| ext.to_str()) != Some("list") {
                continue;
            }
            let other = Self {
                dir: self.dir.clone(),
                path,
            };
            names.extend(other.read()?);
        }
        Ok(names)
    }

    pub(crate) fn read(&self) -> io::Result<BTreeSet<String>> {
        match fs::read_to_string(&self.path) {
            Ok(text) => Ok(text
                .lines()
                .filter(|line| !line.is_empty())
                .map(str::to_string)
                .collect()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(BTreeSet::new()),
            Err(error) => Err(error),
        }
    }

    /// Add `name` unless it is already listed.
    pub(crate) fn record(&self, name: &str) -> io::Result<()> {
        if self.read()?.contains(name) {
            return Ok(());
        }
        fs::create_dir_all(&self.dir)?;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        writeln!(file, "{name}")
    }

    /// Replace the ledger atomically.
    pub(crate) fn rewrite(&self, names: &BTreeSet<String>) -> io::Result<()> {
        fs::create_dir_all(&self.dir)?;
        let staged = self.path.with_extension("list.tmp");
        let mut text = String::new();
        for name in names {
            text.push_str(name);
            text.push('\n');
        }
        fs::write(&staged, text)?;
        fs::rename(&staged, &self.path)
    }
}

fn file_safe(text: &str) -> String {
    text.chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_server_keeps_its_own_ledger_and_records_once() {
        let root = std::env::temp_dir().join(format!(
            "pij-tap-ledger-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let default = TapLedger::for_server(&root, None);
        let private = TapLedger::for_server(&root, Some("pij test/1"));
        assert!(!default.legacy_claimed());
        default.record("_1.raw").unwrap();
        default.record("_1.raw").unwrap();
        private.record("_1.raw").unwrap();
        private.record("_2.raw").unwrap();
        assert!(!default.legacy_claimed(), "recording is not claiming");
        assert_eq!(
            default.others().unwrap(),
            BTreeSet::from(["_1.raw".to_string(), "_2.raw".to_string()])
        );
        default.mark_legacy_claimed().unwrap();
        assert!(default.legacy_claimed());
        assert_eq!(default.read().unwrap().len(), 1, "recorded once");
        assert_eq!(
            fs::read_dir(root.join(LEDGER_DIR))
                .unwrap()
                .filter(|entry| entry.as_ref().unwrap().path().extension().is_some())
                .count(),
            2,
            "one ledger per server"
        );
        default.rewrite(&BTreeSet::new()).unwrap();
        assert!(default.read().unwrap().is_empty());
        assert_eq!(
            private.read().unwrap().len(),
            2,
            "the other server's is untouched"
        );
        let _ = fs::remove_dir_all(&root);
    }
}
