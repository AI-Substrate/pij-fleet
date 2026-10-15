# Original ask (plan 174)

Jordan, 2026-10-15, relayed by pij-different-smelt after the Mac reboot: "pij should warn you about that when sending or receiving message, double seat issue".

pij-different-smelt's three asks:
1. When a seat sends or receives (and when adopt or whoami fails), detect another live claude process resuming the same session id, and warn both panes loudly, naming the other pane.
2. The 'not registered' refusal should say 'conversation X is live as seat Y in pane %N; close one copy, then pij adopt'.
3. On revive, check whether the session id is already open in another live process before launching a second copy.

Jordan's follow-ups:
- "yes" (to the prime writing the plan and handing it to a coder).
- "i want copilot cli harness, hydrafusion-max model please" (the coder dogfoods this seat setting).
- "this is a new model that we dont know how good it is so the work will need to be tested well".

Prime's findings before planning (2026-10-15):
- Ask 3 already exists: revive's E-RS-REVIVE-CONVERSATION-LIVE guard (crates/daemon/src/http/revive.rs). In the incident it ran correctly, 49 s BEFORE the hand resume.
- Asks 1 and 2 were already detected. ~/.pij-rs/hook.log shows %7's SessionStart adopt, its status-line heal and every prompt refused with "conversation cc2d2e8f… is live as seat pij-different-smelt in another process (pane %5)". But that went only to the log, with no code, and %5 was never told.
- Tracked as pij-fleet#55.
