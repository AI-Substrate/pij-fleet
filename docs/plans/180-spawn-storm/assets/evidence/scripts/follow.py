import ctypes, ctypes.util, json, time, sys, collections
lib = ctypes.CDLL(ctypes.util.find_library("proc") or "/usr/lib/libproc.dylib")
libc = ctypes.CDLL(ctypes.util.find_library("c"))
buf = (ctypes.c_int * 65536)()
def pids():
    n = lib.proc_listallpids(buf, ctypes.sizeof(buf)); return set(buf[:n])
def kinfo(pid):
    mib = (ctypes.c_int * 4)(1, 14, 1, pid)
    b = ctypes.create_string_buffer(648); size = ctypes.c_size_t(648)
    if libc.sysctl(mib, 4, b, ctypes.byref(size), None, 0) != 0 or size.value == 0: return None
    r = b.raw
    return dict(ppid=int.from_bytes(r[560:564],"little"), comm=r[243:260].split(b"\0")[0].decode(errors="replace"),
                stat=r[36], xstat=int.from_bytes(r[280:282],"little"), flag=int.from_bytes(r[32:36],"little"))
def args(pid):
    mib=(ctypes.c_int*3)(1,49,pid); size=ctypes.c_size_t(65536); b=ctypes.create_string_buffer(65536)
    if libc.sysctl(mib,3,b,ctypes.byref(size),None,0)!=0: return None
    raw=b.raw[4:size.value]; parts=[p.decode(errors="replace") for p in raw.split(b"\0") if p]
    return parts[:6]
daemon=int(sys.argv[1]); secs=float(sys.argv[2])
seen=pids(); t0=time.time(); watch={}; final=collections.Counter(); samples=[]
STAT={1:"SIDL",2:"SRUN",3:"SSLEEP",4:"SSTOP",5:"SZOMB"}
while time.time()-t0<secs:
    cur=pids()
    for pid in cur-seen:
        k=kinfo(pid)
        if k and k["ppid"]==daemon and k["comm"].startswith("pij-rs"):
            watch[pid]=[k]
            a=args(pid)
            if len(samples)<8: samples.append((pid,STAT.get(k["stat"],k["stat"]),k["flag"],a))
    for pid,hist in list(watch.items()):
        k=kinfo(pid)
        if k is None:
            final[("gone", hist[-1]["comm"], STAT.get(hist[-1]["stat"],hist[-1]["stat"]))]+=1; del watch[pid]
        elif k["comm"]!=hist[-1]["comm"] or k["stat"]!=hist[-1]["stat"]:
            hist.append(k)
            if not k["comm"].startswith("pij-rs"):
                final[("became", k["comm"])]+=1; del watch[pid]
    seen|=cur
print("first-sight samples (pid, stat, p_flag, argv):")
for s in samples: print(" ",s)
print("outcomes:", {" ".join(map(str,k)):v for k,v in final.most_common()})
