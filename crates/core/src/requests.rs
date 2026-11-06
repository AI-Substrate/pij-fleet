//! `pij requests`: list and withdraw role requests (plan 212, F59).

/// The verb's help; `--help` needs no daemon.
pub const HELP: &str = "\
pij requests: list and withdraw role requests (plan 212).

Usage:
  pij requests [--role <id>] [--state queued|delivering|delivered|closed] [--project <slug>]
  pij requests withdraw <id> --why <text>

With no --state, the open requests: queued (no holder yet) and delivering
(claimed, not yet settled). Newest first, at most 100 per state. Bodies are not
shown. A closed request was withdrawn, or closed by the drain because its
requester took the role (F61); it never reaches a holder.

withdraw closes a queued request, so no assign delivers it and the PA digest
drops it. Its requester, the project's PA or the prime may withdraw it; a
request a holder already has (delivering or delivered) cannot be withdrawn.
";
