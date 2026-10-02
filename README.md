# pij

**A local control plane that lets coding agents work as a fleet.**

pij lets agent sessions running in different harnesses — [OMP](https://omp.sh)
(oh-my-pi), Claude Code, GitHub Copilot CLI and Codex — find each other, message
each other, spawn new peers in tmux, and coordinate long-running work under a
"prime" seat that governs streams of workers.

It is built for one developer running many agents on one machine. Everything
stays local: a Rust daemon on loopback, a per-boot bearer key, and a SQLite
store under `~/.pij-rs`.

> **Status:** experimental and moving fast. macOS and Linux only — pij drives
> tmux, so Windows is not supported yet.

## How it fits together

```
 OMP session ──┐  pij extension (.omp/extensions/pij)
 Claude Code ──┤  SessionStart / prompt / stop hooks + statusline
 Copilot CLI ──┤  native extension (.copilot/extensions/pij)      ┌───────────────────┐
 Codex       ──┤  tmux-bound seat                          ──────▶│ pij-rs daemon     │
               │                                                  │ 127.0.0.1:7461    │
 you / scripts ┴─ `pij` CLI (shim)  /  `pij-rs` CLI (native) ────▶│ store: ~/.pij-rs  │
                                                                  └───────────────────┘
```

- **`pij-rs`** (`crates/`) — the daemon and native CLI: seat registry and
  identity, durable message delivery with receipts, held FYIs, a cold-wake
  guard, tmux spawn/revive/reap, background jobs, and the governance records
  (projects, streams, dispatches, fences, decisions) that a prime uses.
- **`pij`** (`harness/scripts/pij-cli.cjs`) — a thin CLI shim that forwards to
  the daemon's API. It serves a supported operation or refuses with a named
  error; it never falls back to anything else.
- **Harness integrations** — the OMP extension and the Copilot native extension
  receive pushed messages in-process; Claude Code seats are adopted by a
  SessionStart hook that the daemon installs; Codex seats are bound to their
  tmux pane.
- **Skills** (`skills/`) — `pij` is a router skill that teaches an agent how to
  adopt a seat, spawn peers, delegate, and run a prime; `flow-pair` runs a
  coder + cross-model reviewer loop.

## Requirements

- macOS or Linux, with **tmux**
- **Node.js 24+** and **npm 11.10+**
- **Rust** (the toolchain pinned in `rust-toolchain.toml`)
- [**just**](https://github.com/casey/just)
- At least one agent harness. OMP is installed for you by `just install`.

## Install

```bash
git clone https://github.com/AI-Substrate/pij-fleet.git
cd pij-fleet
just install        # npm deps, build + link pij-rs, OMP + extension links, skill, CLI
just bounce-rs      # build and (re)start the daemon
just rs-autostart   # optional, macOS: start the daemon at login
just doctor         # read-only health check
```

`just install` is idempotent; re-run it whenever things drift. If
`registry.npmjs.org` is unreachable from your network, export
`PIJ_NPM_REGISTRY=<mirror url>` first.

The repo ships the maintainer's OMP defaults (`.omp/models.yml`,
`.omp/mcp.json`). `just install` links them into `~/.omp/agent` only where you
have no file of your own; a real file there is never replaced.

## Using it

Start a harness inside tmux. OMP and Copilot seats register themselves; a
Claude Code seat is adopted by its SessionStart hook. Then:

```bash
pij list                         # every registered seat
pij whoami                       # which seat am I?
pij send <seat-id> "hello"       # message a peer (it arrives as a user turn)
pij send --fyi <seat-id> "..."   # hold it until the peer's next turn instead
pij state <seat-id>              # one seat's state card
pij report now "<did>" "<next>"  # publish what you're doing
```

Inside an agent, prefer the skill: `/pij ready`, `/pij peer`, `/pij delegate`,
`/pij pair`, `/pij prime`. See [`skills/pij/SKILL.md`](skills/pij/SKILL.md).

## Documentation

| Topic | Where |
|---|---|
| API, CLI and refusal contracts | [`docs/how/pij-rs-api.md`](docs/how/pij-rs-api.md) |
| Platform model (seats, governance) | [`docs/how/pij-platform.md`](docs/how/pij-platform.md) |
| Prime governance and batons | [`docs/how/pij-prime.md`](docs/how/pij-prime.md), [`docs/how/pij-orchestration-baton.md`](docs/how/pij-orchestration-baton.md) |
| Claude Code seats and statusline | [`docs/how/claude-auto-seat.md`](docs/how/claude-auto-seat.md), [`docs/how/claude-statusline.md`](docs/how/claude-statusline.md) |
| Copilot CLI native extension | [`docs/how/copilot-native-extension.md`](docs/how/copilot-native-extension.md) |
| Coder + reviewer loops | [`docs/how/flow-pair.md`](docs/how/flow-pair.md) |
| Telegram bridge | [`docs/how/pij-telegram.md`](docs/how/pij-telegram.md) |
| Building and the npm supply-chain policy | [`docs/how/build.md`](docs/how/build.md) |
| Rules for humans and agents working on pij | [`AGENTS.md`](AGENTS.md) |

## Development

```bash
just self-check     # local paths, typecheck, lockfile, lint, tests, Copilot native, Rust, smoke
just rust-check     # Cargo.lock, fmt, clippy -D warnings, cargo test
just test           # vitest only
```

If you use the [engineering harness](https://github.com/AI-Substrate/harness-engineering)
CLI, `harness checks` runs the same gate stage by stage. See
[CONTRIBUTING.md](CONTRIBUTING.md) before opening a pull request, and
[SECURITY.md](SECURITY.md) to report a vulnerability privately.

## License

[MIT](LICENSE)
