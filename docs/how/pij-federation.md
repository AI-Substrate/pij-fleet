# Federation: pairing two machines

Each machine runs its own pij daemon and its own database. Two daemons that
are **paired** can address each other's seats (`pij send seat@laptop`),
`pij list` shows both rosters, and events flow between them. Nothing crosses
machines without a pairing. Plan 164 switched this on and secured it.

## What a pairing is

One pre-shared key per machine pair. Both ends hold the same key. Each daemon
reads `<state-dir>/peers.toml` (default `~/.pij-rs/peers.toml`) at boot:

```toml
machine = "mac-studio"            # this machine's own alias

[[peer]]
alias = "laptop"                  # what this machine calls the peer
url   = "http://100.101.102.103:7461"
key   = "<the mac-studio ↔ laptop key>"
```

The laptop's file mirrors it: `machine = "laptop"`, and a `[[peer]]` with
`alias = "mac-studio"`, the mac-studio's URL, and **the same key**.

The daemon refuses to start if the file:

- is not owned by the daemon's user, or has any group/other permission bit
  (`chmod 600`);
- has a malformed alias (letters, digits, `.`, `_`, `-` only), a peer named
  like this machine, a duplicate alias, or a URL that is not
  `http(s)://host[:port]`;
- has a key shorter than 32 characters, or two peers sharing one key.

No file, or a file with no `[[peer]]`, means **no machine is paired**: the
daemon accepts no remote calls and refuses any non-loopback bind.

## What a paired machine may do

A peer key authenticates **as that machine's alias**, so every inbound call is
attributed to a named machine. It reaches exactly three routes:

| Route | Why |
|---|---|
| `POST /v1/send` | deliver a message to a seat on this machine |
| `GET /v1/seats?scope=local` | read this machine's roster |
| `GET /v1/events?scope=local` | follow this machine's events |

Every other route (spawn, kill, `bg create`, governance writes, inbox, …) and
the two reads without `scope=local` answer HTTP 403 `E-RS-PEER-SCOPE`. The
allowlist is one function, `Endpoint::peer_access`; a route added later is
refused to peers by default. A peer may not relay onward (`to.machine` set),
may not send controls, and cannot claim to be another machine: `from_machine`
is stamped from the key.

A forwarded message keeps its origin all the way to the agent that reads it.
Every harness (OMP, Pi, Copilot, Claude's pane frame) and every FYI rendering
(the block, the digest, `fyi-read`) shows the sender as `seat@alias`, never a
bare name that could pass for a local seat. A message's identity is the pair
(origin machine, msg_id), each in its own field: a peer's `X` and a local `X`
are two messages, and an exact retry from the same machine is still one. No
msg_id may contain `@` (it separates a seat from its machine), local or
forwarded. A reply to `seat@alias` (the `pij_send` tool, `pij send`, or
`pij-rs send --to`) is routed to that machine.

A forwarded message obeys the **receiver's** rules. The receiving daemon runs
its cold-wake guard and holds `--fyi` messages exactly as for a local sender.
The sender's `pij send` waits up to 10 s for the first forwarding attempt
(`federation_first_attempt_wait_secs`), so a cold recipient's refusal comes
back inline as `E-RS-COLD-WAKE`, with the cold facts in `details`. A
`--force --reason` resend is audited on the receiver
(`send.cold-wake-forced`, `from_machine` naming the sender's machine). If the
peer does not answer in time, the message stays queued and is retried with
backoff.

What the sender's receipt says (F284):

| Receipt | Means |
|---|---|
| `delivered` | the peer delivered it to the seat |
| `queued — accepted by laptop; queued there for <seat> (<its reason>)` | the peer has it, durably, and will deliver it as it would a local message; the reason in brackets is the PEER's local one (`unreachable` there means the recipient's live channel was not ready, not the machine), shown without control characters and cut at 200 characters |
| `queued — peer laptop unreachable; queued here, retrying` | the connection to the peer failed, so nothing was sent; this machine retries with backoff |
| `queued — sent to laptop; reply lost (it may have been delivered); retrying` | the request went out but no complete answer came back (a timeout, a reset, a cut body); the peer may have it already, and the retry is one message by the peer's dedupe |
| `queued — peer laptop answered an error; queued here, retrying` | the peer is up but answered an error or something this build cannot read |
| `queued — forwarding to laptop (pending)` | no attempt finished within the wait; nothing is known about the peer yet |

The spine's `delivery.outcome` event keeps the peer's receipt exactly as the
peer sent it; only the sender's inline answer is worded for the reader.

## Setup on two machines

On either machine, mint the pair's key:

```bash
pij-rs peers new-key          # 64 hex characters, 256 bits from OS entropy
```

Write `~/.pij-rs/peers.toml` on **both** machines (mirrored aliases, same key),
then `chmod 600 ~/.pij-rs/peers.toml`. Move the key between machines over a
channel you trust (an SSH session, a password manager); never paste it into a
chat or an issue.

Bind each daemon to its Tailscale address and restart it:

```bash
PIJ_RS_BIND=100.101.102.103:7461 pij-rs daemon     # or: pij-rs daemon --bind …
```

Or write it into `peers.toml`, above the first `[[peer]]`, so every restart
keeps it, a flagless one included (`just bounce-rs`, `pij-rs daemon bounce`,
the `rs-autostart` LaunchAgent):

```toml
machine = "mac-studio"
listen  = "100.101.102.103:7461"   # IP:port; --bind and PIJ_RS_BIND still win
```

The address is resolved as `--bind`, then `PIJ_RS_BIND`, then `listen`, then
`127.0.0.1:7461`. A `listen` that is not `IP:port` refuses the boot.

Its port is the daemon's one port, loopback included, exactly as with
`--bind`: `listen = "100.101.102.103:7500"` moves `127.0.0.1` to `:7500` too,
and a local client still on `7461` no longer finds the daemon. Keep `7461`
unless every local client is pointed at the new port.

The daemon **always** listens on `127.0.0.1:<port>` too, so local clients,
hooks and extensions keep their default address. The Tailscale address is a
second listener on the same port, serving the same routes under the same
auth and peer scope. If that second listener is refused (no pairing, or not a
Tailscale address without `--insecure-bind`) or cannot bind (the address is
not on this host yet, say Tailscale is down), the daemon prints
`WARNING: remote listener NOT started: …` and keeps serving loopback. It never
exits over the second listener; restart it once the address is available.

Check the pairing from each end:

```bash
pij-rs peers check
# ~/.pij-rs/peers.toml · this machine: mac-studio
#   laptop           http://100.101.102.103:7461   key 3f9a0c1e  ok
```

`peers check` applies the boot validation, then calls each peer's local roster
with that peer's key and accepts only a real, authenticated `pij seats`
envelope. Keys print only as fingerprints (the first 8 hex
characters of their SHA-256): compare fingerprints across machines, never keys.
`key refused` means the other machine's `peers.toml` does not hold this key for
this machine; `alias mismatch` means the two files disagree on a name.

Then:

```bash
pij send pij-quiet-heron@laptop "hello from the studio"
pij list            # remote rows read pij-quiet-heron@laptop
```

An unreachable peer never blocks `pij list`: its last-known rows stay, and the
table ends with `unavailable: laptop (<reason>)`.

## Tailscale and the bind rule

The daemon speaks plain HTTP, so a key crosses the wire in clear unless the
network encrypts it. Outbound, the federation clients and `peers check` never
use `HTTP_PROXY`/`HTTPS_PROXY`/`ALL_PROXY`: a proxy would see every key. They
connect directly or not at all. Inbound, the bind rule for the second
listener (`check_bind`, one function; loopback is always bound):

| Second listener | Unpaired | Paired |
|---|---|---|
| Tailscale (`100.64.0.0/10`, `fd7a:115c:a1e0::/48`) | refused | allowed (WireGuard encrypts) |
| anything else, including `0.0.0.0` / `::` | refused | refused unless `--insecure-bind` (or, for peers.toml's `listen`, `insecure_listen = true`) |

A refused second listener costs only itself; loopback keeps serving.

`--insecure-bind` prints a loud warning on stderr at boot. It is never a
default. It is a brake on where the daemon listens, not a policy on what
callers may do.

On a plain LAN with no Tailscale, put the opt-in beside the address in
`peers.toml`, so a restart keeps both (F284):

```toml
machine = "studio"
listen  = "0.0.0.0:7461"
insecure_listen = true    # this file's `listen` only; never a --bind address
```

`insecure_listen` opts in the file's own `listen` and nothing else: an address
given with `--bind` or `PIJ_RS_BIND` still needs `--insecure-bind`, and
`insecure_listen` without `listen` refuses the boot. The same loud warning
prints at every boot, naming the switch that opened it:
`WARNING: insecure_listen in peers.toml: …` or `WARNING: --insecure-bind: …`.
Both keys only feed `check_bind`: they can add a listener the rule allows,
never bypass it. They are read once, at boot.

## Key rotation and revocation

- **Revoke a machine:** delete its `[[peer]]` from `peers.toml` and restart
  the daemon. Exactly that machine loses access; other pairings are untouched.
- **Rotate a pair's key:** `pij-rs peers new-key`, write the new key into the
  pair's `[[peer]]` on **both** machines, restart both daemons, and run
  `pij-rs peers check` on each. A send made while the two ends disagree is
  refused by the peer (`peer \`<alias>\` refused this machine's pairing key`).
  That refusal is **terminal**: the message is not queued or retried. Resend it
  once both ends hold the new key.
- **Suspect a leak:** rotate that pair's key. Every pair has its own key, so a
  leaked key exposes one pair, and only the three federation routes.

Sources: [`pairing.rs`](../../crates/daemon/src/pairing.rs) (file),
[`http/auth.rs`](../../crates/daemon/src/http/auth.rs) (key → alias),
[`http/mod.rs`](../../crates/daemon/src/http/mod.rs) (`Endpoint::peer_access`, send),
[`http/exposure.rs`](../../crates/daemon/src/http/exposure.rs) (bind rule),
[`federation/`](../../crates/daemon/src/federation/) (forwarding, fan-in),
[`peers.rs`](../../crates/cli/src/peers.rs) (`peers check`).
