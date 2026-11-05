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

