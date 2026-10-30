#!/usr/bin/env python3
"""Plan 187 mutants: apply each, run the tests that should catch it, restore.

usage: python3 mutants.py  (from the repo root; restores every file even on failure)
"""
import subprocess
import sys

EVENTS = "crates/daemon/src/events/mod.rs"
LIFECYCLE = "crates/daemon/src/lifecycle.rs"
HTTP = "crates/daemon/src/http/mod.rs"
BUS_TESTS = "crates/daemon/tests/events_bus.rs"

BUS = ["cargo", "test", "-p", "pij-daemon", "--test", "events_bus"]
WIRE = ["cargo", "test", "-p", "pij-daemon", "--lib", "http::tests::a_multi_page"]
LOG = ["cargo", "test", "-p", "pij-daemon", "--lib", "--",
       "background::tests::each_stranded", "lifecycle::tests::a_repeated"]
BODY = ["cargo", "test", "-p", "pij-daemon", "--lib", "http::tests::a_failed_replay_page"]
OVERLAP = ["cargo", "test", "-p", "pij-daemon", "--test", "events_bus", "replay_and_live_overlap"]

# Review S2: the live receiver is created AFTER the first page instead of before.
LATE_RECEIVER = [
    ("        let receiver = self.live.subscribe();\n        let since = since.unwrap_or(Seq(0));",
     "        let since = since.unwrap_or(Seq(0));"),
    ("            live: BroadcastStream::new(receiver),\n            filter,\n            last_seq: since,",
     "            live: BroadcastStream::new(self.live.subscribe()),\n            filter,\n            last_seq: since,"),
    ("        let page = self.spine.tail_page(since, REPLAY_PAGE_ROWS).await?;\n",
     "        let page = self.spine.tail_page(since, REPLAY_PAGE_ROWS).await?;\n"
     "        subscription.live = BroadcastStream::new(self.live.subscribe());\n"),
]

MUTANTS = [
    ("collect-whole-replay", EVENTS,
     [("let page = self.spine.tail_page(since, REPLAY_PAGE_ROWS).await?;",
       "let page = self.spine.tail_page(since, usize::MAX).await?;")],
     [BUS]),
    ("page-boundary-drops-a-row", EVENTS,
     [("Box::pin(async move { spine.tail_page(since, REPLAY_PAGE_ROWS).await })",
       "Box::pin(async move { spine.tail_page(Seq(since.0 + 1), REPLAY_PAGE_ROWS).await })")],
     [BUS, WIRE]),
    ("page-boundary-repeats-a-row", EVENTS,
     [("Box::pin(async move { spine.tail_page(since, REPLAY_PAGE_ROWS).await })",
       "Box::pin(async move { spine.tail_page(Seq(since.0 - 1), REPLAY_PAGE_ROWS).await })")],
     [BUS, WIRE]),
    ("no-dedupe-at-replay-to-live", EVENTS,
     [("                    if seq <= self.last_seq {\n                        continue;\n                    }\n", "")],
     [BUS]),
    ("refusal-logged-every-tick", LIFECYCLE,
     [("if last.get(&source) != Some(&text) && current.get(&source) != Some(&text) {", "if true {")],
     [LOG]),
    # Review fixes (S1-S3).
    ("S2-receiver-after-first-page", EVENTS, LATE_RECEIVER, [OVERLAP]),
    ("S1-local-end-does-not-end-the-body", HTTP,
     [("let local = local.map(Some).chain(tokio_stream::once(None));", "let local = local.map(Some);")],
     [BODY]),
    ("S1-clean-end-instead-of-error", HTTP,
     [("            .chain(ended),", "            .chain(ended.filter(|item| item.is_ok())),")],
     [BODY]),
    ("S3-lag-gap-skipped-silently", EVENTS,
     [("if self.lagged && self.ends_on_gap && seq.0 != self.last_seq.0 + 1 {", "if false {")],
     [BUS]),
    ("S3-any-lag-ends-the-stream", EVENTS,
     [("if self.lagged && self.ends_on_gap && seq.0 != self.last_seq.0 + 1 {", "if self.lagged && self.ends_on_gap {")],
     [BUS]),
]



def run(command):
    result = subprocess.run(command, capture_output=True, text=True)
    lines = (result.stdout + result.stderr).splitlines()
    keep = [line for line in lines if line.startswith("test ") and ("FAILED" in line or "ok" in line)
            or "panicked" in line or line.strip().startswith(("left:", "right:", "assertion"))
            or line.startswith("test result")]
    return result.returncode, keep


def apply(name, path, edits, commands):
    original = open(path).read()
    mutated = original
    for old, new in edits:
        if mutated.count(old) != 1:
            sys.exit(f"mutant {name}: anchor not found exactly once in {path}: {old[:60]!r}")
        mutated = mutated.replace(old, new)
    try:
        open(path, "w").write(mutated)
        print(f"== mutant {name} ({path})")
        for command in commands:
            code, lines = run(command)
            print(f"$ {' '.join(command)} -> exit {code}")
            print("\n".join(lines))
    finally:
        open(path, "w").write(original)


def main():
    print("== baseline")
    for command in (BUS, WIRE, LOG, BODY):
        code, lines = run(command)
        print(f"$ {' '.join(command)} -> exit {code}")
        print("\n".join(line for line in lines if line.startswith("test result")))
        if code != 0:
            sys.exit("baseline is not green")
    for name, path, edits, commands in MUTANTS:
        apply(name, path, edits, commands)
    # Control for S2: the same mutant against the overlap test as it was at
    # 0b05852 (pause, THEN snapshot) survives, as the reviewer found.
    current_tests = open(BUS_TESTS).read()
    try:
        old_tests = subprocess.run(["git", "show", f"0b05852:{BUS_TESTS}"],
                                   capture_output=True, text=True, check=True).stdout
        open(BUS_TESTS, "w").write(old_tests)
        apply("S2-control-old-overlap-test", EVENTS, LATE_RECEIVER, [OVERLAP])
    finally:
        open(BUS_TESTS, "w").write(current_tests)
    print("== restored")
    for command in (BUS, WIRE, LOG, BODY):
        code, lines = run(command)
        print(f"$ {' '.join(command)} -> exit {code}")


if __name__ == "__main__":
    main()
