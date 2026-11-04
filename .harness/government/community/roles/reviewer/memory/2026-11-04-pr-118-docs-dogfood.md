# PR 118 dogfood documentation review

Tags: reviewer, docs, community, role, ddocs, exact-head

Hook: role-input validation depends on both stamping state and schema-discovery cwd.

Pass-with-notes at `868e830398b3bcba4f1c6361a9920745723266b4`. Verdict: `../outputs/2026-11/pr-118-verdict.md`. All nine new difficulty rows match the dogfood report, source and PM routing; the D-190-5 CI failure and passing rerun are confirmed directly. All four exact-head CI jobs pass.

Two P3 wording notes: only an unstamped role lacking a changelog necessarily fails that schema check; a complete scratch role outside a checkout validates when ddocs runs from the current checkout root. The same scratch file produces E401 from scratch cwd, while removing only its changelog produces exactly one E402 from checkout cwd. Keep the writer-owned workflow and required changelog unchanged.

No product edits, builds, cargo, installs, daemon mutations or checkout movement. Quick local checks remain tooling-limited; Copilot-native passes. The loaded boot command rejects its advertised `--full` flag, but plain boot returns the missing-dependency advisory without downstream work.

magicWand: one read-only docs-review preflight for pinned head, privacy/whitespace, CI and honest dependency readiness. Suggested encoding is in the verdict; no unrelated harness code was changed.
