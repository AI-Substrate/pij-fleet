# pij-legitimate-dragon — pij-fleet#36 (supervise the governance delivery observer)

**magicWand:** a fault-injection seam on the real store (e.g. a test-only
`PIJ_RS_FAULT=spine.tail:1`) so a dev daemon can reproduce a transient store
error on demand. Without it, the runtime proof of a "survives one transient
error" fix stops at a unit witness. A held SQLite lock cannot produce the error
in WAL mode while the daemon keeps its connections open: `BEGIN EXCLUSIVE`
under `locking_mode=EXCLUSIVE` was itself refused with `database is locked`.

**difficulties:**
- Relative-path edits went to the main checkout again (D-36-1, second strike of
  D-167-1).
- The packet named bg, park-notice, death-sweep and federation as having the
  same die-on-first-error shape. On main they already log the error and keep
  going, so no red test could exist for them. Pushed back with file:line
  evidence before writing code, and the coordinator agreed.
- `pij-rs daemon --bind 127.0.0.1:0` is refused (`port 1..65535`). An isolated
  smoke has to pick a free port itself.
