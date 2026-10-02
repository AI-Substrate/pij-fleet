# agent — packaged agent jobs (rs CLI gap)

> Route module — sibling-blind. Conventions cited as § C*n* live in `00-routing.md` § Shared conventions (pull lazily).

**Job**: run a named agent pack. The `pij agent` CLI family (`list`, `show`, `run`, `spawn`, `new`, `check`, `eject`, `report`) is **unported** at the rs cutover and returns `E-RS-UNPORTED`. See [Unsupported status](../../../../docs/how/pij-rs-api.md#unsupported-status). Explicit legacy forcing is no escape; do not run a TS pack runner after this refusal.

No native `agent` wrapper, automatic `--once` dissolution, pack-schema validation, permission preset or agent-report transport is implied by native spawn. Do not represent a plain peer as an equivalent pack execution or fabricate pack/report receipts.

If the user instead authorizes an ordinary peer task, use the actually available host spawn capability or explicit `pij-rs spawn` grammar (§ C1), a persisted bounded packet, `pij dispatch` and supported `pij send` for its report. The packet must name the task, exact worktree, write fence, real validation obligations and report target. Canary the actual model/runtime (§ C2); observe pushed completion (§ C7), verify its claims and compact a reusable peer (§ C3). That is a distinct ordinary delegation, not a silent implementation of this missing pack verb.

Do independent authorized work while this pack-specific prerequisite is unresolved. Record the named refusal and tell the context owner; never create global links/settings or install a package to bypass it without authorization. Production/chainglass acceptance is prime-owned, and canonical fixture examples are not runtime proof.
