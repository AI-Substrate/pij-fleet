import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import tempfile
import time


SOURCE = Path("/Users/jordanknight/pi-hacking/fleet-review-121b")
SCRIPT = SOURCE / "harness/scripts/shared-build.sh"
RESULTS = []


class Fixture:
    def __init__(self):
        self.temp = tempfile.TemporaryDirectory(prefix="pr121-review-")
        self.root = Path(self.temp.name).resolve()
        self.worktree = self.root / "work tree"
        self.worktree.mkdir()
        subprocess.run(["git", "init", "-q", str(self.worktree)], check=True)
        self.lib = self.worktree / "crates/alpha/src/lib.rs"
        self.lib.parent.mkdir(parents=True)
        self.lib.write_text("\n")
        self.target = self.root / "target space"
        self.target.mkdir()
        self.bin = self.root / "bin"
        self.bin.mkdir()
        self.log = self.root / "events.jsonl"
        self.free = self.root / "free"
        self.free.write_text(str(40 * 1048576))
        self.count = self.root / "df-count"
        self.children = []
        self.env = {
            **os.environ,
            "PATH": f"{self.bin}:{os.environ['PATH']}",
            "PIJ_SHARED_TARGET": str(self.target),
            "PIJ_SHARED_BUILD_DF": str(self.bin / "df"),
            "PIJ_SHARED_BUILD_POLL_SECONDS": "1",
            "PIJ_DISK_FLOOR_GB": "25",
            "PIJ_ID": "pij-review-fixture",
            "REVIEW_LOG": str(self.log),
            "REVIEW_FREE": str(self.free),
            "REVIEW_COUNT": str(self.count),
        }
        self.exe("df", """
import os
from pathlib import Path
p = Path(os.environ["REVIEW_COUNT"])
n = int(p.read_text()) + 1 if p.exists() else 1
p.write_text(str(n))
free = Path(os.environ["REVIEW_FREE"]).read_text()
if os.environ.get("REVIEW_DROP_SECOND") and n >= 2:
    free = "1024"
print("Filesystem 1024-blocks Used Available Capacity Mounted on")
print("fake 100000000 0 " + free + " 0% /")
""")
        self.exe("worker", """
import json, os, signal, sys, time
if os.environ.get("REVIEW_RESIST_TERM"):
    signal.signal(signal.SIGTERM, signal.SIG_IGN)
def event(kind):
    with open(os.environ["REVIEW_LOG"], "a") as f:
        f.write(json.dumps({"kind": kind, "name": sys.argv[1], "pid": os.getpid(), "time": time.monotonic()}) + "\\n")
event("worker-start")
time.sleep(float(sys.argv[2]))
event("worker-end")
""")
        self.exe("cargo", """
import json, os, subprocess, sys, time
def event(kind):
    with open(os.environ["REVIEW_LOG"], "a") as f:
        f.write(json.dumps({"kind": kind, "name": sys.argv[1], "args": sys.argv[1:], "pid": os.getpid(), "time": time.monotonic(), "target": os.environ["CARGO_TARGET_DIR"], "incremental": os.environ["CARGO_INCREMENTAL"]}) + "\\n")
if os.environ.get("REVIEW_CHILD"):
    worker = subprocess.Popen([os.environ["REVIEW_WORKER"], sys.argv[1], os.environ.get("REVIEW_DELAY", "0")], stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    while True:
        if os.path.exists(os.environ["REVIEW_LOG"]):
            with open(os.environ["REVIEW_LOG"]) as f:
                if any(json.loads(line)["kind"] == "worker-start" for line in f):
                    break
        time.sleep(0.01)
    event("start")
    if not os.environ.get("REVIEW_ORPHAN"):
        worker.wait()
else:
    event("start")
    time.sleep(float(os.environ.get("REVIEW_DELAY", "0")))
event("end")
sys.exit(7 if sys.argv[1] == "fail" else 0)
""")
        self.env["REVIEW_WORKER"] = str(self.bin / "worker")

    def exe(self, name, body):
        path = self.bin / name
        path.write_text(f"#!{sys.executable}\n" + body)
        path.chmod(0o755)

    @property
    def lock(self):
        return Path(str(self.target) + ".lock")

    def start(self, args, extra=None, script=SCRIPT, recipe=False):
        command = ["/bin/bash", str(script), *args]
        if recipe:
            (self.worktree / "justfile").write_bytes((SOURCE / "justfile").read_bytes())
            dest = self.worktree / "harness/scripts"
            dest.mkdir(parents=True, exist_ok=True)
            (dest / "shared-build.sh").symlink_to(SCRIPT)
            command = ["just", "shared-build", *args]
        child = subprocess.Popen(command, cwd=self.worktree, env={**self.env, **(extra or {})}, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        self.children.append(child)
        return child

    def run(self, args, extra=None, recipe=False):
        p = self.start(args, extra, recipe=recipe)
        out, err = p.communicate(timeout=8)
        return p.returncode, out, err

    def events(self):
        return [json.loads(line) for line in self.log.read_text().splitlines()] if self.log.exists() else []

    def until(self, condition):
        deadline = time.monotonic() + 5
        while not condition():
            if time.monotonic() >= deadline:
                raise AssertionError("timed out waiting for fixture")
            time.sleep(0.01)

    def close(self):
        for child in self.children:
            if child.poll() is None:
                child.terminate()
                child.communicate(timeout=5)
        for event in self.events():
            if event["kind"] == "worker-start":
                try:
                    os.kill(event["pid"], signal.SIGKILL)
                except ProcessLookupError:
                    pass
        self.temp.cleanup()


def check(name, fn):
    f = Fixture()
    try:
        details = fn(f)
        RESULTS.append({"name": name, "status": "pass", "details": details})
    finally:
        f.close()


def basics(f):
    assert f.run(["--", "build"], {"PIJ_SHARED_TARGET": ""})[0] == 2
    assert f.run(["--", "build"], {"PIJ_SHARED_TARGET": "relative"})[0] == 2
    assert not f.events() and not f.lock.exists()
    os.utime(f.lib, (1000000, 1000000))
    args = ["build", "two words", "a'b", "$literal", "semi;colon", "", "(group)"]
    code, _, err = f.run(["--", *args, "--then", "test"], recipe=True)
    assert code == 0, err
    starts = [e for e in f.events() if e["kind"] == "start"]
    assert starts[0]["args"] == args
    assert starts[0]["target"] == str(f.target)
    assert starts[0]["incremental"] == "0"
    assert f.lib.stat().st_mtime > 1000000 and not f.lock.exists()
    assert f.run(["--", "fail", "--then", "never"])[0] == 7
    assert not any(e["name"] == "never" for e in f.events())
    assert not f.lock.exists()
    return {"recipe_args": starts[0]["args"], "source_refreshed": True, "failure": 7}


def floor(f):
    f.free.write_text(str(10 * 1048576))
    assert f.run(["--", "build"])[0] == 3
    assert not f.events() and not f.lock.exists()
    f.free.write_text(str(40 * 1048576))
    f.count.unlink()
    assert f.run(["--", "build"], {"REVIEW_DROP_SECOND": "1"})[0] == 3
    assert not f.events() and not f.lock.exists()
    return "no cargo before lock or after acquisition when the second floor check refuses"


def stale_timeout(f):
    f.lock.mkdir()
    owner = f.lock / "owner"
    owner.write_text(f"seat=review pid={os.getpid()} worktree=fixture started=now\n")
    assert f.run(["--timeout", "1s", "--", "build"])[0] == 4
    assert owner.exists()
    gone = subprocess.run(["/bin/sh", "-c", "echo $$"], capture_output=True, text=True, check=True).stdout.strip()
    owner.write_text(f"seat=review pid={gone} worktree=fixture started=then\n")
    code, _, err = f.run(["--", "build"])
    assert code == 5 and "clear it with:" in err and owner.exists()
    return "timeout preserves live lock; dead-owner refusal preserves stale lock"


def overlap(f, script=SCRIPT, alias=False):
    a = f.start(["--", "first"], {"REVIEW_DELAY": "1.5"}, script)
    f.until(lambda: any(e["kind"] == "start" for e in f.events()))
    extra = {}
    if alias:
        link = f.root / "alias"
        link.symlink_to(f.target, target_is_directory=True)
        extra["PIJ_SHARED_TARGET"] = str(link)
    b = f.start(["--", "second"], extra, script)
    _, aerr = a.communicate(timeout=8)
    _, berr = b.communicate(timeout=8)
    assert a.returncode == b.returncode == 0, (aerr, berr)
    order = [e["kind"] + " " + e["name"] for e in f.events()]
    return order


def serialization(f):
    order = overlap(f)
    assert order == ["start first", "end first", "start second", "end second"], order
    return order


def mutation(f):
    mutated = f.root / "nonexclusive.sh"
    text = SCRIPT.read_text().replace('if /bin/mkdir "$lock" 2>/dev/null; then', '/bin/mkdir "$lock" 2>/dev/null || :\n\tif :; then')
    assert text != SCRIPT.read_text()
    mutated.write_text(text)
    order = overlap(f, mutated)
    assert order != ["start first", "end first", "start second", "end second"], order
    return {"mutation_detected": True, "order": order}


def process_alive(pid):
    try:
        os.kill(pid, 0)
        return True
    except ProcessLookupError:
        return False


def signal_descendant(f, sig, script=SCRIPT):
    a = f.start(["--", "first"], {"REVIEW_CHILD": "1", "REVIEW_DELAY": "1.5"}, script)
    f.until(lambda: any(e["kind"] == "start" for e in f.events()))
    worker = next(e["pid"] for e in f.events() if e["kind"] == "worker-start")
    a.send_signal(sig)
    _, err = a.communicate(timeout=5)
    assert a.returncode == (143 if sig == signal.SIGTERM else 130), err
    assert not f.lock.exists()
    assert not process_alive(worker), "build worker survived cancellation and unlock"
    b = f.start(["--", "second"])
    b.communicate(timeout=5)
    time.sleep(1.6)
    events = f.events()
    assert not any(e["kind"] == "worker-end" for e in events)
    return {"defect_closed": True, "wrapper_exit": a.returncode, "worker_dead_before_next_batch": True, "order": [e["kind"] + " " + e["name"] for e in events]}


def alias(f, script=SCRIPT):
    order = overlap(f, script, alias=True)
    assert order == ["start first", "end first", "start second", "end second"], order
    assert all(e["target"] == str(f.target) for e in f.events())
    return {"defect_closed": True, "same_physical_target_one_lock": True, "order": order}


def path_boundaries(f):
    alias_path = f.root / "alias"
    alias_path.symlink_to(f.target, target_is_directory=True)
    missing = f.target / "not-yet" / "built"
    assert f.run(["--", "third"], {"PIJ_SHARED_TARGET": str(alias_path / "not-yet" / "built")})[0] == 0
    assert f.events()[-1]["target"] == str(missing)
    assert not Path(str(missing) + ".lock").exists()
    dangling = f.root / "dangling"
    dangling.symlink_to(f.root / "absent")
    assert f.run(["--", "no"], {"PIJ_SHARED_TARGET": str(dangling)})[0] == 2
    regular = f.root / "file"
    regular.write_text("not a directory")
    assert f.run(["--", "no"], {"PIJ_SHARED_TARGET": str(regular)})[0] == 2
    f.lock.write_text("a file cannot be a directory lock")
    code, _, err = f.run(["--", "no"])
    assert code == 2 and "could not create the lock" in err
    return "nonexistent target through symlink works; dangling target, non-directory target and blocked lock refuse"


def normal_orphan(f):
    a = f.start(["--", "first"], {"REVIEW_CHILD": "1", "REVIEW_ORPHAN": "1", "REVIEW_DELAY": "3"})
    a.communicate(timeout=5)
    assert a.returncode == 0
    worker = next(e["pid"] for e in f.events() if e["kind"] == "worker-start")
    assert not process_alive(worker) and not f.lock.exists()
    assert not any(e["kind"] == "worker-end" for e in f.events())
    return "successful cargo parent cannot leave a target-writing descendant after unlock"


def escalation(f):
    a = f.start(["--", "first"], {"REVIEW_CHILD": "1", "REVIEW_RESIST_TERM": "1", "REVIEW_DELAY": "30"})
    f.until(lambda: any(e["kind"] == "start" for e in f.events()))
    worker = next(e["pid"] for e in f.events() if e["kind"] == "worker-start")
    began = time.monotonic()
    a.send_signal(signal.SIGTERM)
    time.sleep(0.3)
    assert f.lock.exists() and process_alive(worker)
    b = f.start(["--", "second"])
    _, err = a.communicate(timeout=25)
    b.communicate(timeout=8)
    elapsed = time.monotonic() - began
    assert a.returncode == 143 and b.returncode == 0, err
    assert elapsed >= 10 and not process_alive(worker) and not f.lock.exists()
    assert not any(e["kind"] == "worker-end" for e in f.events())
    return {"term_resistant_worker_killed": True, "elapsed_seconds": round(elapsed, 2), "lock_held_during_grace": True}


def guard_mutation(f, which):
    mutated = f.root / "guard-mutant.sh"
    text = SCRIPT.read_text()
    if which == "process-group":
        start = text.index("drain() {")
        end = text.index("\non_signal()", start)
        text = text[:start] + 'drain() {\n\tkill -TERM "$1" 2>/dev/null\n}\n' + text[end:]
    else:
        text = text.replace('target=$(physical_target "$target") || exit $?', 'target="${target%/}"')
    assert text != SCRIPT.read_text()
    mutated.write_text(text)
    caught = False
    try:
        if which == "process-group":
            signal_descendant(f, signal.SIGTERM, mutated)
        else:
            alias(f, mutated)
    except AssertionError:
        caught = True
    assert caught, "mutated guard escaped its regression oracle"
    return {"mutation_detected": which}


if __name__ == "__main__":
    check("recipe-forwarding-env-refresh-failure", basics)
    check("disk-floor-before-lock-and-before-command", floor)
    check("timeout-and-stale-lock-preservation", stale_timeout)
    check("ordinary-serialization", serialization)
    check("nonexclusive-lock-mutation", mutation)
    check("SIGTERM-descendant-overlap", lambda f: signal_descendant(f, signal.SIGTERM))
    check("SIGINT-descendant-overlap", lambda f: signal_descendant(f, signal.SIGINT))
    check("symlink-target-overlap", alias)
    check("path-resolution-and-lock-errors", path_boundaries)
    check("normal-exit-descendant-drain", normal_orphan)
    check("TERM-resistant-descendant-KILL-escalation", escalation)
    check("process-group-regression-mutation", lambda f: guard_mutation(f, "process-group"))
    check("canonical-target-regression-mutation", lambda f: guard_mutation(f, "canonical-target"))
    print(json.dumps(RESULTS, indent=2))
