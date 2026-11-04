#!/usr/bin/env python3
"""Plan 192 checks (bp-0001, bp-0002). One-off evidence script; never runs in the product.

Reads the frozen register by sha from a read-only checkout of the private repo and
prints COUNTS ONLY. No source text is printed or written.

    python3 docs/plans/192-prd-register-move/assets/evidence/check.py <legacy-repo> [sha]

<legacy-repo> is the private repo's checkout (it is not named here). sha defaults to e05661c8.
The private repo's owner/name is read from that checkout's origin at run time, never stored here.
`check.py --self-test` proves the identity detector on a synthetic name.
"""

import hashlib
import json
import re
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[4]
PRD = REPO / "docs/project/prds/002-pij-base.dd.json"
TRIAGE = HERE / "triage.md"
SRC_PATH = ".harness/government/prd/base-prd.dd.json"
OPEN_STATES = {"unchecked", "open"}
VERDICTS = {"kept", "dropped-met", "dropped-unwanted"}

# Other projects' and repos' names, as sha256 prefixes so this public file doesn't name them.
DENY_WORD_HASHES = {
    "62a0fc3ae94d3390",
    "304b5ff3e128a070",
    "823ae2269ea1fb17",
    "54d0c95b96e8c8ae",
    "f03d9a2cabe1e981",
    "4974615bbfb08afb",
    "b65443c574c456c1",
}
SCRUB_PATTERNS = {
    "machine path": re.compile(r"(/Users/|/home/|~/|/private/tmp|/tmp/)"),
    "PR or issue number": re.compile(r"(#\d{2,}|\bPR ?\d+|/pull/\d+|/issues/\d+)"),
    "other repository": re.compile(r"(github\.com/|[A-Za-z0-9-]+/[A-Za-z0-9-]+#\d+)"),
    "credential-shaped": re.compile(r"(ghp_[A-Za-z0-9]{10,}|github_pat_|sk-[A-Za-z0-9]{16,}|AKIA[A-Z0-9]{12}|-----BEGIN)"),
    "old plan number": re.compile(r"\bplan \d{2,3}\b", re.I),
}


def source_identities(legacy):
    """owner/name of the source checkout's origin, learned at run time so this file names nothing real."""
    url = subprocess.run(
        ["git", "-C", legacy, "remote", "get-url", "origin"], check=True, capture_output=True, text=True
    ).stdout.strip()
    match = re.search(r"[:/]([^/:]+)/([^/]+?)(?:\.git)?$", url)
    if not match:
        sys.exit("could not read owner/name from the source checkout's origin")
    owner, name = match.groups()
    return {f"{owner}/{name}".lower(), f"{name}#".lower()}


def identity_hits(text, identities):
    lower = text.lower()
    return sum(lower.count(i) for i in identities)


def self_test():
    """The detector fires on a planted synthetic identity and stays quiet without it."""
    identities = {"example-org/example-private", "example-private#"}
    assert identity_hits("see example-org/example-private#12 for it", identities) == 2
    assert identity_hits("nothing private here", identities) == 0
    print("self-test ok")


def words(text):
    return re.findall(r"[a-z0-9]+(?:[-'][a-z0-9]+)*", text.lower())


def shingles(text, n=8):
    w = words(text)
    return {" ".join(w[i : i + n]) for i in range(len(w) - n + 1)}


def strings(value):
    if isinstance(value, str):
        yield value
    elif isinstance(value, dict):
        for v in value.values():
            yield from strings(v)
    elif isinstance(value, list):
        for v in value:
            yield from strings(v)


def main():
    if sys.argv[1:] == ["--self-test"]:
        self_test()
        return
    if len(sys.argv) < 2:
        sys.exit(__doc__)
    legacy, sha = sys.argv[1], (sys.argv[2] if len(sys.argv) > 2 else "e05661c8")
    raw = subprocess.run(
        ["git", "-C", legacy, "show", f"{sha}:{SRC_PATH}"], check=True, capture_output=True, text=True
    ).stdout
    rows = next(s["value"] for s in json.loads(raw)["sections"] if s["name"] == "requirements")
    open_ids = {r["id"] for r in rows if r.get("state") in OPEN_STATES}
    source_text = "\n".join(strings(rows))
    identities = source_identities(legacy)

    prd = json.loads(PRD.read_text())
    prd_ids = {r["id"] for s in prd["sections"] if s["name"] == "requirements" for r in s["value"]}
    triage_text = TRIAGE.read_text()

    # bp-0001: one verdict per open row; kept rows name a PRD 002 id that exists.
    verdicts = {}
    problems = []
    for line in triage_text.splitlines():
        cells = [c.strip() for c in line.strip().strip("|").split("|")]
        if len(cells) < 3 or not re.fullmatch(r"req-\d{4}", cells[0]):
            continue
        old, verdict, new = cells[0], cells[1], cells[2]
        if old in verdicts:
            problems.append(f"duplicate verdict for {old}")
        verdicts[old] = verdict
        if verdict not in VERDICTS:
            problems.append(f"{old}: unknown verdict")
        if verdict == "kept" and new not in prd_ids:
            problems.append(f"{old}: kept as {new}, not in PRD 002")
    kept_targets = {line.split("|")[3].strip() for line in triage_text.splitlines() if "| kept |" in line}
    orphans = prd_ids - kept_targets
    missing = open_ids - verdicts.keys()
    extra = verdicts.keys() - open_ids
    counts = {v: sum(1 for x in verdicts.values() if x == v) for v in sorted(VERDICTS)}
    print(f"source sha {sha}: {len(rows)} rows, {len(open_ids)} open")
    print(f"verdicts: {len(verdicts)} ({', '.join(f'{k} {v}' for k, v in counts.items())})")
    print(f"open rows without a verdict: {len(missing)}; verdicts for non-open ids: {len(extra)}")
    print(f"PRD 002 requirement rows: {len(prd_ids)}; rows no kept verdict points at: {len(orphans)}")
    print(f"problems: {len(problems)}")
    for p in problems:
        print(f"  {p}")

    # bp-0002: scrub and copy check over PRD 002 and the triage record.
    targets = {"PRD 002": "\n".join(strings(prd)), "triage.md": triage_text}
    scrub_hits = 0
    for name, text in targets.items():
        for label, pattern in SCRUB_PATTERNS.items():
            n = len(pattern.findall(text))
            scrub_hits += n
            print(f"scrub {name}: {label}: {n}")
        n = identity_hits(text, identities)
        scrub_hits += n
        print(f"scrub {name}: source repository identity: {n}")
        n = sum(1 for w in set(words(text)) if hashlib.sha256(w.encode()).hexdigest()[:16] in DENY_WORD_HASHES)
        scrub_hits += n
        print(f"scrub {name}: other project names: {n}")
    src = shingles(source_text)
    copied = 0
    for name, text in targets.items():
        n = len(src & shingles(text))
        copied += n
        print(f"copy check {name}: shared runs of 8+ words with the source: {n}")

    ok = not (missing or extra or problems or orphans or scrub_hits or copied)
    print("RESULT:", "ok" if ok else "FAIL")
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
