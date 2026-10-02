# pij telegram — drive your sessions from Telegram

## Rust daemon bridge

The Rust sidecar is opt-in via `PIJ_TELEGRAM_ENV` pointing to an explicit credential
file. Its outbound text begins with `[sender-seat] [folder-basename] `; absent seats
or folders use `[sender-seat] ` alone. It does not probe git or append a branch. Oversized context falls back to the bare sender tag.
An exact existing sender tag or complete prefix is stripped before prefixing.

Send literal text with `pij-rs sidecar telegram --body 'hello'`, or avoid shell
quoting with `pij-rs sidecar telegram --body-file ./message.txt`. Use `--body-file -`
to read stdin. File/stdin input preserves newlines and quotes exactly; `--body` and
`--body-file` are mutually exclusive.

Long messages become ordered `(1/n)` through `(n/n)` parts. Every bubble retains
the sender/context prefix; text, prefix and numbering together fit within 4000
UTF-16 units, below Telegram's 4096-character cap. Splitting prefers newlines,
then spaces, without dropping content or splitting Unicode characters.

Each failed HTTP/API attempt appends `telegram.outbound-delivery-failed` on the
conversation seat. Its JSON includes `conversation`, `from`, `msg_id`, `attempt`,
`part`, `parts`, `http_status` (null for transport failure), and a token-free `reason`.
The existing whole-job retry policy is unchanged: a late-part failure can resend
earlier bubbles. Numbering makes repeated parts recognizable; it is not deduplication.
Inbound still uses the persisted outbound binding, not name or swipe-reply parsing.

The API root is injectable in Rust tests, not via environment/credential files.
Tests use only local fake HTTP endpoints; an isolated daemon cannot currently
use a stub API root. Never use production credentials to fill that proof gap.
Outbound media is not implemented in the Rust sidecar.
