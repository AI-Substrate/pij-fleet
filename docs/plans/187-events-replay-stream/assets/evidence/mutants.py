#!/usr/bin/env python3
"""Plan 187 mutants: apply each, run the tests that should catch it, restore.

usage: python3 mutants.py  (from the repo root; restores every file even on failure)
"""
import subprocess
import sys

EVENTS = "crates/daemon/src/events/mod.rs"
LIFECYCLE = "crates/daemon/src/lifecycle.rs"

BUS = ["cargo", "test", "-p", "pij-daemon", "--test", "events_bus"]
WIRE = ["cargo", "test", "-p", "pij-daemon", "--lib", "http::tests::a_multi_page"]
LOG = ["cargo", "test", "-p", "pij-daemon", "--lib", "--",
       "background::tests::each_stranded", "lifecycle::tests::a_repeated"]

MUTANTS = [
    ("collect-whole-replay", EVENTS,
     "let page = self.spine.tail_page(since, REPLAY_PAGE_ROWS).await?;",
     "let page = self.spine.tail_page(since, usize::MAX).await?;",
     [BUS]),
    ("page-boundary-drops-a-row", EVENTS,
     "Box::pin(async move { spine.tail_page(since, REPLAY_PAGE_ROWS).await })",
     "Box::pin(async move { spine.tail_page(Seq(since.0 + 1), REPLAY_PAGE_ROWS).await })",
     [BUS, WIRE]),
    ("page-boundary-repeats-a-row", EVENTS,
     "Box::pin(async move { spine.tail_page(since, REPLAY_PAGE_ROWS).await })",
     "Box::pin(async move { spine.tail_page(Seq(since.0 - 1), REPLAY_PAGE_ROWS).await })",
     [BUS, WIRE]),
    ("no-dedupe-at-replay-to-live", EVENTS,
     "                    if seq <= self.last_seq {\n                        continue;\n                    }\n",
     "",
     [BUS]),
    ("refusal-logged-every-tick", LIFECYCLE,
     "if last.get(&source) != Some(&text) && current.get(&source) != Some(&text) {",
     "if true {",
     [LOG]),
]


def run(command):
    result = subprocess.run(command, capture_output=True, text=True)
    lines = (result.stdout + result.stderr).splitlines()
    keep = [line for line in lines if line.startswith("test ") and ("FAILED" in line or "ok" in line)
            or "panicked" in line or line.strip().startswith(("left:", "right:", "assertion"))
            or line.startswith("test result")]
    return result.returncode, keep


def main():
    print("== baseline")
    for command in (BUS, WIRE, LOG):
        code, lines = run(command)
        print(f"$ {' '.join(command)} -> exit {code}")
        print("\n".join(line for line in lines if line.startswith("test result")))
        if code != 0:
            sys.exit("baseline is not green")
    for name, path, old, new, commands in MUTANTS:
        original = open(path).read()
        if original.count(old) != 1:
            sys.exit(f"mutant {name}: anchor not found exactly once in {path}")
        try:
            open(path, "w").write(original.replace(old, new))
            print(f"== mutant {name} ({path})")
            for command in commands:
                code, lines = run(command)
                print(f"$ {' '.join(command)} -> exit {code}")
                print("\n".join(lines))
        finally:
            open(path, "w").write(original)
    print("== restored")
    code, lines = run(BUS)
    print(f"$ {' '.join(BUS)} -> exit {code}")


if __name__ == "__main__":
    main()
