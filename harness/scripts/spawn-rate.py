#!/usr/bin/env python3
"""Measure the machine's process-spawn rate and attribute it (macOS).

Why: Defender's EDR inspects every process launch, so the launch rate is the
cost. The pij daemon and the pij Copilot status line were the largest pij
sources (pij-fleet#62, plan 180). This probe is the before/after proof.

Two numbers:
- total: new PIDs per second, from the PID counter. A probe child is spawned
  at each window edge, and the difference of their PIDs is the launches in
  between. Exact apart from PID reuse skips; handles one wrap per window.
- attributed: a libproc poll of the PID table, classifying each new process by
  its parent: the pij-rs daemon (found by its listening port), the pij Copilot
  status line (a parent whose argv names copilot-statusline-pij), or other.
  Sub-millisecond processes exit before the poll sees them, so attributed
  counts are a LOWER BOUND. Compare them only against the same probe.

Usage: spawn-rate.py [--seconds N] [--port 7461] [--json]
"""

import argparse
import collections
import ctypes
import json
import subprocess
import sys
import time

lib = ctypes.CDLL("/usr/lib/libproc.dylib")
libc = ctypes.CDLL("/usr/lib/libc.dylib")
PID_MAX = 99999
PID_WRAP_TO = 100
_buf = (ctypes.c_int * 20000)()


def pids():
    n = lib.proc_listallpids(_buf, ctypes.sizeof(_buf))
    return set(_buf[:n])


class BSDInfo(ctypes.Structure):
    _fields_ = [
        ("flags", ctypes.c_uint32), ("status", ctypes.c_uint32), ("xstatus", ctypes.c_uint32),
        ("pid", ctypes.c_uint32), ("ppid", ctypes.c_uint32), ("ids", ctypes.c_uint32 * 7),
        ("comm", ctypes.c_char * 16), ("name", ctypes.c_char * 32), ("rest", ctypes.c_char * 64),
    ]


def info(pid):
    b = BSDInfo()
    if lib.proc_pidinfo(pid, 3, 0, ctypes.byref(b), ctypes.sizeof(b)) <= 0:
        return None
    return b.ppid, b.comm.decode(errors="replace")


def argv(pid):
    """argv via KERN_PROCARGS2; None once the process is gone."""
    mib = (ctypes.c_int * 3)(1, 49, pid)
    size = ctypes.c_size_t(65536)
    buf = ctypes.create_string_buffer(size.value)
    if libc.sysctl(mib, 3, buf, ctypes.byref(size), None, 0) != 0:
        return None
    raw = buf.raw[: size.value]
    argc = int.from_bytes(raw[:4], "little")
    rest = raw[4:].split(b"\0")
    # exec path, padding NULs, then argv
    words = [w for w in rest if w]
    return [w.decode(errors="replace") for w in words[1 : 1 + argc]]


def _last_child_pid():
    p = subprocess.Popen(["/usr/bin/true"])
    p.wait()
    return p.pid


def pid_delta(a, b):
    return b - a if b >= a else (PID_MAX - a) + (b - PID_WRAP_TO)


def daemon_pid(port):
    out = subprocess.run(
        ["lsof", "-nP", f"-iTCP:{port}", "-sTCP:LISTEN", "-t"], capture_output=True, text=True
    ).stdout.split()
    return int(out[0]) if out else None


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--seconds", type=float, default=20)
    ap.add_argument("--port", type=int, default=7461)
    ap.add_argument("--json", action="store_true")
    args = ap.parse_args()

    daemon = daemon_pid(args.port)
    parent_kind = {}

    def classify_parent(ppid):
        if ppid == daemon:
            return "pij-rs daemon"
        if ppid not in parent_kind:
            words = argv(ppid) or []
            text = " ".join(words)
            parent_kind[ppid] = (
                "pij status line" if "copilot-statusline-pij" in text else None
            )
        return parent_kind[ppid]

    seen = pids()
    start_pid = _last_child_pid()
    t0 = time.time()
    polls = 0
    sampled = 0
    by_class = collections.Counter()
    by_comm = collections.Counter()
    daemon_children = collections.Counter()
    statusline_runs = 0
    while time.time() - t0 < args.seconds:
        cur = pids()
        polls += 1
        for pid in cur - seen:
            sampled += 1
            got = info(pid)
            if got is None:
                by_class["<gone before seen>"] += 1
                continue
            ppid, comm = got
            by_comm[comm] += 1
            words = argv(pid) or []
            if any("copilot-statusline-pij" in w for w in words):
                statusline_runs += 1
            kind = classify_parent(ppid)
            if kind == "pij-rs daemon":
                sub = " ".join(words[1:3]) if words and words[0].endswith("tmux") else ""
                daemon_children[(comm + " " + sub).strip()] += 1
            by_class[kind or "other"] += 1
        seen = cur
    elapsed = time.time() - t0
    end_pid = _last_child_pid()
    total = pid_delta(start_pid, end_pid)

    rate = lambda n: round(n / elapsed, 1)
    report = {
        "seconds": round(elapsed, 1),
        "daemon_pid": daemon,
        "total_new_per_s": rate(total),
        "polls": polls,
        "sampled_per_s": rate(sampled),
        "attributed_per_s_lower_bound": {k: rate(v) for k, v in by_class.most_common()},
        "daemon_children_per_s_lower_bound": {k: rate(v) for k, v in daemon_children.most_common(10)},
        "statusline_runs_seen_per_s": rate(statusline_runs),
        "top_commands_per_s_lower_bound": {k: rate(v) for k, v in by_comm.most_common(12)},
    }
    if args.json:
        print(json.dumps(report, indent=2))
    else:
        for key, value in report.items():
            print(f"{key}: {value}")


if __name__ == "__main__":
    sys.exit(main())
