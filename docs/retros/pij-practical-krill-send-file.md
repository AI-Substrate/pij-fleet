# Retro: pij-practical-krill, pij send-file (files through the daemons)

**Role:** coder, native Copilot seat. **Prime:** pij-kind-parrotfish. **Ask:** Jordan, 2026-11-28: files up to 50 MiB through the daemon connection, cross-machine first, deleted once accepted.

**Delivered:** `pij send-file` and `pij file list|accept|decline`. One local route (`POST /v1/files`, argv plus caller, like `pij requests`) and one new peer route (`POST /v1/files/inbound`, the fourth on the federation allowlist). The sender's daemon snapshots the file and streams it to the paired daemon under the pairing key; the bytes live only on the recipient's machine, as `<state>/transfers/<code>` plus `<code>.json`, no migration. Accept writes an `O_EXCL|O_NOFOLLOW` temp file, checks sha256, then hard-links (no clobber) or renames (`--force`) into place. Expiry runs at boot and on each send; no timer.

**magicWand:** a `pij-rs` test fixture that boots two *paired* routers in one call (`paired_daemons()`), with each one's federation pointing at the other. Today a test must spawn B first to learn its address before building A's federation, so the round-trip test (B telling A's sender the outcome) hand-binds both listeners first; `files::tests::serve` is the seed of that fixture.

**difficulties:**
- D-SF-1: the shared build volume was at 19 GiB free, under shared-build's 25 GiB floor, at the start of the session (16 GiB by the end). I ran my checks with `PIJ_DISK_FLOOR_GB=12` on the existing shared target and told the prime.
- `cargo check` would have told me nothing about the route inventory, the clap verb roster, the shim's native-verb list or the skill table: four pinned lists outside the code that each fail a different test. All four are good sensors; a one-line "adding a verb touches these" in AGENTS.md would have saved three test rounds.
