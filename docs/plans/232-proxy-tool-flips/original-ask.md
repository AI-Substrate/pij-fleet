# Original ask

From pm-004 (pij-outstanding-silkworm), 2026-11-19, packet `004-s07-coder-232-packet.md`, prime-approved scope, top priority. In brief: "Stop the tool-list flips on proxied Copilot seats, then weigh a per-seat TTL."

1. Canary `toolSearch=false` on one proxied seat against a control. Report the numbers, then stop. Any machine-wide change is Jordan's call.
2. Extend egret's `scratch/proxy-survey/sim.py` to re-price the same windows as if the flip re-writes were gone. Report the fleet both ways.
3. Weigh per-seat TTL policies against "1h for all" and "5m for all". Build one in the proxy only if the sim shows a clear win after the flip fix.

pm-004's rulings during the step:
- `toolSearch` cannot be set from a repo (`copilot config --local` refuses it). The prime applied it user-wide on Jordan's go at 2026-11-19T03:27:46Z. The canary is one fresh proxied seat, and egret's before-window is the control.
- Split the after-window by Copilot process start. Copilot reads settings only at start.
- Keep 1h for everyone and build nothing in the proxy (TTL verdict accepted).
- Record the `extensions_reload` token-only flip as a known residual. No fix is in scope.
- Open the PR now. The after-window goes in as a dated receipt at 03:40Z and is re-run once, with at least an hour of post-restart data, before the round-1 verdicts.
