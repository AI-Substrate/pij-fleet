# pij-rival-salmon: F169 and F170 (2026-11-10)

**magicWand:** a Queue-port read of pending FYIs. The stall net and the PA
line read the `fyis` table through `SqliteOrchestration`, because the
`Queue` port can only count one seat's pile or claim it. A daemon on the fake
queue (tests only) sees no backlog at all, and that split is easy to miss.

**What happened:** the text taught the misuse. `SKILL.md` and both `pij_send`
descriptions said "choose `--fyi` unless their current work depends on it",
and the cold-wake refusal offered `--fyi` first at "$0 now". The fix is in
four layers: the wording leads with "does the recipient have to do
anything?", the receipt says the seat was NOT woken, a held FYI that reads
as actionable is warned about, and a stall net (one notice per backlog to
the PA, in the presence round) plus a waiting-FYIs line on every PA turn
catch what still slips through. F170 re-checks held state notices as they
flush.

**Found on the way:**
- The PA watchdog runs on simulated time in tests, but an FYI's `held_at`
  comes from the delivery clock. A re-arm that compared the newest FYI with
  the claim time mixed the two; the claim now records the newest `held_at`
  it covered, so the comparison stays on one clock.
- `SeatDescriptor.role` is empty on a bare registry read: the watchdog fills
  it with `join_roles`. Anything that asks "is this a PA?" at delivery has to
  read `seat_roles` itself.
- Delivery attaches FYIs inside a synchronous closure in the store's
  transaction, so an async re-check has to be captured as a snapshot before
  the closure is built.
- A changed golden fixture also needs its sha256, size and `answer` in
  `crates/testkit/fixtures/MANIFEST.toml`; the corpus test names the file.
