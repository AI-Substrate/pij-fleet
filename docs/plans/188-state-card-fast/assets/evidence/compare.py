import json, sys
main = [json.loads(l) for l in open(sys.argv[1]) if l.strip()]
branch = {c["id"]: c for c in (json.loads(l) for l in open(sys.argv[2]) if l.strip())}
same = differ = 0
new_keys = {"stateNoteAt", "stateNoteSeq"}
notes = with_note = 0
for card in main:
    b = dict(branch[card["id"]])
    assert new_keys <= b.keys(), card["id"]
    if b.get("stateNote") is not None:
        with_note += 1
        assert b["stateNoteAt"] is not None and b["stateNoteSeq"] is not None
    else:
        assert b["stateNoteAt"] is None and b["stateNoteSeq"] is None
    for k in new_keys: b.pop(k)
    if b == card: same += 1
    else:
        differ += 1
        diff = {k: (card.get(k), b.get(k)) for k in set(card) | set(b) if card.get(k) != b.get(k)}
        print("DIFF", card["id"], json.dumps(diff)[:400])
held = sum(1 for c in main if c.get("held"))
cards = sum(1 for c in main if c.get("statusSeq") is not None)
print(f"compared={len(main)} identical={same} differ={differ} with_card={cards} with_held={held} with_note={with_note}")
