# security outputs

2026-11-04 verdict pr:110 4c5a792beab929a94a399a2a3cef57aad5696862 pass - 31 documentation files reviewed for public disclosure and unsafe procedures; notes owed: none; 2026-11-04-pr-110-4c5a792-security.md.

2026-11-04 verdict pr:111 ed41a254612d47432349656831e7804baed302c0 block - Private-repository identity in three plan/evidence locations; newer head b13f5b3a433e3b410a8e376ca0252cb67358bbab not covered; 2026-11-04-pr-111-ed41a25-security.md.

2026-11-04 verdict pr:111 b13f5b3a433e3b410a8e376ca0252cb67358bbab block - D-192-3 delta safe; all three private-repository-identity blobs unchanged; 2026-11-04-pr-111-b13f5b3-security.md.

2026-11-04 verdict pr:113 75cf1a150d32e2f1b2ecf3ef3f1aab4f979001eb pass-with-notes - No new install/permission grant or shell evaluation; 7 tests and 4 pinned probes pass; CI unavailable warning is not validation proof; governance delta through e9d3e9d clear; newer PR head not covered; 2026-11-04-pr-113-75cf1a1-security.md.

2026-11-04 verdict pr:113 2b328dc6b71788fee466a0a8c1e0b7fc9720df1d pass-with-notes - C18 docs-only delta clear, code/wiring unchanged; C20 coverage note retained; governance through 0daaf755 clear; correctness gate independent; 2026-11-04-pr-113-2b328dc-security.md.

2026-11-04 verdict pr:111 75102c5a16060632d4fe325dd519ab54f5e06cfb pass-with-notes - Prior identity blocker fixed; zero added identity hits; inherited AGENTS references need forward scrub, difficulties issue is public; governance through 7d8fd896 checked; newer head not covered; 2026-11-04-pr-111-75102c5-security.md.

2026-11-04 verdict pr:113 224512db39caf3d549b50b25190e13aa6884e97a pass-with-notes - Relative PATH skipped, pin refusal/argv boundary intact; 9 tests pass; CI/wiring/supply-chain configuration unchanged; C20 coverage note retained; governance through c4e8f951 checked; 2026-11-04-pr-113-224512d-security.md.

2026-11-04 verdict pr:111 a83c73b9fcaf4a1688e8139f60e420f403ca55cd pass-with-notes - Eight-file req-14/count delta clear; checker unchanged, no private/copy hits; inherited AGENTS follow-up retained; governance through 758a2c6c checked; live cb7a0cb not covered; 2026-11-04-pr-111-a83c73b-security.md.

2026-11-04 verdict pr:111 cb7a0cb6be670d140535dfb114b3cd42917329bf pass-with-notes - Rebase change sequences identical; conflict composition checked; merged squash tree matches requested head; worktree disappeared externally, immutable verification completed; inherited scrub note retained; governance through 6e04df6c checked; 2026-11-04-pr-111-cb7a0cb-security.md.

2026-11-04 verdict pr:114 d5fd8cf48b4ffbe636818e996798f4bac3a4d9ed block - Static tracked-symlink confinement bypass, confidence 8/10; PATH/cwd defect confirmed; 9 pure tests pass; governance through d8588bb has separate personal build-path note; C21 applied; later fix not covered; 2026-11-04-pr-114-d5fd8cf-security.md.

2026-11-04 verdict pr:116 279b0ec430edcb0b64ed39f76afbbb6ac111be20 pass-with-notes - Full six-file scrub clear; calibrated controls 4 identity/5 private path, whole-tree target hits 0; 34 bare refs allowed by C21; no further scrub owed; governance through 9e4a5c9b and scrub3cf verified; 2026-11-04-pr-116-279b0ec-security.md.

2026-11-04 verdict pr:114 4ec59a851d224b0a7736c6366ee9f05684f8b447 pass-with-notes - S1/F002 closed; six targeted tests and exact-head CI pass; external-process TOCTOU retained as nonblocking only under observed same-user governance model, not privileged/untrusted-writable deployment; governance through 94b0a775 checked; 2026-11-04-pr-114-4ec59a8-security.md.

2026-11-04 verdict pr:118 868e830398b3bcba4f1c6361a9920745723266b4 pass-with-notes - Two-file docs delta clear; writer still validates all batch documents before render/stage/commit with rollback, no safety bypass; unchanged same-user TOCTOU note retained; governance through 0e159a7c checked; 2026-11-04-pr-118-868e830-security.md.

2026-11-04 verdict pr:120 43821d5ba66ad7e6f46a976785d393f9f4af0c15 pass-with-notes - Product filters/SQL/bounds preserved; in-memory paging and identical-tree CI proof, parser checks static; anomaly history not absolutely capped; separate governance C21 blocker; 2026-11-04-pr-120-43821d5-security.md.

2026-11-04 note prime-governance 4f87bd7168b5ed569fae949d572713b3721c59d5 block - Delivery memory 2026-11-04-daemon-6.5gb-peak-is-anomalies-fold.md:7,12 publishes concrete personal DB/session paths; retained unchanged at 05c57b84f43d5470843eb0218f0d64430ad691ef; values omitted; same report.

2026-11-06 note prime-governance 590772d10bb3cb9ba69c0d32b777d7c46d2654fa resolved - Owner fix independently verified through public tip 1a2cb887bdfb428a5da09e9d38b856ff6385ab29; both original path values absent, replacements neutral, fix additions contain no copied values; no duplicate edit needed; remediation follow-up in 2026-11-04-pr-120-43821d5-security.md.

2026-11-06 verdict pr:122 e4527770a69db16cf5b3a85df528d4e08c21672a pass-with-notes - Authority/prime/close/confinement/adopt/argv boundaries preserved; pending-end/holding fix assessed; four CI checks green, identical-tree 1874-case evidence; no local Cargo; governance proof limit explicit; 2026-11-06-pr-122-e452777-security.md.

2026-11-06 verdict pr:121 2ecbd26a8437593f03e27b1b7e412862d7900baf block - Conditional MEDIUM cross-user owner symlink overwrite under permissive umask, static 9/10; private atomic lock mode required; green CI not cross-user proof; 2026-11-06-pr-121-2ecbd26-security.md.

2026-11-06 note prime-governance 51cb8bd3472352f72b76984cf31c1e1ff8a750c1 block - Reviewer artifact outputs/2026-11/pr121b-trailing-slash.json:14-15 retains concrete temporary-runtime values, literals omitted; forward neutralization routed; PR121 report.

2026-11-06 verdict pr:121 eff6286758eae1d8f825805f3ceda04dbf6c8ffb pass-with-notes - Original MEDIUM closed by atomic0700/owner-noclobber fail-closed creation; MKDIR seam is trusted caller override outside tests, no new lower-trust capability found; four CI jobs green, proof limits explicit; separate governance artifact values persist at 3b153235; 2026-11-06-pr-121-eff6286-security.md.

2026-11-06 verdict pr:125 9384397762d2e7955ceff0de373640b85553227b pass - Complete eight-file C26 PEC rename-forward docs reviewed; no private/host-state content or unsafe advice; validation delegation intact; no tests/builds/CI claim; 2026-11-06-pr-125-9384397-security.md.

2026-11-06 note prime-governance edf81257e776acf448f9aea71d0b2300b1adbb3b resolved - Reviewer artifact entries neutral; both original values absent across tracked fix/public-tip snapshots through 43852c29395ae72d203c2b247f9383c5dcc8910b; values omitted, no duplicate edit; PR125 and PR121 repair follow-up.

2026-11-06 verdict pr:126 04144d1ca39130106202322d3479273c636a388a pass-with-notes - Embedded selectors and brief confinement safe; six packets public-safe; prompts/list/diff do not create or execute; CI merge tree equals head, Rust1887pass5skip; real-ddocs UNAVAILABLE/early-return limit explicit; no local build; 2026-11-06-pr-126-04144d1-security.md.

2026-11-06 verdict pr:126 6b820d0e04c89a6c7980c5cbc94f8e5258baa6aa pass-with-notes - Seven-file project-quoting delta safe; all printed commands select viewed project; 29 values/58 inert argv roundtrips and identical-tree CI proof; real-ddocs limit carried; no build/replay; 2026-11-06-pr-126-6b820d0-security.md.

2026-11-06 note pr:129 fd7bf4141420f132974ece403faa5ae8c64c9982 block - INTERIM stale assignment drain sends later requests to released holder, MEDIUM9/10, in-memory migrations/SELECT proof; no transport or old-head PR verdict; replacement head/CI and remaining review pending; 2026-11-06-pr-129-fd7bf41-interim-security.md.

2026-11-06 verdict pr:129 da23eb7566844c93857ecb11ba58c4da6947c605 block - Final merged head retains identical vulnerable drain blob; released holder can receive subsequently queued requests, MEDIUM9/10; in-memory proof carries; four CI jobs green/identical tree do not cover race; audit/stage qualifications recorded; 2026-11-06-pr-129-da23eb7-security.md.

2026-11-06 verdict pr:130 dcc3e526043b2aa3206eae0ea036876183fb9d99 pass-with-notes - Holder snapshots excluded from file, auth/hooks/held FYI boundaries preserved; rendering not fully literal and compact hook is admission-timed, not completion proof; identical-tree CI1899 cases, ddocs caveat; no local mutations; 2026-11-06-pr-130-dcc3e52-security.md.

2026-11-06 note pr:132 5fbcfbdc0323af2546c0c9b17a89e3a0f0e2ae22 block - INTERIM delayed-close authorization/incarnation discarded, MEDIUM9/10; bound/held-task gaps and evidence-path candidate routed for pending design; no old-head post or new-head verdict; CI/source only; 2026-11-06-pr-132-5fbcfbd-interim-security.md.

2026-11-06 verdict pr:134 d85c0f3adab39895dd6776422aaa5b3ad4048d8c pass-with-notes - Supported-name/argv/session guards and safe bounds retained; no first-party unsafe, fixed absolute ps argv; no macOS runtime certification or universal name/exec equivalence; global one-ps cap not enforced; identical-tree Linux CI proof, no local probes/builds; 2026-11-06-pr-134-d85c0f3-security.md.

2026-11-06 verdict pr:131 09653d32a47224a61f0e2957b3862001b869e218 pass-with-notes - Explicitly requested SNAPSHOT ONLY; project labels preserved in JSON/human cards, fixes/quote/auth proof checked, CI green; live API/Git ref 93ca2b1 is NOT approved and owner notified; no local validation under backoff; 2026-11-06-pr-131-09653d3-security.md.

2026-11-06 verdict pr:131 93ca2b1c3dc80d3ea609438e9864d9593b7e35f5 pass-with-notes - PM explicitly re-pinned live head; complete five-file indexed state-only read delta safe, PM selection/project labels retained; 48-row display completeness limit recorded; four CI jobs green, identical-tree 1914-case proof; no local validation under backoff; 2026-11-06-pr-131-93ca2b1-security.md.

2026-11-06 verdict pr:134 39307919c43cfb23f8a5728b371234025ce37d1c block - MEDIUM9/10 raw-ucomm row injection plus duplicate PID overwrite can substitute ancestor harness/session evidence; source-derived exec-window case, no local exploit; one-call/tripwire fixes present, green identical-tree CI lacks parser negatives and macOS compile coverage; 2026-11-06-pr-134-3930791-security.md.

2026-11-06 verdict pr:132 bd8331e09db997e1ab68938a037fc72d7791a29e pass-with-notes - Prior delayed-close MEDIUM resolved; S1-S5 and F005 verified, authority/incarnation/CAS and shared admission deadline retained; four green CI checks, identical-tree 1920-case proof; restart/timing/ddocs limits explicit; 2026-11-06-pr-132-bd8331e-security.md.

2026-11-06 verdict pr:130 8bfba9de1206a0f781bd853827df38abc961ca08 pass-with-notes - F001/F002 recipient-qualified replay key and control/bidi filtering verified; held-FYI/auth/writer boundaries retained; four green CI checks, identical-tree 1901-case proof; Markdown/admission timing/ddocs limits explicit; 2026-11-06-pr-130-8bfba9d-security.md.

2026-11-06 verdict pr:131 c6aa07bfeb7316ee277f7325f873c3684e8eb4fa pass-with-notes - Covering-index newest-three project/PR selection and complete PK address reads verified; seat-global PM scope and per-group project labels retained; four green CI checks, identical-tree 1915-case proof including 60-address regression; ddocs/static cross-project limits explicit; 2026-11-06-pr-131-c6aa07b-security.md.

2026-11-06 note pr:131 c6aa07bfeb7316ee277f7325f873c3684e8eb4fa held-for-next-head - PM C29 round arrived after publication; existing verdict covers this snapshot only, not C29/replacement; further review waits for fresh PM pin/worktree; #132/#130 unchanged and complete.

2026-11-06 verdict pr:138 7c2b6b147d158746545699226343a3fd09b7bdeb pass-with-notes - F15/F16/F18/F24 and wrapper boundaries reviewed; marker/seat labels advisory, not authority; four green checks, identical-tree 1894-case Rust and 15 shared-build-test evidence; live PR DIRTY/CONFLICTING, no replacement-head/merge clearance; macOS/ddocs limits explicit; 2026-11-06-pr-138-7c2b6b1-security.md.

2026-11-06 verdict pr:140 ebcdb101d4d73bbf0e32a27bedb2aa9992539f75 pass - Docs-only public-safety scan; complete 71-line file and 18 additions clear, 18/18 positive controls; no private paths/identities/content found; exact head CLEAN and four green checks before publication; correctness/severity/owner-plan review excluded; 2026-11-06-pr-140-ebcdb10-security.md.

2026-11-06 note pr:140 ebcdb101d4d73bbf0e32a27bedb2aa9992539f75 held-for-next-head - PM renumbering/drop notice arrived after publication; existing pass is this snapshot only. Await replacement pin; next review limited to renumbering from D-WB-18 and removal of F17/F18/F24 owned by #138. PM informed of crossed publication.

2026-11-06 verdict pr:140 29c09d77ab9f391f0db868cef96f7f6e5cea1352 pass - Requested delta-only public-safety scan; sole parent ebcdb10, only difficulties.md, 33 changed lines clear with calibrated controls; separate reviewer owns numbering/duplicates/merge correctness; final exact head CLEAN and four green checks before publication; 2026-11-06-pr-140-29c09d7-security.md.

2026-11-06 verdict pr:131 599747c6727140f883ccc5708f3f03a7d7f73cc7 pass-with-notes - C29 path/claim/release normalization and PM routing boundaries retained; inherited evidence machine-label candidate routed for neutralization/classification, private status unverified and value withheld; exact head CLEAN/four green checks, identical-tree 1945-case proof; same-user/static/ddocs limits explicit; 2026-11-06-pr-131-599747c-security.md.

2026-11-06 friction security-memory-add blocked - E-RS-GOV-FOREIGN-EDIT on roles/security/memory/INDEX.md carrying pre-s07 uncommitted edits; supported verb wrote nothing. PM notified for prime baseline commit before retry; no index cleanup, manual memory write or governance commit attempted. PR #131 comment 6677181209 read-back verified.

2026-11-06 verdict pr:142 d2235f542b6953dc37790cf96ee2289953515ffa pass - Docs-only public-safety delta: PRD JSON/generated Markdown, 21 changed lines clear; calibrated positive/allowed-reference controls pass; exact head CLEAN and four green checks before publication; row/changelog correctness excluded; 2026-11-06-pr-142-d2235f5-security.md.

2026-11-06 verdict pr:143 fdb0b2745c4a194227f0d405b99a089a4e92c39c pass - Docs-only public-safety delta: difficulties.md 12 additions clear; four positive categories and eight allowed-reference controls pass; exact head CLEAN and four green checks before publication; owner/severity/correspondence/merge-tree correctness excluded; 2026-11-06-pr-143-fdb0b27-security.md.

2026-11-06 verdict pr:134 c54aeb51438ef0f20845ec5e7d560a7a2fefc02c pass-with-notes - S001 cross-PID/raw-ucomm substitution closed; amended C27 single-PID host-only cached/fail-closed reads verified; four green checks, identical-tree 1900-case proof; current PR DIRTY/CONFLICTING, native evidence attributed rather than exact-head certified; no replacement-head/merge clearance; 2026-11-06-pr-134-c54aeb5-security.md.

2026-11-06 note pr:130 8bfba9de1206a0f781bd853827df38abc961ca08 pass-with-notes - Queued-request delta reread agrees with existing filed/posted verdict, which was preserved and read back rather than duplicated; nonblocking custom-ID delimiter collision qualifies universal F002 claims, latest DIRTY merge gate independent; exact PM line re-sent; 2026-11-06-pr-130-8bfba9d-qualification.md.

