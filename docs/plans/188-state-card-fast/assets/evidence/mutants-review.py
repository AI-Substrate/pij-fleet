#!/usr/bin/env python3
"""Plan 188, review of #105: the reviewer's surviving mutants, plus one per fix.

usage: python3 mutants-review.py  (from the repo root; restores every file even on failure)
"""
import importlib.util
import pathlib

HERE = pathlib.Path(__file__).parent
spec = importlib.util.spec_from_file_location("mutants", HERE / "mutants.py")
base = importlib.util.module_from_spec(spec)
spec.loader.exec_module(base)

HTTP = "crates/daemon/src/http/mod.rs"
SPINE = "crates/store/src/spine.rs"

CONTRACT = ["cargo", "test", "-p", "pij-store", "--test", "store_integration", "--",
            "the_sqlite_spine_honours_the_same_contract_the_fake_does"]
CARDS = ["cargo", "test", "-p", "pij-daemon", "--lib", "http::card_tests"]

MUTANTS = [
    # S3: latest by timestamp, not sequence.
    ("latest-by-at-not-seq", SPINE,
     [('        query.push(" ORDER BY seq DESC LIMIT 1");',
       '        query.push(" ORDER BY at DESC, seq DESC LIMIT 1");')],
     [CONTRACT]),
    # N1: the seat form of tail includes its cursor.
    ("seat-tail-inclusive-since", SPINE,
     [("WHERE seat = ?1 AND seq > ?2 ORDER BY seq", "WHERE seat = ?1 AND seq >= ?2 ORDER BY seq")],
     [CONTRACT]),
    # N2: the bulk read skips the role join.
    ("bulk-skips-roles", HTTP,
     [("    if let Err(error) = state.services.roles.join_roles(&mut seats).await {",
       "    if let Err(error) = Ok::<(), PijError>(()) {")],
     [CARDS]),
    # N2: cards filled in the order they finish.
    ("bulk-completion-order", HTTP,
     [("        slots.insert(\n            index,\n", "        slots.insert(\n            slots.len(),\n")],
     [CARDS]),
    # S1 removed: the bulk read waits for every session read.
    ("bulk-unbounded-session", HTTP,
     [("            seat_card(&state, seat, Some(LIST_STATUS_WAIT)).await",
       "            seat_card(&state, seat, None).await")],
     [CARDS]),
    # S2 removed: one bad seat fails the fleet.
    ("bulk-one-error-fails-all", HTTP,
     [("            Err(unavailable) => projection.unavailable.push(unavailable),",
       "            Err(unavailable) => return internal(\"pij cards\", unavailable.error),")],
     [CARDS]),
]


def main():
    print("== baseline")
    for command in (CONTRACT, CARDS):
        code, lines = base.run(command)
        print(f"$ {' '.join(command)} -> exit {code}")
        print("\n".join(line for line in lines if line.startswith("test result")))
        if code != 0:
            raise SystemExit("baseline is not green")
    for name, path, edits, commands in MUTANTS:
        base.apply(name, path, edits, commands)
    print("== restored")
    for command in (CONTRACT, CARDS):
        code, lines = base.run(command)
        print(f"$ {' '.join(command)} -> exit {code}")
        print("\n".join(line for line in lines if line.startswith("test result")))


if __name__ == "__main__":
    main()
