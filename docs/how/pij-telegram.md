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
A failure retries the whole job: a late-part failure can resend earlier bubbles.
Numbering makes repeated parts recognizable; it is not deduplication. A 429's
`parameters.retry_after` is recorded as `retry_after_s`.
Inbound still uses the persisted outbound binding, not name or swipe-reply parsing.

### Retries, timeouts and inbound (plan 183)

- **Backoff.** A retrying job (transport error, 429, 5xx, and for text any
  non-success) waits 2 s, then doubles per attempt up to 300 s: 2, 4, 8, … 256,
  300. A 429's `retry_after` wins when it is longer, so a retry is never sooner
  than Telegram asked. The queue rounds each deadline UP to its whole-second
  clock, so even a 100 ms wait is never released early.
- **Attempt cap.** After 10 attempts the job fails
  terminally (`state=failed`, outcome `gave up after 10 attempts; last failure:
  …`) and appends `telegram.outbound-gave-up` on the conversation seat
  (`conversation`, `from`, `msg_id`, `kind`, `attempts`, `retry_after_s`,
  token-free `reason`). A `retry_after` over 3600 s fails the job at once with
  that reason instead of waiting. pij's own waits add up to about 13.5 minutes;
  with a `retry_after` of up to an hour per wait and up to 600 s per upload
  attempt, one job can stay live for about 10.7 hours at worst.
- **Order on a chat.** Text and file jobs on one chat's serial key are claimed
  strictly in queue order, even while the earliest is waiting out a backoff: a
  retrying body holds its files behind it. The cap bounds the hold; once the
  head fails, the rest go, in order. Other chats, and every other queue kind,
  are not held.
- **Timeouts.** Connect 10 s. `sendMessage` and `getUpdates` 30 s. A file upload
  600 s: 50 MB at 1 Mbit/s takes 400 s, so this covers an uplink down to about
  0.7 Mbit/s, and a stalled upload ends as a retry instead of hanging the worker.
- **Inbound never waits on outbound.** Outbound sends and inbound polling are two
  separate loops. A retrying, failing or stalled send never delays `getUpdates`:
  a message from the phone reaches the bound seat while an upload is in flight.
  While `getUpdates` itself fails, inbound waits 1 s, doubling to 60 s, between
  calls (reset by a success) rather than failing on every tick.
- **A crashed pass is not a dead loop.** Each pass of either loop runs on its
  own; if one panics, that loop waits 1 s, doubling per consecutive panic to
  60 s, appends `telegram.loop-restarted` (`loop`, `reason`, `restarts`,
  `backoff_ms`) on seat `pij-telegram-loops`, logs it, and carries on.
- **Shutdown.** Both loops are told to stop before either is joined, so inbound
  stops at once even while an upload (up to 600 s) finishes.
- **Cost.** Each loop ticks every 250 ms and launches no process. Outbound does
  one in-process SQLite claim per tick and an HTTP call only when a job is due:
  a job that never succeeds makes at most 10 calls in all (at most 5 in its first
  minute, then about one per backoff), not one per tick. Inbound is one
  `getUpdates` long poll (1 s) per pass, about one call a second, as before,
  and at most one a second (then one a minute) while it fails. Each pass is a
  task in the daemon, not a process.

The API root is injectable in Rust tests, not via environment/credential files.
Tests use only local fake HTTP endpoints; an isolated daemon cannot currently
use a stub API root. Never use production credentials to fill that proof gap.

### Files: photos, videos, GIFs and documents

Send files with
`pij-rs sidecar telegram --file ./chart.png [--file ./demo.mp4 …] [--caption 'text']`,
optionally with `--body 'text'` or `--body-file <path|->`. The shim also serves
one file: `pij send pij-telegram --file ./report.html [--caption 'text']` (a
relative path resolves against the caller's cwd; the shim takes no body with
`--file`). `--caption` requires `--file`. Only `pij-telegram` accepts `--file`;
any other recipient still refuses it by name.

- **Method by type.** Chosen at enqueue from the lower-cased extension and size,
  and recorded on the job as `method`:

  | file | method | field |
  |---|---|---|
  | `jpg` `jpeg` `png` `webp` up to 10 MB | `sendPhoto` | `photo` |
  | `mp4` | `sendVideo` | `video` |
  | `gif` | `sendAnimation` | `animation` |
  | anything else, or an image over 10 MB | `sendDocument` | `document` |

  When `sendPhoto`, `sendVideo` or `sendAnimation` answers HTTP 400 (for example
  a photo whose dimensions Telegram refuses), the same attempt re-sends the same
  bytes with `sendDocument`. The 400 is still recorded (see below), with
  `"fallback": "sendDocument"`.
- **Several files.** Up to 10 `--file` flags. Each file is its own job, so each
  has its own outcome and a retry never resends a file that was delivered. Jobs
  are queued in argument order on the chat's serial key, so they arrive in order.
  With one file and no body the job keeps the request's `msg_id`; otherwise file
  *n* is `<msg_id>#<n>`. `--caption` goes on the first file; every file's caption
  carries the `[sender-seat]` tag.
- **Body with files.** The body is queued first, as an ordinary text send (same
  prefix, chunking and failure events as above), then the files.
- **Staged by content at enqueue.** The daemon copies each file into
  `<state>/sidecars/telegram-attachments/<sha256>-<msg-key>` (dir `0700`, file
  `0600`) while hashing it. The job (`sidecar:telegram:document`) records the
  source and staged paths, file name, size, SHA-256 and method, never the bytes.
  Edits to the source after the command returns cannot change what is sent. The
  receipt names each job id, size and hash.
- **Refused before queueing:** no files; more than 10 files; a missing,
  unreadable, empty or non-regular file; a file over Telegram's 50 MB bot limit
  (52,428,800 bytes); a caption that does not fit Telegram's 1024-character limit
  behind the `[sender-seat]` tag; a relative path (the CLI resolves paths to
  absolute ones). Every file is staged before any job is queued: if one is
  refused, nothing is queued, the body is not sent, and copies already staged
  are removed.
- **Upload:** `multipart/form-data` with `chat_id`, `caption` (sender/context
  prefix as for text, falling back to the bare tag) and the method's file field.
  The worker re-hashes the staged copy first; a missing or changed copy fails the
  job without uploading. The reply binding is written exactly as for text, so a
  Telegram reply routes back to the sending seat. A delivered file appends
  `telegram.outbound-media-delivered` (`conversation`, `from`, `msg_id`,
  `method` actually used, `telegram_message_id` (Telegram's own id for the
  message), `document`).
- **No silent drop.** The receipt says `queued`, never delivered. Every failed
  attempt appends `telegram.outbound-delivery-failed` with `method`, `document`
  (`file_name`, `size`, `sha256`), `http_status` and `terminal`. A 4xx other than
  429 (after any fallback), a foreign chat, or a damaged staged copy fails the job
  (`state=failed`); transport errors, 429 and 5xx retry without a fallback. A
  delivered job's staged copy is deleted; a failed job's copy is kept as evidence
  for 7 days (by its mtime, which is its enqueue time), then removed by a sweep
  the outbound loop runs on start and every 6 hours: one directory read and one
  queue read in process, nothing launched. Abandoned `.partial-*` writes go the
  same way. Age is the policy; the brake is that a copy a pending or running
  file job sends is NEVER removed, however old (for example after the daemon
  was down for longer than a week with files queued).
- **No copy a live job sends is ever released.** A refused or failed send gives
  back the copies it staged, except any a pending or running job sends: an
  identical file under the same id and position stages to the same path as the
  live job. File sends stage and queue one at a time in a daemon, so that check
  stays true until the release.
- **One send, one batch.** The body and every file are queued in one store
  transaction: an internal error leaves nothing from the send queued, and the
  copies it staged are released (as above).
- **`msg_id` reuse.** Resending the identical request while it is still queued
  answers with the live jobs. Reusing a `--msg-id` that is still queued with
  different content (other bytes, caption, body or source) is refused by name,
  nothing is queued, and the new copies are removed; once the earlier job has
  finished, the id is free again.
- **Symlinks.** A symlinked `--file` is read through the link and sent under the
  link's own name, from the CLI and the shim alike.
- **The token stays in the daemon.** It is read only from the credential file;
  it is never on the CLI's argv, and it never appears in a job payload, a job
  outcome, a spine event or a logged error (transport errors drop the URL, which
  carries the token).
- **Version skew is loud.** Files are their own sidecar tag and queue kind, so
  an older daemon refuses the request (unknown variant) and never claims the
  row, rather than sending the caption alone.

### Inbound media

pij-rs does not save media that Vaughan sends yet
([pij-fleet#70](https://github.com/AI-Substrate/pij-fleet/issues/70)). It no
longer drops it silently either. A photo, document, video, animation, audio,
voice note, video note or sticker from an allowed user is delivered to the bound
seat as a text turn:

```
[telegram media not saved by pij-rs: photo, 1.2 MB] caption: look at this
```

(kind, then the size and file name when Telegram reports them, then any caption),
and appends `telegram.inbound-media-unsupported` (`update_id`, `conversation`,
`msg_id`, `kind`, `size`, `file_name`, `mime_type`) beside the usual
`telegram.inbound-enqueued`. With no outbound binding it is refused exactly as
text is.
