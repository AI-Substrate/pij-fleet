//! `pij requests`: list, withdraw and close role requests (plan 212, F59, F83).

/// The verb's help; `--help` needs no daemon.
pub const HELP: &str = "\
pij requests: list, withdraw and close role requests (plan 212).

Usage:
  pij requests [--role <id>] [--state queued|delivering|delivered|closed] [--project <slug>]
  pij requests withdraw <id> --why <text>
  pij requests close <id> [--why <text>] [--quiet]

With no --state, the open requests: queued (no holder yet) and delivering
(claimed, not yet settled). Newest first, at most 100 per state. Bodies are not
shown. A closed request was withdrawn, closed by the drain because its
requester took the role (F61), or closed after delivery. A delivered request
marked (answered) was answered by its holder's reply (--in-reply-to <id>) or
by pij output --answers: its time limit has stopped, and it stays delivered
until someone closes it (F176).

withdraw closes a queued request, so no assign delivers it and the PA digest
drops it. Its requester, the project's PA or the prime may withdraw it; a
request a holder already has (delivering or delivered) cannot be withdrawn.

close closes a delivered request that was answered or abandoned (F83), which
stops its time limit: no more check-ins or escalations. Its requester, the
project's PA, the prime or its holder may close it. The requester is told
once by FYI, unless it closed the request itself or --quiet is given.
";
