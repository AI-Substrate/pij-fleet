import json
from pathlib import Path
from runpy import run_path

Fixture = run_path(str(Path(__file__).with_name("pr121b-probes.py")))["Fixture"]


f = Fixture()
try:
    f.target.rmdir()
    first = f.start(["--", "first"], {
        "PIJ_SHARED_TARGET": str(f.target) + "/",
        "REVIEW_DELAY": "1.5",
    })
    f.until(lambda: any(e["kind"] == "start" for e in f.events()))
    first_lock = f.target / ".lock"
    assert first_lock.exists()
    second = f.start(["--", "second"])
    first.communicate(timeout=6)
    second.communicate(timeout=6)
    assert first.returncode == second.returncode == 0
    order = [e["kind"] + " " + e["name"] for e in f.events()]
    assert order == ["start first", "start second", "end second", "end first"], order
    print(json.dumps({
        "head": "30b3a371067450e2275d3e3926736a8479a70d0c",
        "bug_reproduced": True,
        "case": "initially missing target with trailing slash versus same directory without slash",
        "first_lock": "<temp>/target space/.lock",
        "second_lock": "<temp>/target space.lock",
        "order": order,
        "exported_targets": [e["target"] for e in f.events() if e["kind"] == "start"],
    }, indent=2))
finally:
    f.close()
