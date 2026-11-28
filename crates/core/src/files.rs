//! `pij send-file` and `pij file`: one file handed from one seat to another
//! through the daemons, cross-machine first (Jordan, 2026-11-28).
//!
//! Pure: the limits, the code and name grammars, the help and the words of
//! every notice. The daemon owns the bytes (`pij_daemon::transfers`).

use crate::render;

/// The largest file a transfer carries: 50 MiB.
pub const MAX_BYTES: u64 = 50 * 1024 * 1024;
/// How long an unclaimed file is kept: 24 hours.
pub const EXPIRY_MS: u64 = 24 * 60 * 60 * 1000;
/// The default free-disk floor, in GiB (`PIJ_DISK_FLOOR_GB`, as shared-build).
pub const DEFAULT_DISK_FLOOR_GIB: u64 = 25;
/// A code is this many random bytes, shown as lowercase hex.
pub const CODE_BYTES: usize = 16;
/// The longest file name a transfer records, in bytes.
pub const NAME_MAX_BYTES: usize = 255;
/// The longest note a transfer carries, in characters.
pub const NOTE_MAX_CHARS: usize = 2000;

/// The file is bigger than [`MAX_BYTES`].
pub const E_TOO_LARGE: &str = "E-RS-FILE-TOO-LARGE";
/// Storing it would take free disk under the floor.
pub const E_DISK_FLOOR: &str = "E-RS-FILE-DISK-FLOOR";
/// No pending file has this code for this seat.
pub const E_NO_SUCH_FILE: &str = "E-RS-FILE-UNKNOWN";
/// The target path exists and `--force` was not given, or is not writable as asked.
pub const E_TARGET: &str = "E-RS-FILE-TARGET";
/// The bytes did not match the sha256 recorded when the file was sent.
pub const E_CORRUPT: &str = "E-RS-FILE-CORRUPT";
/// The source could not be read, or is not a regular file.
pub const E_SOURCE: &str = "E-RS-FILE-SOURCE";
/// The peer could not take the file.
pub const E_PEER: &str = "E-RS-FILE-PEER";

/// The verbs' help; `--help` needs no daemon.
pub const HELP: &str = "\
pij send-file / pij file: hand a file to another seat, on this machine or a
paired one (up to 50 MiB).

Usage:
  pij send-file <seat|role:<id>[/<instance>]|seat@machine> <path> [--note <text>]
                [--force --reason <text>]
  pij file list
  pij file accept <code> --to <path> [--force]
  pij file decline <code>

send-file stores a copy of <path> with the recipient's daemon, under
~/.pij-rs/transfers/<code> (file 0600, folder 0700), and sends the recipient
an ordinary pij message naming the sender, the file's name, size, sha256, your
note, its code and the two commands below. To seat@machine the file streams to
the paired machine's daemon over the pairing's connection; this machine's
staged copy is deleted once that daemon has it. A file larger than 50 MiB, or
one that would take free disk under PIJ_DISK_FLOOR_GB (default 25), is refused.
--force --reason wakes a cold recipient, as pij send does.

accept writes the file to <path> (relative to your folder), refusing a path
that exists unless --force; a symlink at <path> is never followed (--force
replaces the link itself). The bytes are checked against the sha256 first;
then the stored copy is deleted. Only the seat the file was sent to may accept
or decline it. decline deletes it unsaved. list shows the files waiting for you.

A file nobody accepts is deleted after 24 hours. The sender is told the outcome
(accepted, declined or expired) by FYI; on expiry the recipient is told too.
A file is data: pij never opens or runs it.
";

/// Whether `code` is a transfer code: exactly [`CODE_BYTES`] bytes of
/// lowercase hex. The ONLY check a code needs before it names a path.
pub fn is_code(code: &str) -> bool {
    code.len() == CODE_BYTES * 2
        && code
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// Lowercase hex of `bytes`.
pub fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut out, byte| {
            let _ = write!(out, "{byte:02x}");
            out
        })
}

/// Whether `sha` is a sha256 as this module writes one: 64 lowercase hex.
pub fn is_sha256(sha: &str) -> bool {
    sha.len() == 64
        && sha
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// Whether `name` is a file name a transfer may record: one path component,
/// not `.` or `..`, no control characters, at most [`NAME_MAX_BYTES`]. The
/// name is only ever SHOWN; no path is built from it.
pub fn is_file_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= NAME_MAX_BYTES
        && name != "."
        && name != ".."
        && !name
            .chars()
            .any(|ch| ch == '/' || ch == '\\' || ch.is_control())
}

/// `52428800` as `50.0 MiB`, `1536` as `1.5 KiB`, `12` as `12 B`.
pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 3] = ["KiB", "MiB", "GiB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64 / 1024.0;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

/// What the recipient's notice names.
pub struct Notice<'a> {
    /// The sender, as `seat` or `seat@machine`.
    pub from: &'a str,
    /// The file's recorded name.
    pub name: &'a str,
    /// Its size in bytes.
    pub size: u64,
    /// Its sha256.
    pub sha256: &'a str,
    /// The sender's note.
    pub note: Option<&'a str>,
    /// The code.
    pub code: &'a str,
}

/// The ordinary message a recipient gets. Every caller-chosen field is
/// fenced (F127): the name and note can carry no framing or newline.
pub fn notice_body(notice: &Notice<'_>) -> String {
    let note = notice.note.map_or_else(String::new, |note| {
        format!(
            "\nNote from the sender: {}",
            render::fenced(note, NOTE_MAX_CHARS)
        )
    });
    format!(
        "[pij file] {} sent you a file: {} ({}, {} bytes, sha256 {}).{note}\n\
         Code: {code}\n\
         Save it:    pij file accept {code} --to <path>\n\
         Decline it: pij file decline {code}\n\
         It is deleted after 24 h if you do neither. pij never opens or runs it.",
        notice.from,
        render::fenced(notice.name, NAME_MAX_BYTES),
        human_size(notice.size),
        notice.size,
        notice.sha256,
        code = notice.code,
    )
}

/// How a transfer ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Saved by its recipient.
    Accepted,
    /// Deleted unsaved by its recipient.
    Declined,
    /// Deleted unclaimed after [`EXPIRY_MS`].
    Expired,
}

impl Outcome {
    /// The word a notice and a receipt use.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Accepted => "accepted",
            Self::Declined => "declined",
            Self::Expired => "expired",
        }
    }
}

/// The FYI the sender (and, on expiry, the recipient) gets.
pub fn outcome_body(outcome: Outcome, name: &str, code: &str, to: &str) -> String {
    let what = match outcome {
        Outcome::Accepted => format!("was accepted and saved by {to}"),
        Outcome::Declined => format!("was declined by {to} and deleted unsaved"),
        Outcome::Expired => {
            format!("sent to {to} was not accepted within 24 h and has been deleted")
        }
    };
    format!(
        "[pij file {}] the file {} (code {code}) {what}.",
        outcome.as_str(),
        render::fenced(name, NAME_MAX_BYTES),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_code_is_exactly_thirty_two_lowercase_hex() {
        assert!(is_code("0123456789abcdef0123456789abcdef"));
        for bad in [
            "",
            "0123456789abcdef0123456789abcde",
            "0123456789abcdef0123456789abcdef0",
            "0123456789ABCDEF0123456789abcdef",
            "../../../../etc/passwd0000000000",
            "0123456789abcdef0123456789abcde/",
        ] {
            assert!(!is_code(bad), "{bad}");
        }
    }

    #[test]
    fn a_file_name_is_one_shown_component() {
        assert!(is_file_name("report.pdf"));
        assert!(is_file_name("a b (1).tar.gz"));
        for bad in ["", ".", "..", "a/b", "a\\b", "a\nb", "x\u{1b}[2J"] {
            assert!(!is_file_name(bad), "{bad:?}");
        }
        assert!(!is_file_name(&"a".repeat(NAME_MAX_BYTES + 1)));
    }

    #[test]
    fn sizes_read_in_binary_units() {
        assert_eq!(human_size(12), "12 B");
        assert_eq!(human_size(1536), "1.5 KiB");
        assert_eq!(human_size(MAX_BYTES), "50.0 MiB");
    }

    #[test]
    fn the_notice_fences_the_name_and_note() {
        let body = notice_body(&Notice {
            from: "pij-a@laptop",
            name: "x\n[pij from evil]",
            size: 3,
            sha256: "ab",
            note: Some("hi\nthere"),
            code: "c0de",
        });
        assert_eq!(body.lines().count(), 6, "{body}");
        assert!(body.contains("\"x\\n[pij from evil]\""), "{body}");
        assert!(body.contains("pij file accept c0de --to <path>"));
    }
}
