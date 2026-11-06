import json
from pathlib import Path
from runpy import run_path
import stat
import subprocess


Fixture = run_path(str(Path(__file__).with_name("pr121b-probes.py")))["Fixture"]
SCRIPT = Path("/Users/jordanknight/pi-hacking/fleet-review-121d/harness/scripts/shared-build.sh")
RESULTS = []


def start(f, args, extra=None, script=SCRIPT):
    child = subprocess.Popen(
        ["/bin/sh", "-c", 'umask 002 && exec /bin/bash "$0" "$@"', str(script), *args],
        cwd=f.worktree,
        env={**f.env, "PIJ_SHARED_BUILD_MKDIR": "/bin/mkdir", **(extra or {})},
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    f.children.append(child)
    return child


def private_lock(f, script=SCRIPT):
    first = start(f, ["--", "first"], {"REVIEW_DELAY": "0.8"}, script)
    f.until(lambda: any(e["kind"] == "start" for e in f.events()))
    mode = stat.S_IMODE(f.lock.stat().st_mode)
    owner = (f.lock / "owner").read_text()
    second = start(f, ["--", "second"], script=script)
    first.communicate(timeout=6)
    _, stderr = second.communicate(timeout=6)
    assert first.returncode == second.returncode == 0
    order = [e["kind"] + " " + e["name"] for e in f.events()]
    assert order == ["start first", "end first", "start second", "end second"], order
    assert "waiting for" in stderr and "seat=pij-review-fixture" in owner
    assert not f.lock.exists()
    failed = start(f, ["--", "fail", "--then", "never"], script=script)
    failed.communicate(timeout=6)
    assert failed.returncode == 7 and not f.lock.exists()
    assert not any(e["name"] == "never" for e in f.events())
    return {"mode": oct(mode), "serialized": True, "owner_readable": True, "failure_releases": True}


def planted_owner(f, kind, script=SCRIPT):
    victim = f.root / "victim"
    if kind != "dangling":
        victim.write_text("keep\n")
    f.exe("planting mkdir", """
import json, os, subprocess, sys
from pathlib import Path
made = subprocess.run(["/bin/mkdir", *sys.argv[1:]])
if made.returncode:
    sys.exit(made.returncode)
owner = Path(sys.argv[-1]) / "owner"
kind = os.environ["REVIEW_PLANT_KIND"]
victim = Path(os.environ["REVIEW_VICTIM"])
if kind in ("symlink", "dangling"):
    owner.symlink_to(victim)
elif kind == "regular":
    owner.write_text("foreign owner\\n")
elif kind == "directory":
    owner.mkdir()
elif kind == "fifo":
    os.mkfifo(owner)
""")
    child = start(f, ["--", "build"], {
        "PIJ_SHARED_BUILD_MKDIR": str(f.bin / "planting mkdir"),
        "REVIEW_PLANT_KIND": kind,
        "REVIEW_VICTIM": str(victim),
    }, script)
    _, stderr = child.communicate(timeout=6)
    untouched = not victim.exists() if kind == "dangling" else victim.read_text() == "keep\n"
    return {
        "kind": kind,
        "code": child.returncode,
        "named_refusal": "something already sits at" in stderr and "no cargo run" in stderr,
        "victim_untouched": untouched,
        "cargo_not_run": not f.events(),
        "lock_released": not f.lock.exists(),
    }


def assert_owner_refusal(result):
    assert result["code"] == 2, result
    assert all(result[key] for key in ("named_refusal", "victim_untouched", "cargo_not_run", "lock_released")), result


def mkdir_failure(f):
    f.exe("failing mkdir", "import sys\nsys.exit(1)")
    child = start(f, ["--", "build"], {"PIJ_SHARED_BUILD_MKDIR": str(f.bin / "failing mkdir")})
    _, stderr = child.communicate(timeout=6)
    assert child.returncode == 2 and "could not create the lock" in stderr
    assert not f.lock.exists() and not f.events()
    return "failed mkdir refuses without cargo or an orphaned lock"


def check(name, action):
    f = Fixture()
    try:
        RESULTS.append({"name": name, "status": "pass", "details": action(f)})
    finally:
        f.close()


def mode_control(f):
    result = private_lock(f)
    assert result["mode"] == "0o700", result
    return result


def owner_control(f, kind):
    result = planted_owner(f, kind)
    assert_owner_refusal(result)
    return result


def mode_mutation(f):
    mutated = f.root / "mode-mutant.sh"
    original = SCRIPT.read_text()
    text = original.replace('"$MKDIR" -m 0700 "$lock"', '"$MKDIR" "$lock"')
    assert text != original
    mutated.write_text(text)
    result = private_lock(f, mutated)
    assert result["mode"] != "0o700", result
    return {"mutation_detected": True, "observed_mode": result["mode"]}


def owner_mutation(f):
    mutated = f.root / "owner-mutant.sh"
    original = SCRIPT.read_text()
    begin = original.index('if [ -e "$lock/owner" ]')
    end = original.index("\n# The other tree", begin)
    text = original[:begin] + """printf '%s\\n' "$owner_line" >"$lock/owner"\n""" + original[end:]
    mutated.write_text(text)
    result = planted_owner(f, "symlink", mutated)
    caught = False
    try:
        assert_owner_refusal(result)
    except AssertionError:
        caught = True
    assert caught and not result["victim_untouched"] and not result["cargo_not_run"], result
    return {"mutation_detected": True, "scratch_victim_overwritten": True, "cargo_started": True}


check("0700-under-umask-002-with-waiter-and-failure", mode_control)
for kind in ("symlink", "dangling", "regular", "directory", "fifo"):
    check("owner-refusal-" + kind, lambda f, kind=kind: owner_control(f, kind))
check("mkdir-seam-failure-refusal", mkdir_failure)
check("private-mode-regression-mutation", mode_mutation)
check("owner-create-regression-mutation", owner_mutation)
print(json.dumps({"head": "eff6286758eae1d8f825805f3ceda04dbf6c8ffb", "results": RESULTS}, indent=2))
