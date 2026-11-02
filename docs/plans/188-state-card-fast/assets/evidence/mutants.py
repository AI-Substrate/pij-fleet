#!/usr/bin/env python3
"""Plan 188 mutants: apply each, run the tests that should catch it, restore.

usage: python3 mutants.py  (from the repo root; restores every file even on failure)
"""
import subprocess
import sys

HTTP = "crates/daemon/src/http/mod.rs"
REPORT = "crates/core/src/report.rs"
SPINE = "crates/store/src/spine.rs"

CARDS = ["cargo", "test", "-p", "pij-daemon", "--lib", "http::card_tests"]
REPORT_HTTP = ["cargo", "test", "-p", "pij-daemon", "--test", "report_http", "--",
               "state_note", "bulk_card"]
PLANS = ["cargo", "test", "-p", "pij-store", "--test", "store_integration", "--",
         "a_seat_tail_seeks", "all_matching_seeks"]

MUTANTS = [
    # A read falls back to a spine scan: four places it could.
    ("scan-card-and-note-via-tail", REPORT,
     [("        self.spine.latest_matching(seat, &[kind]).await",
       "        Ok(self.spine.tail(Some(seat), Seq(0)).await?.into_iter().rev()"
       ".find(|event| event.kind == kind))")],
     [CARDS]),
    ("scan-holds-via-tail", HTTP,
     [("        .all_matching(seat, &HOLD_EVENT_KINDS)\n",
       "        .tail(Some(seat), pij_core::model::Seq(0))\n")],
     [CARDS]),
    ("seat-tail-or-defeats-index", SPINE,
     [("INDEXED BY spine_by_seat WHERE seat = ?1 AND seq > ?2 ORDER BY seq",
       "WHERE seq > ?2 AND (?1 IS NULL OR seat = ?1) ORDER BY seq")],
     [PLANS]),
    ("holds-without-indexed-by", SPINE,
     [("INDEXED BY spine_by_seat_kind WHERE seat = ?1 AND kind IN",
       "WHERE seat = ?1 AND kind IN")],
     [PLANS]),
    # Bulk and single cards diverge.
    ("bulk-drops-held", HTTP,
     [("            Ok((index, Ok(card))) => cards[index] = Some(card),",
       "            Ok((index, Ok(mut card))) => {\n"
       "                card.card.held.clear();\n"
       "                cards[index] = Some(card);\n"
       "            }")],
     [CARDS, REPORT_HTTP]),
    ("bulk-includes-tombstoned", HTTP,
     [("            .filter(|seat| seat.tombstoned_at.is_none())\n            .collect::<Vec<_>>(),\n"
       "        Err(error) => return internal(\"pij cards\", error),",
       "            .collect::<Vec<_>>(),\n"
       "        Err(error) => return internal(\"pij cards\", error),")],
     [CARDS]),
    # Note timestamp taken from the wrong record (the card's).
    ("note-time-from-card", HTTP,
     [("                Some(_) => (Some(state.at), Some(state.seq.0)),",
       "                Some(_) => (\n"
       "                    card.as_ref().map(|status| status.card.at),\n"
       "                    card.as_ref().and_then(|status| status.card.seq).map(|seq| seq.0),\n"
       "                ),")],
     [CARDS, REPORT_HTTP]),
    ("note-time-kept-after-clear", HTTP,
     [("                None => (None, None),\n            };",
       "                None => (Some(state.at), Some(state.seq.0)),\n            };")],
     [REPORT_HTTP]),
]


def run(command):
    result = subprocess.run(command, capture_output=True, text=True)
    lines = (result.stdout + result.stderr).splitlines()
    keep = [line for line in lines if line.startswith("test ") and ("FAILED" in line or "ok" in line)
            or "panicked" in line or line.strip().startswith(("left:", "right:", "assertion"))
            or line.startswith("test result") or line.startswith("error[")]
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
    for command in (CARDS, REPORT_HTTP, PLANS):
        code, lines = run(command)
        print(f"$ {' '.join(command)} -> exit {code}")
        print("\n".join(line for line in lines if line.startswith("test result")))
        if code != 0:
            sys.exit("baseline is not green")
    for name, path, edits, commands in MUTANTS:
        apply(name, path, edits, commands)
    print("== restored")
    for command in (CARDS, REPORT_HTTP, PLANS):
        code, lines = run(command)
        print(f"$ {' '.join(command)} -> exit {code}")
        print("\n".join(line for line in lines if line.startswith("test result")))


if __name__ == "__main__":
    main()
