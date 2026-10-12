# pij-active-blackbird — plan 168 (a blank boot session must not fight the resumed Copilot session)

**magicWand:** a Copilot TUI driver in the harness that is versioned against the
installed runtime (composer box, modal footer, extension-permission dialog), so
`copilot-native-smoke` breaks loudly on a UI drift instead of timing out on a
parser that no longer matches. The native smoke cannot reach `/new` on 1.0.96-2.

**difficulties:**
- Relative-path `edit` resolved into the main checkout again (D-167-1, second
  strike). Recovered by moving the exact 66-line patch with `git apply`/`-R`;
  main was left with only its own pre-existing changes. This repeat is the
  argument for the magic wand recorded in plan 167.
- A canary dev daemon started without `TMUX=<isolated socket>` enumerated the
  live default tmux server's panes (read-only: "leaving foreign tap", no taps,
  no writes). Any isolated daemon must get the isolated socket in `TMUX`, as
  `copilot-native-smoke` does; a bare `--state-dir`/`--bind` is not isolation.
- Copilot 1.0.96 runs a custom status line only on its own events unless
  `statusLine.refreshInterval` is set, which looked like a pij identity bug
  (phantom name) while `pij whoami --pane` was already correct.
- `copilot-native-smoke` on 1.0.96-2: needs the extension hook permission
  pre-approved (now seeded), then fails at the pij_send approval modal because a
  status-line footer renders below it, and its composer parser expects the older
  box glyphs. Recorded, not ported.
