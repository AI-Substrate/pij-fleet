# Plan 179 prime verification

**Reviewer:** prime pij-broad-rykard, pij msg `01a17bdd-5b65-75c0-bfc1-dc8894a2b7e4` (2026-10-27 16:38).

**bp-0005 (docs and follow-up): ACCEPTED.** The prime read the diffs of `docs/how/pij-telegram.md`,
`docs/how/pij-rs-api.md` and `skills/pij/references/routes/peer.md`, and spot-checked them against
`crates/sidecars/src/telegram_document.rs` and `crates/sidecars/src/telegram.rs`. All of these match:

- photo limit 10 MB (`PHOTO_LIMIT_BYTES`), file limit 50 MB (`DOCUMENT_LIMIT_BYTES`), `MAX_FILES` 10, caption 1024 (`CAPTION_LIMIT`);
- the extension → method table (jpg/jpeg/png/webp → sendPhoto, mp4 → sendVideo, gif → sendAnimation, else sendDocument);
- the 400 → sendDocument fallback, for non-document methods only;
- staging permissions 0700 (dir) / 0600 (file);
- the inbound media kinds, including video_note, voice and sticker.

**Earlier prime acceptances:**
- R-ac9: CI is the gate.
- The live canary (`01a17bdb-ccde-78c3-ab05-9ef589ccc191`): GREEN, and the reply binding was taken back (seq 56317, job 4497).

**Still pending:**
- Vaughan's phone check (photo / pptx / GIF), relayed by the prime.
- Merge by a maintainer. The prime has pull-only access to AI-Substrate/pij-fleet, so Vaughan merges #71.
