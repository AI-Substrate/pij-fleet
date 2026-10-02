# pij-rs — quick start

The Rust port of pij (plan 108). The TypeScript pij continues to live in this
same repo at its own paths; the two coexist and nothing here touches it.

## Prerequisites

**Use rustup's cargo, not another rust on your PATH.** This is not style advice:
`rust-toolchain.toml` is a rustup mechanism, and a cargo that is not a rustup shim
ignores it *silently*. On the machine this port was built on, Homebrew's rust
1.95.0 shadowed rustup's 1.98.0, so the compiler and the linter were already
different versions before any code existed.

```bash
export PATH="$HOME/.cargo/bin:$PATH"     # or invoke ~/.cargo/bin/cargo directly
rustc --version                          # must print 1.98.0
```

The gate's first stage checks this for you, and fails loudly if it is wrong.

## Build, test, gate

```bash
cd <repo root>
CARGO_INCREMENTAL=0 cargo build --workspace
CARGO_INCREMENTAL=0 cargo test  --workspace

# The one command. Runs every stage even after one fails, then prints a
# per-stage verdict — one invocation surfaces every problem.
#
# `--locked` is part of the command, not a flourish: without it the `cargo run`
# that builds the gate can quietly repair a stale Cargo.lock before the gate
# starts, and the lock stage then inspects a tree the gate itself just fixed.
CARGO_INCREMENTAL=0 cargo run --locked -p pij-testkit --bin pij-gate
```

`pij-gate` is: **toolchain** (running rustc == the pin) → **lock** (`Cargo.lock`
already matches the manifests) → **fmt** (`--check`) →
**clippy** (`-D warnings`) → **test** (whole workspace) → **arch** (crate graph vs
the committed allow-list). Non-zero exit means the task is not done.

## Run it

```bash
# Terminal 1 — a daemon on fakes: no database, no tmux, no network.
cargo run -p pij-cli --bin pij-rs -- daemon --bind 127.0.0.1:7461 \
    --state-dir /tmp/pij-rs

# Terminal 2
cargo run -p pij-cli --bin pij-rs -- ping --addr 127.0.0.1:7461 \
    --state-dir /tmp/pij-rs
# pij ping: ok — healthy
cargo run -p pij-cli --bin pij-rs -- ping --addr 127.0.0.1:7461 \
    --state-dir /tmp/pij-rs --json
```

The daemon writes a per-boot bearer key to `<state-dir>/daemon.key` (0600, before
the socket binds) and `ping` reads it. A restart invalidates the old key. A
transcript of all of this is in
`docs/plans/108-rust-port/assets/reports/first-light.md`.

## The crates

| crate | what it owns |
|---|---|
| `core` | domain types, config, errors, the **seven frozen ports**, pure logic. No IO, no tokio, no SQL, no HTTP. |
| `store` | SQLite via sqlx: registry, spine, queue, migrations. The only crate that speaks SQL. |
| `tmux` | every tmux syscall in the workspace |
| `harnesses` | per-harness detect/bind/readiness/model-catalog quirks |
| `transport` | message transports behind the `Transport` port |
| `daemon` | composition root #1 — HTTP, tick loop, workers |
| `cli` | composition root #2 and the only binary, `pij-rs` |
| `testkit` | fakes, contract suites, the fixture corpus, `FreshStore`, the gate binaries |

`docs/plans/108-rust-port/assets/architecture.md` explains why, and what a later
unit is expected to copy.

## Adding a unit (the shapes to copy)

1. **Put it in the crate that owns the concern.** If the code needs IO, it does
   not belong in `core`; give it a port there and an adapter beside it.
2. **Every new dependency costs one reviewed line** in
   `crates/testkit/arch-allowlist.toml`, with a rationale. That is the point, not
   friction to route around.
3. **Write the contract once** in `pij_testkit::contract`, and run it against both
   the fake and the real adapter — this caught two real defects in wave 0.
4. **Cite a fixture rather than inventing data**: `crates/testkit/fixtures/`, with
   `MANIFEST.toml` carrying each fixture's sha256 and the answer a correct
   implementation must produce.
5. **Fakes come from `pij-testkit`.** Mocking frameworks are refused
   workspace-wide by the arch gate, by name.
6. **Spikes go in `pocs/`**, outside the workspace and outside the gate.
