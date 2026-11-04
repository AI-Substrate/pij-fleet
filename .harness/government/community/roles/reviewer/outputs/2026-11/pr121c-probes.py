import json
from pathlib import Path
from runpy import run_path


FIXTURE_FILE = Path(__file__).with_name("pr121b-probes.py")
Fixture = run_path(str(FIXTURE_FILE))["Fixture"]
SCRIPT = Path("/Users/jordanknight/pi-hacking/fleet-review-121c/harness/scripts/shared-build.sh")
RESULTS = []


def exclusion(f, spelling, canonical, script=SCRIPT):
    first = f.start(["--", "first"], {
        "PIJ_SHARED_TARGET": spelling,
        "REVIEW_DELAY": "0.7",
    }, script)
    f.until(lambda: any(e["kind"] == "start" for e in f.events()))
    first_lock = Path(canonical + ".lock")
    canonical_lock_held = first_lock.exists()
    interior_lock_held = (Path(canonical) / ".lock").exists()
    second = f.start(["--", "second"], {"PIJ_SHARED_TARGET": canonical}, script)
    first.communicate(timeout=6)
    _, stderr = second.communicate(timeout=6)
    assert first.returncode == second.returncode == 0
    order = [e["kind"] + " " + e["name"] for e in f.events()]
    targets = [e["target"] for e in f.events() if e["kind"] == "start"]
    return {
        "order": order,
        "targets_match": targets == [canonical, canonical],
        "canonical_lock_held": canonical_lock_held,
        "interior_lock_held": interior_lock_held,
        "second_waited": "waiting for" in stderr,
        "released": not first_lock.exists(),
    }


def assert_exclusion(result):
    assert result["order"] == ["start first", "end first", "start second", "end second"], result
    assert result["targets_match"], result
    assert result["canonical_lock_held"] and not result["interior_lock_held"], result
    assert result["second_waited"] and result["released"], result


def check(name, prepare, mutant=False):
    f = Fixture()
    try:
        spelling, canonical = prepare(f)
        script = SCRIPT
        if mutant:
            script = f.root / "no-normalization.sh"
            original = SCRIPT.read_text()
            text = original.replace('path=$(normal_path "$1")', "path=$1")
            text = text.replace('normal_path "$resolved$rest"', """printf '%s' "${resolved%/}$rest" """)
            assert text != original
            script.write_text(text)
        result = exclusion(f, spelling, canonical, script)
        if mutant:
            detected = False
            try:
                assert_exclusion(result)
            except AssertionError:
                detected = True
            assert detected, "normalization mutant escaped the exclusion oracle"
            result["mutation_detected"] = True
        else:
            assert_exclusion(result)
        RESULTS.append({"name": name, "status": "pass", "details": result})
    finally:
        f.close()


def missing_slash(f):
    f.target.rmdir()
    return str(f.target) + "/", str(f.target)


def missing_repeated(f):
    f.target.rmdir()
    return str(f.target).replace("/", "//") + "///", str(f.target)


def existing_repeated(f):
    return str(f.target).replace("/", "//") + "///", str(f.target)


def missing_symlink_suffix(f):
    alias = f.root / "alias"
    alias.symlink_to(f.target, target_is_directory=True)
    return str(alias) + "//missing///built///", str(f.target / "missing" / "built")


def existing_symlink(f):
    alias = f.root / "alias"
    alias.symlink_to(f.target, target_is_directory=True)
    return str(alias) + "///", str(f.target)


check("original-missing-target-trailing-slash-reproducer", missing_slash)
check("missing-target-doubled-and-trailing-separators", missing_repeated)
check("existing-target-doubled-and-trailing-separators", existing_repeated)
check("missing-target-under-symlink-repeated-separators", missing_symlink_suffix)
check("existing-target-symlink-trailing-separators", existing_symlink)
check("normalization-regression-mutation", missing_slash, mutant=True)
print(json.dumps({
    "head": "2ecbd26a8437593f03e27b1b7e412862d7900baf",
    "results": RESULTS,
}, indent=2))
