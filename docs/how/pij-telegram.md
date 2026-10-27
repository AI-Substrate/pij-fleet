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
  `method` actually used, `document`).
- **No silent drop.** The receipt says `queued`, never delivered. Every failed
  attempt appends `telegram.outbound-delivery-failed` with `method`, `document`
  (`file_name`, `size`, `sha256`), `http_status` and `terminal`. A 4xx other than
  429 (after any fallback), a foreign chat, or a damaged staged copy fails the job
  (`state=failed`); transport errors, 429 and 5xx retry without a fallback. A
  delivered job's staged copy is deleted; a failed job's copy is kept as evidence.
- **The token stays in the daemon.** It is read only from the credential file;
  it is never on the CLI's argv, and it never appears in a job payload, a job
  outcome, a spine event or a logged error (transport errors drop the URL, which
  carries the token).
- **Version skew is loud.** Files are their own sidecar tag and queue kind, so
  an older daemon refuses the request (unknown variant) and never claims the
  row, rather than sending the caption alone.

### Inbound media

pij-rs does not save media that Vaughan sends yet (follow-up issue). It no
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
