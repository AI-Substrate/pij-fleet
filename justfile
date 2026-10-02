# pij — agent-facing harness recipes.
#
# `just` is the canonical interface for agents. `npm run …` exists for
# direct script invocation and IDE integration, but the composite gates
# (self-check, vet, snapshots) live here so there is exactly one place
# to encode them. If you find yourself running multi-step npm chains by
# hand, the chain belongs in this file.
#
# See AGENTS.md § Self-improvement loop — encode, don't document.

# Forward variadic recipe args verbatim via "$@" (preserves quoting,
# spaces, and shell metacharacters like () so `just pij send <id> "a (b)"`
# works). Without this, {{ARGS}} re-splits and the shell chokes on ().
set positional-arguments

# Default recipe: list available recipes.
default:
    @just --list

# --- bootstrap ---
#
# `just install` is THE single command to set up pij on a fresh macOS/Linux
# machine after `git clone`. Idempotent — safe to re-run any time.
#
# What it does:
#   1. Install repo dependencies (`npm ci`, locked)
#   2. Build pij-rs (the daemon + CLI) and link it onto PATH next to npm's bin
#   3. Install OMP if absent, link the pij extension + curated MCP/models into
#      ~/.omp/agent, and link the native Copilot extension when Copilot is present
#   4. Link the pij skill machine-wide and the `pij` CLI shim (`npm link`)
#   5. Run `just doctor`
#
# Then start the daemon: `just bounce-rs` (or `pij-rs daemon`); on macOS,
# `just rs-autostart` starts it at login.

install:
    @echo "=== 1/5 npm dependencies ==="
    just _root-lock-npm-ci
    @echo
    @echo "=== 2/5 build + link pij-rs ==="
    just rs-install
    @echo
    @echo "=== 3/5 install omp + link managed surfaces ==="
    just omp-install
    @echo
    @echo "=== 4/5 link the pij skill and CLI ==="
    just pij-skill-link-global
    npm link
    @echo
    @echo "=== 5/5 doctor ==="
    just doctor
    just _copilot-native-install-check
    @echo
    @echo "✓ install complete. Start the daemon with: just bounce-rs"

# Build pij-rs in release mode from the canonical checkout and link it as
# `pij-rs` in npm's global bin (the same directory as the `pij` shim).
rs-install:
    #!/bin/sh
    set -eu
    npm run link -- --check-only
    cargo build --release --locked -p pij-cli
    bin="$(npm prefix -g)/bin"
    mkdir -p "$bin"
    ln -sfn "$(pwd)/target/release/pij-rs" "$bin/pij-rs"
    echo "✓ $bin/pij-rs → $(pwd)/target/release/pij-rs"

# Read-only audit of the installed surfaces: CLI shim, pij-rs, OMP policy,
# skill link and daemon health.
doctor:
    @global_npm_root="$(npm root -g)"; \
      pij_bin="$(command -v pij)"; \
      just _pij-bin-shape-check "$global_npm_root" "$pij_bin"
    @echo "pij-rs bin: $(realpath "$(command -v pij-rs)" 2>/dev/null || echo '(not on PATH — run: just rs-install)')"
    @echo "skill:      $(readlink ~/.agents/skills/pij 2>/dev/null || echo '(unlinked — run: just pij-skill-link-global)')"
    just omp-doctor
    @printf 'daemon:     '; pij-rs ping 2>/dev/null || echo '(not running — run: just bounce-rs)'

# Link this worktree's packages from the canonical checkout without npm resolution.
# --check is read-only; missing JS tooling never prevents Rust-only work.
worktree-deps *ARGS:
    @node harness/scripts/worktree-deps.mjs "$@"

# --- atomic checks (thin wrappers over npm scripts) ---

typecheck:
    npm run typecheck

lint:
    npm run lint

# Auto-fix formatting + import order via Biome.
format:
    npm run format

# Run vitest. Optionally scope to file(s)/pattern: `just test path/to/x.test.ts`.
test *ARGS:
    npm run test -- "$@"

# tmux-driven end-to-end smoke (Driver SDK).
smoke:
    npm run smoke


# Native Copilot extension contracts are node:test, outside Vitest's TS include.
copilot-native-test:
    node --test .copilot/extensions/pij/*.test.mjs

# Keep Cargo artifacts local to this checkout during isolated parallel work.
copilot-native-rust-test:
    CARGO_TARGET_DIR="{{justfile_directory()}}/target" cargo test -p pij-core -p pij-store -p pij-harnesses -p pij-daemon -p pij-cli

# Opt-in actual JS + daemon HTTP + SQLite contract, outside Rust-only/default gates.
copilot-native-contract-test:
    node --check "{{justfile_directory()}}/.copilot/extensions/pij/store.mjs"
    VITEST_MAX_WORKERS=4 VITEST_MAX_THREADS=4 CARGO_BUILD_JOBS=4 PIJ_NATIVE_RUNTIME_MODULE="{{justfile_directory()}}/.copilot/extensions/pij/store.mjs" CARGO_TARGET_DIR="{{justfile_directory()}}/target" cargo test --locked -p pij-daemon --test native_runtime_cold_resume -- --ignored --nocapture --test-threads=1

# Actual CLI smoke; local fixture and real-provider evidence remain distinct.
copilot-native-smoke *ARGS:
    node_modules/.bin/tsx harness/scripts/copilot-native-smoke.ts "$@"



# Large native history, receiver death and manual inbox recovery on isolated services.
copilot-native-memory-smoke *ARGS:
    @just _copilot-native-pinned-smoke harness/scripts/copilot-native-memory-smoke.ts "$@"

# Receiver RPC deadlines, model-picker recovery and SDK disconnect on isolated services.
copilot-native-progress-smoke *ARGS:
    @just _copilot-native-pinned-smoke harness/scripts/copilot-native-progress-smoke.ts "$@"


# Recipe-only runtime prerequisite; no installation, topology workaround or updater.
# COPILOT_BIN selects a launcher; COPILOT_VERSION defaults to the proven runtime.
_copilot-native-pinned-smoke SCRIPT *ARGS:
    #!/usr/bin/env node
    const { execFileSync, spawnSync } = require("node:child_process");
    const { readFileSync } = require("node:fs");
    const { join } = require("node:path");
    const pin = process.env.COPILOT_VERSION || "1.0.84-9";
    const reject = (reason) => {
        console.error(`PIJ_NATIVE_PREREQUISITE_VERSION: ${reason}`);
        process.exit(2);
    };
    let binary = process.env.COPILOT_BIN || "copilot";
    let runtime = join(process.env.HOME, ".copilot/pkg", `${process.platform}-${process.arch}`, pin);
    const args = process.argv.slice(3);
    const forwarded = [];
    for (let i = 0; i < args.length; i++) {
        if (args[i] === "--copilot-bin" || args[i] === "--copilot-runtime-dir") {
            const flag = args[i];
            const value = args[++i];
            if (!value || value.startsWith("--")) reject(`${flag} needs a value`);
            if (flag === "--copilot-bin") binary = value;
            else runtime = value;
        } else forwarded.push(args[i]);
    }
    let actual;
    try {
        const output = execFileSync(binary, ["--prefer-version", pin, "--version"], {
            env: { ...process.env, COPILOT_OFFLINE: "true" },
            encoding: "utf8", timeout: 10_000, stdio: ["ignore", "pipe", "pipe"],
        });
        actual = output.match(/^GitHub Copilot CLI (\S+)\.$/m)?.[1];
    } catch {
        reject(`${binary} could not report version ${pin}; select an installed native launcher with COPILOT_BIN`);
    }
    if (actual !== pin) reject(`${binary} resolved ${actual || "unknown"}, expected COPILOT_VERSION=${pin}; runtime drift is not a product failure`);
    try {
        if (JSON.parse(readFileSync(join(runtime, "package.json"), "utf8")).version !== pin)
            reject(`${runtime} does not contain COPILOT_VERSION=${pin}`);
    } catch {
        reject(`missing installed runtime ${runtime}; no automatic install`);
    }
    console.log(`copilot-native-smoke: ${binary} resolved ${pin}`);
    const result = spawnSync("node_modules/.bin/tsx", [
        process.argv[2], "--copilot-bin", binary, "--copilot-runtime-dir", runtime, ...forwarded,
    ], { stdio: "inherit" });
    if (result.error) reject(`could not launch smoke: ${result.error.message}`);
    process.exit(result.status ?? 1);
# Real-client draft-safety classifier; requires PIJ_STEP_ON_REAL=1 explicitly.
step-on-probe *ARGS:
    node_modules/.bin/tsx harness/scripts/step-on-probe.ts "$@"
# Read-only installation diagnosis. Does not enable extensions or change settings.
copilot-native-doctor:
    copilot --version
    npm run link -- --doctor-copilot

# Bootstrap reports an absent optional CLI; an installed but broken CLI still fails.
_copilot-native-install-check:
    #!/bin/sh
    set -eu
    if command -v copilot >/dev/null 2>&1; then
        just copilot-native-doctor
    else
        echo "Copilot CLI not installed; skipping optional native doctor."
    fi

# Reject user-specific absolute home paths in executable/configuration surfaces.
local-path-check:
    node_modules/.bin/tsx harness/scripts/local-path-check.ts

# Report-only attribution scan, scoped after merge-base with main.
pij-commit-trailers *ARGS:
    node_modules/.bin/tsx harness/scripts/pij-commit-trailers.ts "$@"

# Assert every `resolved` source in package-lock.json is allowlisted (npmjs
# registry or the sanctioned minih git source). The compensating control for
# npmjs-scoped host replacement — tamper-DETECTION at CI/review time. Any other
# host hard-fails.
lockfile-allowlist:
    node_modules/.bin/tsx harness/scripts/lockfile-allowlist.ts

# Run the pij CLI in-repo (no global link needed): `just pij list --here`.
# Quote message bodies normally: `just pij send pij-X "hello (world)"`.
pij *ARGS:
    node harness/scripts/pij-cli.cjs "$@"

# Print the production npm release-age value from the typed policy module.
_release-age-days:
    @node --input-type=module -e 'import { MIN_RELEASE_AGE_DAYS } from "./harness/scripts/release-age-policy.ts"; process.stdout.write(String(MIN_RELEASE_AGE_DAYS))'

# Run a fresh npm/Pi resolver with the governed registry (PIJ_NPM_REGISTRY or npmjs), online revalidation,
# and seven-day release-age policy.
_npm-resolution *ARGS:
    @node_modules/.bin/tsx harness/scripts/npm-resolution-run.ts "$@"

# Root lock replay must work before node_modules exists. Strip inherited npm
# policy keys case-insensitively, then retain only the governed authority,
# lock-host replacement, and
# online settings while the CLI argument clears age for this frozen operation.
_root-lock-npm-ci:
    @set -eu; \
      eval "$(env | sed -n 's/=.*//p' | awk '{ lower=tolower($0); if (lower=="npm_config_registry" || lower=="npm_config_replace_registry_host" || lower=="npm_config_prefer_online" || lower=="npm_config_min_release_age" || lower=="npm_config_before") print "unset " $0 }')"; \
      npm_config_registry="${PIJ_NPM_REGISTRY:-https://registry.npmjs.org/}" \
      npm_config_replace_registry_host="npmjs" \
      npm_config_prefer_online="true" \
      npm ci --min-release-age=null

# Prove locked install, fresh-resolution refusal, and audit visibility separately.
release-age-probe:
    @node_modules/.bin/tsx harness/scripts/release-age-probe.ts

# List the GitHub Copilot models your account is actually entitled to.
# Auto-selects the correct API host from the token's proxy-ep claim
# (enterprise vs individual), so it works where pi's models.json host 421s.
#   just copilot-models            # all entitled model ids
#   just copilot-models mai        # filter ids containing "mai"
#   just copilot-models --json     # raw JSON
copilot-models *ARGS:
    @python3 harness/scripts/copilot-models.py "$@"

# --- composite gates ---

# Pre-merge / pre-release gate. Agents MUST run this before reporting a
# task complete — never run npm directly to compose these steps.
self-check:
    just local-path-check
    just lockfile-allowlist
    just typecheck
    just lint
    just test
    just copilot-native-test
    just rust-check
    just smoke
    just pij-commit-trailers

# The Rust gate CI runs: lockfile, fmt, clippy (warnings are errors), tests.
rust-check:
    cargo metadata --locked --format-version 1 >/dev/null
    cargo fmt --all --check
    cargo clippy --locked --workspace --all-targets -- -D warnings
    cargo test --locked --workspace

# --- ergonomics ---

# Link the pij extension + curated MCP/models into ~/.omp/agent and the native
# Copilot extension into ~/.copilot (canonical checkout only).
link:
    npm run link

unlink:
    npm run link -- --remove

# Report where the machine's live pij currently resolves — CLI bin, extension,
# skill store, and daemon. The quick answer to "am I on main or the worktree?".
where:
    @echo "pij CLI bin → $(realpath "$(command -v pij)" 2>/dev/null || echo '(not on PATH)')"
    @echo "extension   → $(readlink ~/.omp/agent/extensions/pij 2>/dev/null || echo '(unlinked)')"
    @echo "skill store → $(readlink ~/.agents/skills/pij 2>/dev/null || echo '(unlinked)')"
    @echo "pij-rs bin  → $(realpath "$(command -v pij-rs)" 2>/dev/null || echo '(not on PATH)')"
    @printf 'daemon      → '; pij-rs ping || true

# Structural gates for the /pij router skill (plan 030): registry↔module parity,
# sibling-blindness, line budgets, CLI-verb coverage, duplicated-prose scope.
pij-skill-check:
    bash harness/scripts/pij-skill-check.sh

# Link skills/pij MACHINE-WIDE from the canonical checkout. The shared
# link-global guard refuses linked worktrees before this recipe can remove the
# existing target, preventing a development checkout from hijacking every seat.
pij-skill-link-global:
    #!/bin/sh
    set -eu
    npm run link -- --check-only
    source="$(realpath skills/pij)"
    target="$HOME/.agents/skills/pij"
    mkdir -p "$(dirname "$target")"
    rm -rf "$target"
    ln -sfn "$source" "$target"
    echo "✓ $target → $source (symlink, drift-proof)"

# Backwards-compatible alias; global skill installation is link-only.
pij-skill-install:
    just pij-skill-link-global

# Install the official OMP binary (GitHub releases, via https://omp.sh/install).
# Downloads the installer to a file and runs it, rather than `curl … | sh`: in a
# pipe the shell's exit status wins, so a failed download silently feeds an empty
# script to a shell that exits 0 and the caller believes it installed something.
_omp-binary-install *REF:
    #!/bin/sh
    set -eu
    tmp=$(mktemp)
    backup=""
    prev=$(command -v omp 2>/dev/null || true)
    if [ -n "$prev" ]; then
        backup=$(mktemp)
        cp "$prev" "$backup"
    fi
    trap 'rm -f "$tmp" "$backup"' EXIT
    if ! curl -fsSL --connect-timeout 10 --max-time 300 https://omp.sh/install -o "$tmp"; then
        echo "!! could not download the omp installer from https://omp.sh/install" >&2
        exit 1
    fi
    # Do NOT let `set -e` abort here. The upstream installer runs its own start
    # check and exits non-zero on the macOS stale-vnode SIGKILL below — the exact
    # failure the repair path that follows exists to fix. Aborting on its exit
    # code leaves the machine with a downloaded, unrunnable omp and no rollback.
    if [ -n "${1:-}" ]; then
        sh "$tmp" --binary --ref "$1" || true
    else
        sh "$tmp" --binary || true
    fi
    # A failed smoke check means we just replaced a working omp with one that does
    # not run. Put the old one back rather than leaving the machine without omp.
    if just _omp-smoke-check; then
        exit 0
    fi
    # Observed on macOS 25.x: the upstream installer rewrites the binary in place
    # (`curl -o "$INSTALL_DIR/omp"`), and a signed Mach-O rewritten over a vnode the
    # kernel has already validated gets SIGKILLed on launch — rc 137, no output, even
    # though the file is byte-complete, correctly signed and notarized. Re-materialising
    # it at a fresh inode (copy + atomic rename) clears the stale validation.
    now=$(command -v omp 2>/dev/null || true)
    if [ -n "$now" ]; then
        echo "= omp did not launch; re-materialising it at a fresh inode" >&2
        cp "$now" "$now.reinstall"
        chmod +x "$now.reinstall"
        mv "$now.reinstall" "$now"
        if just _omp-smoke-check; then
            exit 0
        fi
    fi
    if [ -n "$prev" ] && [ -n "$backup" ]; then
        echo "= rolling back to the previous omp binary at $prev" >&2
        cp "$backup" "$prev.rollback"
        chmod +x "$prev.rollback"
        mv "$prev.rollback" "$prev"
        echo "= rolled back: $(omp --version 2>/dev/null | head -1 || echo '(still not running)')" >&2
    fi
    exit 1

# A downloaded omp is not an installed omp. A byte-complete, correctly signed and
# notarized binary can still be SIGKILLed on launch (see _omp-binary-install), and a
# version delta alone would report that as a successful update. `--version` is the
# cheapest proof it actually runs.
_omp-smoke-check:
    #!/bin/sh
    set -eu
    if ! v=$(omp --version 2>/dev/null | head -1) || [ -z "$v" ]; then
        echo "!! the installed omp does not run — 'omp --version' produced no version." >&2
        echo "   The download itself may be fine; check 'codesign -v' and the file size" >&2
        echo "   against the release asset before assuming a network or registry fault." >&2
        echo "   Recover by pinning a release known to run here, e.g.:" >&2
        echo "     just _omp-binary-install v17.1.2" >&2
        exit 1
    fi
    echo "= omp runs: $v"

# Explain why omp's own updater could not reach its update source. omp's updater
# fetches https://registry.npmjs.org/@oh-my-pi/pi-coding-agent/latest directly: the
# host is compiled into the binary, so it honours neither `.npmrc` nor
# NPM_CONFIG_REGISTRY. On a machine where npmjs is blocked or proxied, that fetch
# always fails — and omp still exits 0.
_omp-update-diagnose:
    #!/bin/sh
    set -eu
    if curl -fsS --connect-timeout 10 --max-time 20 -o /dev/null https://registry.npmjs.org/ 2>/dev/null; then
        echo "  · registry.npmjs.org is reachable — the cause is not a blocked registry;"
        echo "    read omp's own message above (rate limit, TLS, or a transient network fault)."
    else
        echo "  · registry.npmjs.org is NOT reachable from this machine."
        echo "    omp's updater hardcodes that host, so it ignores your configured registry"
        echo "    (npm config get registry = $(npm config get registry 2>/dev/null || echo unknown))."
        echo "    The GitHub-releases installer used below is the supported path here."
    fi

# Latest omp release tag (bare version, no leading "v") from the same GitHub
# releases feed the installer uses. Prints nothing and fails if unreachable, so
# callers can tell "already latest" apart from "cannot reach any update source".
_omp-latest-release:
    #!/bin/sh
    set -eu
    json=$(curl -fsSL --connect-timeout 10 --max-time 30 \
        https://api.github.com/repos/can1357/oh-my-pi/releases/latest 2>/dev/null) || exit 1
    printf '%s' "$json" \
        | grep -o '"tag_name"[[:space:]]*:[[:space:]]*"[^"]*"' \
        | head -1 \
        | sed -e 's/.*"\(.*\)"/\1/' -e 's/^v//'

# Install OMP from the SAME governed npm proxy as everything else, so the
# supply-chain policy in .npmrc actually applies to it: the configured registry,
# `replace-registry-host`, the `before` cutoff and the 7-day `min-release-age`
# quarantine. `_npm-resolution` supplies that environment; nothing here bakes in
# a registry URL or relaxes policy.
#
# `@latest` here means "newest the PROXY offers that also satisfies the age
# quarantine" — deliberately BEHIND upstream. That lag is the control working,
# not a failure to update.
#
# The npm package is bun-shebanged (`#!/usr/bin/env bun`), so bun is a hard
# runtime dependency. It is installed from the proxy too: the platform binary
# ships as the `@oven/bun-*` optional dependency, i.e. registry payload rather
# than a postinstall download, so the chain stays governed end to end. bun is
# the one install here that must run scripts — its postinstall materialises the
# bin from that optional dependency.
_omp-npm-install:
    #!/bin/sh
    set -eu
    if ! command -v bun >/dev/null 2>&1; then
        echo "= installing bun (omp's runtime) from the governed proxy"
        just _npm-resolution npm install -g bun
    fi
    just _npm-resolution npm install -g --ignore-scripts @oh-my-pi/pi-coding-agent@latest
    just _omp-smoke-check

# Warn when a second omp exists outside the governed install. The GitHub-releases
# installer writes to ~/.local/bin; npm writes to `npm prefix -g`/bin. Whichever
# PATH reaches first wins, so an un-governed copy can silently shadow the vetted
# one — and it reports a HIGHER version, which reads as more up to date rather
# than as policy-evading.
_omp-shadow-check:
    #!/bin/sh
    set -eu
    governed="$(npm prefix -g)/bin/omp"
    active="$(command -v omp 2>/dev/null || true)"
    [ -n "$active" ] || exit 0
    if [ "$active" != "$governed" ] && [ -x "$governed" ]; then
        echo "!! the active omp is NOT the governed install:" >&2
        echo "   active:   $active ($("$active" --version 2>/dev/null | head -1))" >&2
        echo "   governed: $governed ($("$governed" --version 2>/dev/null | head -1))" >&2
        echo "   The active copy bypassed the npm proxy and its quarantine." >&2
        echo "   Remove it, or put $(dirname "$governed") earlier on PATH." >&2
        exit 1
    fi

# Install OMP from the governed registry when absent, then restore pij's managed
# OMP policy: only the pij extension plus the curated MCP and model config.
omp-install:
    #!/bin/sh
    set -eu
    if command -v omp >/dev/null 2>&1; then
        echo "= omp already installed: $(omp --version | head -1)"
    else
        just _omp-npm-install
    fi
    just link
    just omp-doctor

# Update OMP from the governed proxy, then re-apply managed links because
# updates may replace home state.
#
# `omp update` is NOT used, deliberately. Its update source
# (registry.npmjs.org/@oh-my-pi/pi-coding-agent) is compiled into the binary, so
# it honours neither .npmrc nor NPM_CONFIG_REGISTRY: on this machine it cannot
# reach that host at all, and where it could it would fetch a version the
# quarantine has not cleared. Same reason the GitHub-releases installer is no
# longer a fallback here — it is a different host, so reaching it proves the
# network works while proving nothing about the supply chain. That path survives
# as `just _omp-binary-install [REF]` for deliberate, explicitly-chosen recovery
# only; it is never reached automatically.
update-omp:
    #!/bin/sh
    set -eu
    before=$(omp --version 2>/dev/null | head -1 || true)
    if [ -z "$before" ]; then
        echo "= omp not installed — installing from the governed proxy"
    else
        echo "= current: $before"
    fi
    just _omp-npm-install
    after=$(omp --version 2>/dev/null | head -1)
    if [ -z "$before" ]; then
        echo "✓ omp installed: $after"
    elif [ "$before" != "$after" ]; then
        echo "✓ omp updated from the proxy: $before → $after"
    else
        echo "= already on the newest proxy release the quarantine allows: $after"
    fi
    just _omp-shadow-check
    just link
    just omp-doctor

omp-doctor:
    #!/bin/sh
    set -eu
    npm run link -- --check-only
    omp --version
    npm run link -- --doctor-omp

# Run vitest scoped to the flow-pair lib tests (explicit path bypasses vitest
# config include filter; also works with: just test skills/flow-pair/test/).
flow-pair-test *ARGS:
    node_modules/.bin/vitest run skills/flow-pair/test/ "$@"

# Mutation smoke: PROVE the flow-pair suite actually guards a behaviour. The worker
# writes its own tests, so green != good — this deliberately breaks <file> with a
# sed ERE expr, asserts tests go RED, restores byte-identical, asserts GREEN again.
# A suite that stays green under mutation is decoration. See references/review-rubrics.md
# Dimension 0. Usage:
#   just flow-pair-mutate skills/flow-pair/lib/ledger.ts 's/if \(!ev[A-Za-z]+\.ok\)/if (false)/g'
#   just flow-pair-mutate <file> '<expr>' 'npx vitest run <suite>'   # target the suite that guards <file>
flow-pair-mutate file expr *test_cmd:
    bash harness/scripts/flow-pair-mutate.sh "{{file}}" '{{expr}}' {{test_cmd}}

# --- pij-rs daemon + CLI ---

_pij-bin-shape-check global_npm_root pij_bin:
    @set -eu; \
      expected="{{global_npm_root}}/pij/harness/scripts/pij-cli.cjs"; \
      test -e "{{pij_bin}}" || { \
        echo "❌ global pij bin is missing: {{pij_bin}}"; \
        echo "   run npm link from the local main checkout, or run: just install"; \
        exit 1; \
      }; \
      test -f "$expected" || { \
        echo "❌ linked pij package has no wrapper at: $expected"; \
        echo "   run npm link from the local main checkout, or run: just install"; \
        exit 1; \
      }; \
      actual="$(realpath "{{pij_bin}}")"; \
      expected="$(realpath "$expected")"; \
      if [ "$actual" != "$expected" ]; then \
        echo "❌ stale global pij bin: {{pij_bin}} -> $actual"; \
        echo "   expected: $expected"; \
        echo "   run npm link from the local main checkout, or run: just install"; \
        exit 1; \
      fi; \
      echo "pij bin: $actual"

# Rebuild pij-rs from THIS checkout and restart the production rs daemon in one act
# (sub-second gap; clients reconnect). Standing order after any rs merge. Verifies pid change + ping.
bounce-rs:
    #!/usr/bin/env sh
    set -eu
    cargo build --release --locked -p pij-cli 2>&1 | grep -E '^error|Finished' | tail -1
    bin="$(npm prefix -g)/bin/pij-rs"
    old=$(lsof -nP -iTCP:7461 -sTCP:LISTEN -t || true)
    [ -n "$old" ] && kill "$old" && sleep 0.5
    # Telegram worker is OPT-IN by env (crates/daemon/src/lib.rs); a bare restart silently turns it off.
    mkdir -p "$HOME/.pij-rs"
    (PIJ_RETIRED_HARNESSES="${PIJ_RETIRED_HARNESSES-pi}" PIJ_TELEGRAM_ENV="${PIJ_TELEGRAM_ENV:-$HOME/.pij/telegram.env}" nohup "$bin" daemon >> "$HOME/.pij-rs/daemon.log" 2>&1 &)
    sleep 1.5
    new=$(lsof -nP -iTCP:7461 -sTCP:LISTEN -t || true)
    [ -n "$new" ] && [ "$new" != "$old" ] || { echo "bounce-rs: daemon did not come back (old=$old new=$new)" >&2; exit 1; }
    "$bin" ping
    echo "bounce-rs: pid $old -> $new"

# Start the production rs daemon at login (launchd LaunchAgent, RunAtLoad only).
# launchd execs Apple-signed /bin/sh, which execs pij-rs: after the 2026-09-27 macOS
# upgrade launchd killed the ad-hoc (cargo linker-signed) binary at login with
# OS_REASON_CODESIGNING "Launch Constraint Violation" while a shell ran it fine.
# No KeepAlive on purpose: bounce-rs kills and relaunches by port, and a KeepAlive
# respawn would race it for 7461. Env mirrors bounce-rs; PATH is captured from this shell.
rs-autostart:
    #!/usr/bin/env sh
    set -eu
    label=dev.pij.rs-daemon
    bin="$(npm prefix -g)/bin/pij-rs"
    plist="$HOME/Library/LaunchAgents/$label.plist"
    mkdir -p "$HOME/Library/LaunchAgents"
    cat > "$plist" <<PLIST
    <?xml version="1.0" encoding="UTF-8"?>
    <!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
    <plist version="1.0"><dict>
      <key>Label</key><string>$label</string>
      <key>ProgramArguments</key><array><string>/bin/sh</string><string>-c</string><string>exec "$bin" daemon</string></array>
      <key>EnvironmentVariables</key><dict>
        <key>PATH</key><string>$PATH</string>
        <key>PIJ_RETIRED_HARNESSES</key><string>${PIJ_RETIRED_HARNESSES-pi}</string>
        <key>PIJ_TELEGRAM_ENV</key><string>${PIJ_TELEGRAM_ENV:-$HOME/.pij/telegram.env}</string>
      </dict>
      <key>RunAtLoad</key><true/>
      <key>StandardOutPath</key><string>$HOME/.pij-rs/daemon.log</string>
      <key>StandardErrorPath</key><string>$HOME/.pij-rs/daemon.log</string>
    </dict></plist>
    PLIST
    plutil -lint "$plist"
    # Register without starting now if a daemon already holds 7461 (it would just fail to bind).
    launchctl bootout "gui/$(id -u)/$label" 2>/dev/null || true
    if lsof -nP -iTCP:7461 -sTCP:LISTEN -t >/dev/null; then
      echo "rs-autostart: installed $plist (daemon already running; takes effect at next login)"
    else
      launchctl bootstrap "gui/$(id -u)" "$plist" && echo "rs-autostart: installed and started $label"
    fi

# Remove the login LaunchAgent. Does not stop a running daemon.
rs-autostart-off:
    launchctl bootout "gui/$(id -u)/dev.pij.rs-daemon" 2>/dev/null || true
    rm -f "$HOME/Library/LaunchAgents/dev.pij.rs-daemon.plist"
    @echo "rs-autostart: removed"
