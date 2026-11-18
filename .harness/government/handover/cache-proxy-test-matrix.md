# Copilot cache proxy: setups and tests (acceptance for workstream 004)

The rule under test is that a seat uses the proxy only if the proxy is proven working when the seat spawns.
Otherwise the seat runs plain Copilot, and its receipt says why. Tests run on real seats in the live fleet.
Seats whose model is not Claude still run plain.

| # | Setup | Expected | Test (live seats) |
|---|---|---|---|
| 1 | No `copilot.account` set (fresh machine) | Proxy off; seats run plain; doctor row "off: no account" plus the fix | Clean config, spawn a Copilot Claude seat: plain, receipt states why, a normal turn works |
| 2 | Account set, valid, Claude model | Seat proxied; 1h cache | Spawn, one turn, idle 6+ min, a second turn: `cache_read` ≈ context, write tiny, `ephemeral_1h` > 0 |
| 3 | Account set, GPT model | Plain (model gate) | Spawn gpt-6.1-sol: no COPILOT_PROVIDER_* in its env, turn works |
| 4 | Account set, but gh not logged in to it, or gh missing | Plain plus the reason | `copilot.account` set to an account gh lacks, then spawn: plain, receipt names it |
| 5 | Account valid, no Copilot entitlement or quota exhausted | The gate's last check, a 1-token `/v1/messages` probe (cached per account per token life), fails; the seat runs plain plus the reason. Mid-run: one state-change notice. (`/models` answers 200 on an exhausted quota, so it can't be the check.) | Use the out-of-quota personal account: spawn is plain with the reason "quota exhausted (402)"; at runtime, the 402 is surfaced once, not looped |
| 6 | Proxy not listening at spawn (off or crashed) | Plain plus the reason | Stop the proxy, spawn: plain |
| 7 | Proxy dies mid-session | Seat calls fail; ONE prime notice naming the seats and the fix; `/restart` comes up plain | Kill the proxy under 2 live seats: notice; `/restart` one: plain, works |
| 8 | Daemon bounce during an in-flight turn | Decides in-daemon vs sidecar | Bounce mid-turn on 2 seats: does Copilot retry and finish the turn? Record it both ways |
| 9 | Token lifetime (decided by step 1's measurement) | **If the `gho_` token has no expiry:** refresh only on a 401, single-flight, one retry. **If the exchanged ~30-min token is required:** proactive refresh at about 80%. Either way, no failed call across a refresh | No expiry: force a 401 (revoke or inject a stale token) on 3 active seats at once: 1 refresh, each call retried once, 0 failures. With expiry: a short test lifetime (env-only knob), 3 seats active across 3 refreshes: 0 failures, 1 exchange per refresh |
| 10 | Token revoked mid-run (401) | Refresh plus one retry; the call succeeds | Force a 401 (inject a stale token): retried once, the turn completes |
| 11 | Refresh fails (gh logged out mid-run) | Keep the token until real expiry, back off, one notice; at expiry, the affected-seats notice | `gh auth logout` during a run: one notice, seats work until expiry, then the expiry notice |
| 12 | Account switched (`pij config set copilot.account B`) | The next refresh and spawns use B; doctor shows B | Switch, spawn: the request goes to B's endpoint |
| 13 | Opt-out: `--no-cache-proxy`, and machine-wide off | Plain | Both forms: no provider env |
| 14 | Personal, business and managed enterprise accounts | The endpoint is discovered from the token (`endpoints.api`) | Each available account type: correct upstream host in the proxy log (host only) |
| 15 | Tool-use continuations | `X-Initiator: agent`; not billed as a user request | A turn with 5 tool calls: premium requests +1, not +6 |
| 16 | Streaming, tool use, 1M context, images | Byte-faithful pass-through | A long tool-heavy turn plus an image on a proxied seat matches a plain seat's behaviour |
| 17 | 10 seats share the proxy | Single-flight refresh; no launch per request | spawn counter: 0 launches per request; at most 1 `gh` per account per process life (+1 per failed refresh) |
| 18 | Seats launched before the proxy existed | Listed as "plain" in doctor and directory; `/restart` moves them | Doctor lists them; restart one: proxied |
| 19 | Second human account on the Mac | Doctor warns: the loopback port is reachable cross-account | Simulated via doctor's account check (unit test) |
| 20 | OMP seats | Unaffected; they keep their own 1h config | An OMP seat's turn is unchanged; it never sees the proxy |
| 21 | Proxy state changes (on to off to on) | One notice per change to each prime; no repeats (F192) | Toggle twice: exactly 2 notices per prime |

Receipts for each row go into the workstream's proof: the command, the observed value and the seat ids.
