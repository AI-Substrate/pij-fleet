# pa memory

- [2026-11-04-pa-brief-is-the-procedure.md](2026-11-04-pa-brief-is-the-procedure.md): read the pij skill's prime/pa.md for how to run a round; PRD and roadmap live in ~/pi-hacking/pij on main

- [2026-11-04-copilot-ctx-from-footer.md](2026-11-04-copilot-ctx-from-footer.md): nudge shows ctx ? for Copilot seats; read size from the ⛓ footer line
- [2026-11-04-no-unscoped-anomalies.md](2026-11-04-no-unscoped-anomalies.md): LIFTED 16:28 by #120; run unscoped pij anomalies --json each round again

<!-- pij:generated -->
- [2026-11-06 Compaction line is 400k in this community](2026-11-06-compaction-line-is-400k-in-this-community.dd.md): read before deciding whether a seat needs compacting (pa, compact, threshold)
- [2026-11-06 Holders come from pij directory, not HOLDERS.md](2026-11-06-holders-come-from-pij-directory-not-holders-md.dd.md): read when mapping seats to roles in a round; s03 retired HOLDERS.md (pa, directory, holders, s03)
- [2026-11-06 Role-queue wakes come to the PA](2026-11-06-role-queue-wakes-come-to-the-pa.dd.md): read when woken because a role:\<id\> send queued with no live holder (pa, s05, role-queue, routing)
- [2026-11-06 Time-limit escalations: look, then tell the requester](2026-11-06-time-limit-escalations-look-then-tell-the-requester.dd.md): read when a \[pij time limit\] notice names a holder that hasn't answered (pa, s12, time-limit)
- [2026-11-07 After a reboot: map holdings to live seats](2026-11-07-after-a-reboot-map-holdings-to-live-seats.dd.md): read when the fleet comes back from a reboot or many seats die (pa, reboot, holdings, directory)
- [2026-11-07 Compaction line is 600k again \(supersedes the 400k note\)](2026-11-07-compaction-line-600k.dd.md): read before deciding whether a seat needs compacting (pa, compact, threshold)
- [2026-11-08 C31 steering is on: normal sends reach busy Copilot seats mid-turn](2026-11-08-c31-steering-on.dd.md): read when timing a handshake or judging whether a send is held (pa, c31, delivery, steering)
- [2026-11-08 ctrl+d dialog exception DROPPED for good \(supersedes the F109 note\)](2026-11-08-ctrl-d-exception-dropped-for-good.dd.md): read when a modal-stall tell or a stuck Copilot dialog appears (pa, f47, f109, f110, dialog, exception)
- [2026-11-08 ctrl+d follow-up-dialog exception is WITHDRAWN \(supersedes the F108 note\)](2026-11-08-ctrl-d-exception-withdrawn.dd.md): read when a modal-stall tell or a stuck Copilot dialog appears (pa, f47, f108, dialog, exception)
- [2026-11-08 Time-boxed: PA may ctrl+d Copilot's follow-up dialog \(until F108\)](2026-11-08-ctrl-d-follow-up-dialog-f108.dd.md): read when a seat is stuck on Copilot's 'Follow-up action:' dialog (pa, f47, f108, dialog, exception)
- [2026-11-08 Re-granted: PA may ctrl+d Copilot's follow-up dialog \(until F109\)](2026-11-08-ctrl-d-follow-up-dialog-f109.dd.md): read when pij-bg sends a modal-stall tell or a seat is stuck on Copilot's 'Follow-up action:' dialog (pa, f47, f108, f109, dialog, exception)
- [2026-11-08 Post-compaction wake is automatic now \(plan 222\); drop the manual one](2026-11-08-f111-wake-automatic.dd.md): read right after any compaction completes (pa, compact, f111, plan-222, wake)
- [2026-11-08 Flag normal sends held over 15 min \(C31, F113\)](2026-11-08-flag-held-sends-over-15m.dd.md): read when the digest's 'Messages held behind a busy turn' section has entries (pa, c31, f113, delivery, steering)
- [2026-11-08 Seats stuck on a modal dialog look working \(F47\)](2026-11-08-modal-dialog-stall-f47.dd.md): read when a seat shows working but has had no model call for 30+ min (pa, watchdog, f47, stall)
- [2026-11-08 Check native\_receiver\_reason, not the native-delivery flag](2026-11-08-receiver-lease-not-delivery-flag.dd.md): read when a send is refused or parked as native-extension-unavailable or native-receiver-unavailable (pa, delivery, receiver, lease)
- [2026-11-08 Recheck size before sending compact: Copilot may have auto-compacted](2026-11-08-recheck-size-before-compact.dd.md): read after 'handover ready' arrives, before pij send --command compact (pa, compact, copilot)
- [2026-11-08 Stray text on a Copilot input line may be screen-only](2026-11-08-stray-pane-text-may-be-screen-only.dd.md): read when a pane's input line shows text nobody typed (pa, pane, copilot, input)
- [2026-11-08 After compacting a seat with a live holding, wake it \(F111\)](2026-11-08-wake-after-compaction-f111.dd.md): read right after any compaction completes, and in each round's holdings check (pa, compact, f111, holdings, wake)
- [2026-11-09 Reviewers that queue work for a 'next turn' stall](2026-11-09-reviewer-queued-next-turn-stall.dd.md): read when a reviewer holds reviewer/\<pr\> but sits idle with no verdict (pa, stall, reviewer, gpt)
- [2026-11-10 After a compaction, the digest's context figure is stale; read the footer](2026-11-10-digest-ctx-stale-after-compaction.dd.md): read when someone reports a just-compacted seat as over the limit (pa, compact, context, digest)
- [2026-11-10 A held compact can be silently dropped \(F180\); verify, and escalate after one drop](2026-11-10-held-compact-can-drop-f180.dd.md): read after any --command compact returns 'held — memory-turn' (pa, compact, f180, copilot)
- [2026-11-10 The pij-fleet prime is now pij-outstanding-silkworm \(from 2026-11-10 16:15\)](2026-11-10-prime-is-silkworm.dd.md): read before escalating anything to the prime (pa, prime, escalation)
<!-- /pij:generated -->
