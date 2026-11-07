# Original ask

Jordan, 2026-11-07, to pij-far-jackal (prime), after the usage survey found that another project's PA made no calls from 14:00 to 21:00 AEST on 6 Nov (about 66% of that day's Claude usage) while its prime and 14 workers ran at 400k to 800k tokens of context, and nothing fired because the PA was idle, not dead:

- "lets talk about what we can do in dameon to nudge if prime is doing work but PA is missing, or dormant for more than x time"
- "new messages to prime when seats are over 600k in its fleet shodl say so. just like FYI they will be psted next call."
- "good get it done" (on the playback: missing 20 min, dormant 30 min, unresponsive 10 min more; Copilot seats report their context size).

The idle-PA alert was first raised by pij-bright-jay from the survey (`scratch/usage-survey-2026-11-07.md`, git-ignored).

Why the existing watchdog didn't catch it (`crates/daemon/src/pa_watchdog.rs`): it nudges a PA only at the fleet's idle edge and once when the prime resumes after one, so it sends nothing while workers are busy; and it is driven per PA, so a fleet with no live PA gets nothing at all.
