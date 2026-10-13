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

### Documents (file attachments)

Send a file as a Telegram document with
`pij-rs sidecar telegram --file ./report.html [--caption 'text']`, or through the
shim with `pij send pij-telegram --file ./report.html [--caption 'text']` (a
relative path resolves against the caller's cwd). `--file` excludes a body;
`--caption` requires `--file`. Only `pij-telegram` accepts `--file`; any other
recipient still refuses it by name.

- **Staged by content at enqueue.** The daemon copies the file into
  `<state>/sidecars/telegram-attachments/<sha256>-<msg-key>` (dir `0700`, file
  `0600`) while hashing it, and the job (`sidecar:telegram:document`) records the
  staged path, size and SHA-256. Edits to the source after the command returns
  cannot change what is sent. The receipt names the job id, size and hash.
- **Refused before queueing:** a missing, unreadable, empty or non-regular file;
  a file over Telegram's 50 MB bot limit (52,428,800 bytes); a caption that does
  not fit Telegram's 1024-character limit behind the `[sender-seat]` tag; a
  relative path with no caller cwd.
- **Upload:** `sendDocument`, `multipart/form-data` with `chat_id`, `caption`
  (sender/context prefix as for text, falling back to the bare tag) and
  `document`. The worker re-hashes the staged copy first; a missing or changed
  copy fails the job without uploading. The reply binding is written exactly as
  for text, so a Telegram reply routes back to the sending seat.
- **No silent drop.** The receipt says `queued`, never delivered. Every failed
  attempt appends `telegram.outbound-delivery-failed` with `document`
  (`file_name`, `size`, `sha256`), `http_status` and `terminal`. A 4xx other than
  429, a foreign chat, or a damaged staged copy fails the job (`state=failed`);
  transport errors, 429 and 5xx retry. A delivered job's staged copy is deleted;
  a failed job's copy is kept as evidence.
- **Version skew is loud.** Documents are their own sidecar tag and queue kind,
  so an older daemon refuses the request (unknown variant) and never claims the
  row, rather than sending the caption alone.
