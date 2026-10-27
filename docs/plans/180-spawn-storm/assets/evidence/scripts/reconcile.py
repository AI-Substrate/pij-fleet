import ctypes, ctypes.util, json, subprocess, time, os, sys, collections
lib = ctypes.CDLL(ctypes.util.find_library("proc") or "/usr/lib/libproc.dylib")
libc = ctypes.CDLL(ctypes.util.find_library("c"))
buf = (ctypes.c_int * 65536)()
def pids():
    n = lib.proc_listallpids(buf, ctypes.sizeof(buf))
    return set(buf[:n])
def kinfo(pid):
    mib = (ctypes.c_int * 4)(1, 14, 1, pid)  # CTL_KERN, KERN_PROC, KERN_PROC_PID
    b = ctypes.create_string_buffer(648); size = ctypes.c_size_t(648)
    if libc.sysctl(mib, 4, b, ctypes.byref(size), None, 0) != 0 or size.value == 0:
        return None
    raw = b.raw
    ppid = int.from_bytes(raw[560:564], "little")
    comm = raw[243:243+17].split(b"\0")[0].decode(errors="replace")
    return ppid, comm
daemon = int(sys.argv[1]); secs = float(sys.argv[2])
def counter():
    return json.loads(subprocess.check_output(["pij-rs","--json","ping"]))["data"]["spawns"]
c1 = counter(); l1 = os.getloadavg()[0]
seen = pids(); t0 = time.time(); kids = {}; polls = 0
while time.time() - t0 < secs:
    cur = pids(); polls += 1
    for pid in cur - seen:
        k = kinfo(pid)
        if k and k[0] == daemon:
            kids[pid] = k[1]
    seen |= cur
c2 = counter(); dt = time.time() - t0; l2 = os.getloadavg()[0]
by = collections.Counter(kids.values())
delta = {op: c2["by_op"].get(op,0) - c1["by_op"].get(op,0) for op in c2["by_op"]}
delta = {k:v for k,v in delta.items() if v}
print(json.dumps({"dt": round(dt,1), "load1": [round(l1,2), round(l2,2)], "polls": polls, "poll_hz": round(polls/dt,1),
  "counter_launches": c2["total"]-c1["total"], "counter_by_op": delta,
  "kinfo_unique_children_seen": len(kids), "kinfo_by_comm": dict(by)}, indent=1))
