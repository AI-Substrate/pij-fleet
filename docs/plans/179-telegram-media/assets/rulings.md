# Plan 179 rulings (prime pij-broad-rykard, 2026-10-27)

| id | topic | ruling |
|---|---|---|
| Q1 | #49 | Supersede: a new PR from pij-prime/telegram-media that references #49. Ask in #49 to close it on merge; do not touch its branch. |
| Q2 | gif | sendAnimation for gif only; fall back to sendDocument on a 400, the same as photos. |
| Q3 | inbound | (b) In this PR: no silent drop, a notice to the bound seat plus a telegram.inbound-media-unsupported event. (a) A follow-up issue for full inbound save-to-path, citing the old TS design (20 MB getFile, sanitised names, retention). |
| — | deploy | Out of scope. A live canary only with Vaughan's go. |
| — | PR body | The full test list, the mutants and the CI link. |
