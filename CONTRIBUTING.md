# Contributing to pij

Thanks for your interest! Issues and pull requests are welcome.

## Before you start

- For anything larger than a small fix, open an issue first so we can agree on
  the approach.
- Security problems: **do not** open an issue — follow [SECURITY.md](./SECURITY.md).

## Getting set up

```bash
git clone https://github.com/AI-Substrate/pij-fleet.git
cd pij-fleet
just install     # deps, official pi, synced config, doctor
```

You need Node 24+, npm 11.10+, Rust (pinned in `rust-toolchain.toml`), and
[`just`](https://github.com/casey/just). If `registry.npmjs.org` is unreachable
from your network, export `PIJ_NPM_REGISTRY=<mirror url>`.

Read [`AGENTS.md`](./AGENTS.md) (rules for humans and agents alike) and
[`AGENTS_README.md`](./AGENTS_README.md) (cold-start guide) before changing code.

## Making a change

1. Branch from `main`.
2. Follow the extension patterns P1–P10 in `AGENTS.md`; scaffold new extensions
   with `just new <name>`.
3. Add or update tests next to the code you change.
4. Run the full gate before opening a PR:

   ```bash
   just lint
   just typecheck
   just test
   cargo test --workspace
   ```

   (`harness checks` runs the full signal inventory if you have the harness CLI.)
5. Use [Conventional Commits](https://www.conventionalcommits.org/) for commit
   messages (`feat:`, `fix:`, `docs:`, `chore:` …).
6. Open a PR describing what changed and how you verified it. CI must be green.

## Licensing

By contributing you agree that your contributions are licensed under the
[MIT License](./LICENSE).
