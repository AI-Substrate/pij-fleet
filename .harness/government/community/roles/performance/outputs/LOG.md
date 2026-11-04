# performance outputs

2026-11-04 note live-fleet-baseline 79b535b0bb304709ff272f1118258c541179dcd3 measured - daemon +40 launches/60.002652s (0.667/s); machine 309.3/s over 20s; daemon PID 97258 CPU 9.1%, RSS 121376 KiB; ping omits PID; memory/2026-11-04-live-fleet-baseline.md

2026-11-04 verdict pr:113 75cf1a150d32e2f1b2ecf3ef3f1aab4f979001eb pass-with-notes - check-only 2N direct ddocs invocations (12 for 6 docs), 0 added idle launches/s and no daemon site; relative PATH candidates are not normalized absolute; unchanged sensor run on later shared head 2b328dc6b71788fee466a0a8c1e0b7fc9720df1d passed 6 docs, exit 0, 4.337s; no new live-fleet before/after; requested-head verdict only; https://github.com/AI-Substrate/pij-fleet/pull/113#issuecomment-6621674664

