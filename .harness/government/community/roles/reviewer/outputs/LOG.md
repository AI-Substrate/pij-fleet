# reviewer outputs

2026-11-04 verdict pr-110 4c5a792beab929a94a399a2a3cef57aad5696862 pass-with-notes — no blocker; P3 schema-show wording; scratch replay and required-section mutation passed; pinned CI green; local quick gate dependency-limited.

2026-11-04 verdict pr-111 b13f5b3a433e3b410a8e376ca0252cb67358bbab block — P2 stale typing-gate gap and P2 already-decided Claude inbound default; five mutation controls passed, docs validate/render clean, exact-head CI green; local quick gate dependency-limited.

2026-11-04 verdict pr-113 75cf1a150d32e2f1b2ecf3ef3f1aab4f979001eb block — P2 directory-read errors silently skip project docs; 7 unit tests pass, 14 immutable-SHA CLI probes reproduced blocker; normal invalid/unavailable guards work; local quick gate tooling-limited, pinned CI cancelled.

2026-11-04 verdict pr-113 2b328dc6b71788fee466a0a8c1e0b7fc9720df1d block - supersedes 75cf1a1; six-file C18/req-27 documentation delta passes validation/render/lint, exact-head CI four jobs passed; unchanged sensor retains F001 silent traversal pass.

2026-11-04 verdict pr-111 75102c5a16060632d4fe325dd519ab54f5e06cfb block - original two P2s closed; F003 retains missing live pane/large-body proof from old req-0005/0022 as one narrow row; six mutation controls, revised docs and exact-head CI pass; no product edits.

2026-11-04 verdict pr-113 224512db39caf3d549b50b25190e13aa6884e97a pass-with-notes - F001 closed by named traversal failures; 9 unit tests and 17 exact-head CLI controls pass, including chmod/missing/nested roots and warn mode; all four CI jobs pass; local composite gate dependency-limited, no correctness changes owed.

2026-11-04 verdict pr-111 a83c73b9fcaf4a1688e8139f60e420f403ca55cd pass-with-notes - F003 closed by req-14; final counts 16 kept as 14 rows, 19 met, 5 obsolete; checker/docs/render/lint and four exact-head CI jobs pass; local composite gate dependency-limited, no correctness changes owed.

2026-11-04 verdict pr-114 d5fd8cf48b4ffbe636818e996798f4bac3a4d9ed block - P2 missing-brief packet commits entire batch; P2 relative-PATH ddocs fails after chdir; 33 existing targeted tests pass, two guard mutations detected, real-ddocs/runtime controls reproduce both blockers; frozen-head CI green, local quick gate tooling-limited.

2026-11-04 verdict pr-114 0c6ea8886e6f4dec9cc09d61cdcd654157be28ae block - supersedes d5fd8cf after complete two-file docs-only delta review; F001/F002 unchanged, production code/schemas/tests identical; exact 0c6ea88 CI four jobs green, no rebuild or checkout movement; GitHub already reports later e735b84.

2026-11-04 verdict pr-114 b28d815be55ee4698d308eea1b67d91c3e4d0fd2 block - supersedes 0c6ea88 after complete d5fd8cf..b28d815 docs-only review; cost note matches receipts, backpressure validates and renders without drift, exact-head CI four jobs green; unchanged F001/F002, no rebuild or checkout movement.

2026-11-04 verdict pr-116 99d8bf42102a8d642a520fbc27008369de63f8e5 block - F001: findings.md:78 retains the exact worktree path scrubbed elsewhere (four occurrences to one); meaning preserved, private owner/name hits zero, eight positive controls pass, exact-head CI four jobs green; local quick gate tooling-limited, no Cargo or product edits.

2026-11-04 verdict pr-116 279b0ec430edcb0b64ed39f76afbbb6ac111be20 pass - corrects the provisional same-head block after PM clarification: generic documented install roots are allowed; F001 closed, no outstanding findings; tracked-tree identity/path scrub zero, 20 controls pass, plan validates 0/0 and render has no drift, four CI jobs green; local quick gate tooling-limited, original worktree unchanged.

2026-11-04 verdict pr-114 a4bb4fa1e9786667051897156e78c3143a75303e pass-with-notes - F001/F002 closed; static S1 controls and six expected mutation failures verified, restored 32 community + 3 HTTP + 4 spawn-site tests pass; scoped fmt/clippy and merged DD documents pass, main merge clean, manifests identical; security owns lstat/write residual, local composite gate tooling-limited.

2026-11-04 verdict pr-114 4ec59a851d224b0a7736c6366ee9f05684f8b447 pass-with-notes - final one-line D-190-4 delta reviewed, no new finding; F001/F002 closure, static S1 checks and a4bb4fa evidence carried forward; exact final-head CI four jobs green, no rebuild or checkout movement; security residual note unchanged.

2026-11-04 verdict pr-118 868e830398b3bcba4f1c6361a9920745723266b4 pass-with-notes - nine difficulty rows and writer procedure corroborated; P3 unstamped-input and discovery-cwd qualifications; schema positive/negative controls and seven privacy controls pass, all four exact-head CI jobs green; local quick gate tooling-limited, no builds or cargo.

2026-11-04 review-paused pr-120 43821d5ba66ad7e6f46a976785d393f9f4af0c15 incomplete - six focused tests and exact-head CI pass; no correctness finding; required scratch mutation stopped before cargo because disk fell to 24 GiB while waiting for the shared lock. Not a pass or merge approval.

2026-11-04 verdict pr-120 43821d5ba66ad7e6f46a976785d393f9f4af0c15 pass-with-notes - no correctness finding; overlapped initial baseline withdrawn, indexed-kind mutation detected, independent paging/SQLite controls pass, uncontended cold restored daemon oracle and no-tail guard pass; exact-head CI green; local composite gate tooling-limited, live AC5 remains performance-owned.

2026-11-04 verdict pr-122 94592ad6a0ba14a97c8ae1ad2d930c4ba7185ff3 block - F001 P1 same-role writes clear pending close-ends without committing them; F002 P2 same-second release/reassign collides with holding PK and rebuild silently drops the new holding; 51 baseline tests pass, three regressions fail, close guard mutation detected and restored control passes; exact-head CI green, local quick gate tooling-limited.

2026-11-04 verdict pr-122 e4527770a69db16cf5b3a85df528d4e08c21672a pass-with-notes - F001/F002 closed; 56 exact-head targeted tests pass, both repair mutations detected, missing-holding and duplicate-index boundary controls pass; F003 P3 test-clock false failure observed after restore, eight targeted repeats pass; all four exact-head CI jobs green, local quick gate tooling-limited; new checkout frozen, no product edits.

2026-11-04 verdict pr-121 e13a834de99fe8e41e0373bc3ee0e2edb4956c8e block - P1 TERM/INT unlock while cargo descendants remain active; P2 target symlink selects a different lock for the same directory; both overlap defects reproduced with fake cargo on Bash 3.2, normal controls and non-exclusive-lock mutation pass; exact-head CI four jobs green, local quick gate tooling-limited; no real cargo or product edits.

2026-11-04 verdict pr-121 30b3a371067450e2275d3e3926736a8479a70d0c block - F001 closed by group drain (TERM, INT, normal leftovers and resistant-worker KILL pass); F002 existing-symlink case closed but initially missing target/ selects target/.lock while target selects target.lock, reproducing overlap; 13 repair groups and three guard mutations pass, focused residual reproduces, CI four jobs/10 tests green; no real cargo or product edits.

2026-11-04 verdict pr-121 2ecbd26a8437593f03e27b1b7e412862d7900baf pass-with-notes - F001/F002 closed; exact trailing-slash reproducer and four separator/symlink variants serialize, normalization mutation detected; final two-file delta clean, exact-head CI four jobs/11 tests green; local quick gate tooling-limited, no real cargo or product edits, no correctness changes owed.

2026-11-06 verdict pr-121 eff6286758eae1d8f825805f3ceda04dbf6c8ffb pass-with-notes - owner-hardening delta only; 0700 under umask 002, occupied-owner refusal across five node kinds, waiter/failure behavior and mkdir failure pass; both new guard mutations detected, exact-head CI four jobs/13 tests green; local quick gate tooling-limited, security seam verdict independently owned, no correctness changes owed.

2026-11-06 verdict pr-125 9384397762d2e7955ceff0de373640b85553227b pass-with-notes - C26 forward-only rename preserves all prior ids, rows and changelogs; four ddocs validate/render clean, F12-F16 corroborated, exact-head CI four jobs green; P3 merge-source preservation acceptance note, no blocker; local quick gate tooling-limited, no cargo or product edits.

2026-11-06 verdict pr-126 04144d1ca39130106202322d3479273c636a388a block - F001 P2 generated commands drop --project and can commit roles into the wrong governance branch; two-project replay reproduces, scoped control passes; templates/version/drift/embedding mutation and restore pass, CI four jobs green; unchanged #122 test-clock precondition fails, local quick gate tooling-limited.

2026-11-06 verdict pr-126 6b820d0e04c89a6c7980c5cbc94f8e5258baa6aa pass-with-notes - F001 closed; scoped prompts and drift hint, independent real-shell two-project replay and old-head regression negative control pass; six template HTTP tests, exact native hint test, core/shell round trips and exact-head CI green; no correctness changes owed, local quick gate tooling-limited.

2026-11-06 verdict pr-129 fd7bf4141420f132974ece403faa5ae8c64c9982 block - F001 P2 request replay dispatches to a rotated holder while the stored holder remains old; F002 P2 overlapping instance drains both dispatch one queued request before CAS; both reproduced, unchanged-holder control and three committed HTTP cases pass; project docs validate/render, exact-head CI absent, local quick gate tooling-limited.

2026-11-06 verdict pr-129 da23eb7566844c93857ecb11ba58c4da6947c605 block - re-pinned after main merge; test-rig conflict resolution preserves template catalog and real drain hook, s05 implementation blobs unchanged, F001/F002 reproduction evidence carries forward; exact-head CI four jobs green, merged docs validate/render, both worktrees stay frozen and clean.

2026-11-06 verdict pr-129 a560297ec107c4f7168cc4400a4224d689f59b05 block - original F001/F002 closed with adopted regressions passing in exact-head JUnit; new P2 F003: committed delivering claim is not recovered after daemon interruption and vanishes from assign drains/PA feed; source-dataflow proof, no local runtime claim; CI four jobs green, 1900 tests passed, merge rig and enabled #122 case verified, no local builds/tests.

2026-11-06 verdict pr-131 a00a4cfbb287efb4bc60b292b1b168404a94677e block - P2 incomplete-scan allocation, pending receipt overwrite, unreadable-cell replacement and omitted C23 printed states; P3 changed-PM replay wording; core/store 10/10, migration-key mutation and exact-head CI pass, seven docs clean; additional HTTP probes unexecuted after two interrupted shared-lock waits, no product edits.

2026-11-06 verdict pr-130 dcc3e526043b2aa3206eae0ea036876183fb9d99 block - F001 P2 compact request replay creates duplicate routing-table FYIs although the control deduplicates; real-SQLite HTTP regression reproduces, three ordinary routing HTTP controls and exact-head CI pass, docs validate/render; compact consumption proof is qualified, mutation deferred under fleet backoff before lock acquisition/cargo, local gate tooling-limited.

2026-11-06 correction pr-131 a00a4cfbb287efb4bc60b292b1b168404a94677e block - F004 withdrawn per prime's settled design call (5), relayed by pm-001: delivered prints delivered_by and plan, not state flips. Three P2s remain (F001-F003), plus N001 P3; verdict, memory and proposed regression file corrected, state-edit test removed; CI-only backoff acknowledged, no further local runs.

2026-11-06 clarification pr-131 a00a4cfbb287efb4bc60b292b1b168404a94677e block - final verdict rests on source review plus exact-head CI; HTTP proof explicitly deferred because of backoff. PM confirmed the waiting builds were intentionally stopped as a safety brake, not a PR defect; no reviewer build window requested or needed, no further local runs.

2026-11-06 verdict pr-132 5fbcfbdc0323af2546c0c9b17a89e3a0f0e2ae22 block - three source-proven P2s: delayed actions affect revived incarnations, native held compact receipt fails typed decoding and a buffered old idle can finish the memory turn; N001 missing bound_ms; exact-head CI 4/4 and 1,909 tests pass, ddocs unavailable, no local builds/tests/probes under backoff.

2026-11-06 addendum pr-132 5fbcfbdc0323af2546c0c9b17a89e3a0f0e2ae22 block - F004 P2 in unchanged memory-note logic: Markdown-shape ownership strips hand-written .dd.md rows and unescaped linked titles accumulate duplicate generated rows; source trace only, no local execution; posted verdict held pending PM's new exact head/worktree.

2026-11-06 verdict pr-134 d85c0f3adab39895dd6776422aaa5b3ad4048d8c block - F001 P2 one ps per host-named ancestor does not implement at most one per registration; valid two-node/launcher chains need multiple reads, count fixture only has one candidate; N001 P3 standalone gate-tripwire false-negative shape; UTC/DST and unchanged identity checks sound, exact-head CI 4/4 and 1,896 tests pass, no local builds/tests/probes under backoff.

2026-11-06 verdict pr-131 09653d32a47224a61f0e2957b3862001b869e218 pass-with-notes - complete seven-file repair closes F001-F003 and N001; four adopted HTTP regressions pass, exact-head CI 4/4 and 1,913 tests green; F004 remains withdrawn. Final pin check found later GitHub head 93ca2b1, explicitly not approved; frozen worktree unchanged, CI/source only, no local runs under backoff.

2026-11-06 verdict pr-131 93ca2b1c3dc80d3ea609438e9864d9593b7e35f5 pass-with-notes - authorized five-file card/index delta reviewed; bulk cards skip delivery reads, singular state walks at most 48 indexed rows without SQL history sort/grouping; exact-head CI 4/4 and 1,914 tests pass including new query-plan guard and all four regressions; no blocker, bounded-summary qualification, no local runs under backoff.

2026-11-06 verdict pr-134 39307919c43cfb23f8a5728b371234025ce37d1c pass-with-notes - nine-file repair closes F001/N001: one registration-scoped argv batch and production-delegation guard with two negatives; exact-head CI 4/4 and 1,899 tests pass; fixture counter versus Mac receipt/live AFTER proof limits explicit, no local reviewer builds/tests/probes under backoff.

2026-11-06 verdict pr-130 9d2a746b222f53e86383d6739db01a3ec18efa5a block - seven-file delta closes F001 but introduces F002 cross-recipient FYI collision: control identity includes recipient, derived global FYI key does not; source/SQL proof and exact-head CI 4/4 with 1,900 tests, no local runs; final pin found later 1623200, explicitly not reviewed/approved, detached worktree unchanged.

2026-11-06 verdict pr-130 8bfba9de1206a0f781bd853827df38abc961ca08 pass-with-notes - five-file final delta closes F002 and retains F001 closure; recipient-scoped stable FYI key, two identity-scope regressions and cell/warning sanitizing tests pass; exact-head CI 4/4 and 1,901 tests green, no local checks needed/run; compact-consumption proof ceiling remains non-blocking, fresh worktree frozen/clean.

2026-11-06 verdict pr-132 2851637fd10be349d30329e0f49704eb12519f69 block - fourteen-file repair closes F001-F004/N001; new F005 P2 hold deadline is not enforced during prompt admission (supported short override versus UDS hold grace); source/transport proof, exact-head CI 4/4 and 1,919 tests pass, no local runs; pending task/deadline CI case required.

2026-11-06 verdict pr-131 c6aa07bfeb7316ee277f7325f873c3684e8eb4fa pass-with-notes - four-file covering-index repair closes row-window truncation; newest three distinct PRs and all addresses, both query plans and 60-address regression pass exact-head CI 4/4 with 1,915 tests; original findings remain closed, N002 nonblocking aggregate retry-provenance note, no local checks needed/run.

2026-11-06 verdict pr-132 bd8331e09db997e1ab68938a037fc72d7791a29e pass-with-notes - three-file F005 repair bounds admission and event wait with the original deadline, preserves service-owned continuation and adds delayed-admission HTTP coverage; F001-F005/N001 closed, exact-head CI 4/4 and 1,920 tests green, no local checks needed/run; fresh worktree frozen/clean.

2026-11-06 verdict pr-134 c54aeb51438ef0f20845ec5e7d560a7a2fefc02c pass-with-notes - amended C27 verified: cached single-PID per-host args reads, strict PID/row parsing and post-read kernel recheck; expected 2/2/1/0 controls and gate negatives pass, exact-head CI 4/4 with 1,900 tests; old F001 ceiling superseded, N001 closed, reported Mac receipts separate and live AFTER deferred, no local reviewer runs.

2026-11-06 verdict pr-138 7c2b6b147d158746545699226343a3fd09b7bdeb pass-with-notes - lineage/batch/window/marker/zone and shared-build owner/QoS changes verified, deterministic F003 rewrite retained; N001 P3 mixed-invalid batch diagnostics are phase-local; exact-head CI 4/4 with 1,894 Rust and 15 shared-build tests pass, no local runs, security/performance proof separately owned; final pin unchanged but GitHub DIRTY, reconciliation not approved.

2026-11-06 verdict pr-140 ebcdb101d4d73bbf0e32a27bedb2aa9992539f75 pass-with-notes - all 18 F17-F34 rows, severities and owner plans corroborated against s03/s07 dogfood and PM records; N001 parallel D-WB-14..17 allocation in #138 requires convergence reconciliation; exact-head CI 4/4 green, CLEAN, no local runs/worktree, security public-safety scan separately owned.

2026-11-06 verdict pr-140 29c09d77ab9f391f0db868cef96f7f6e5cea1352 pass - renumber-only delta removes F17/F18/F24 and keeps unchanged text for fifteen D-WB-18..32 rows; N001 closed, independent merge-tree ledger has 32 unique D-WB IDs with #138, unrelated three-file conflicts not approved; exact-head CI 4/4 green before review/publication, no local builds/tests/worktree.

2026-11-06 verdict pr-131 599747c6727140f883ccc5708f3f03a7d7f73cc7 pass-with-notes - C29 workstream/roadmap canonical counter, manual/file-derived release and both-folder PM lookup verified; Services/API/skill merge keeps claims and s07 memory; exact-head CI 4/4 with 1,945 tests, no new blocker/local checks; prior c6aa07b dispositions stand, role memory written/read through verb at writer commit 2113b1c.

