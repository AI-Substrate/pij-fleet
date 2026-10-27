import ctypes, ctypes.util, os, sys, threading, time, json
lib=ctypes.CDLL(ctypes.util.find_library("proc") or "/usr/lib/libproc.dylib"); libc=ctypes.CDLL(ctypes.util.find_library("c"))
buf=(ctypes.c_int*65536)()
def pids():
    n=lib.proc_listallpids(buf,ctypes.sizeof(buf)); return set(buf[:n])
def kinfo(pid):
    mib=(ctypes.c_int*4)(1,14,1,pid); b=ctypes.create_string_buffer(648); size=ctypes.c_size_t(648)
    if libc.sysctl(mib,4,b,ctypes.byref(size),None,0)!=0 or size.value==0: return None
    r=b.raw; return int.from_bytes(r[560:564],"little"), r[243:260].split(b"\0")[0].decode(errors="replace")
# daemon PATH
mib=(ctypes.c_int*3)(1,49,int(sys.argv[1])); size=ctypes.c_size_t(262144); b=ctypes.create_string_buffer(262144)
libc.sysctl(mib,3,b,ctypes.byref(size),None,0); raw=b.raw[:size.value]; argc=int.from_bytes(raw[:4],"little")
env=[p.decode(errors="replace") for p in [p for p in raw[4:].split(b"\0") if p][argc+1:]]
PATH=next(e[5:] for e in env if e.startswith("PATH="))
def run(prog, n):
    me=os.getpid(); seen=pids(); kids={}; stop=threading.Event()
    def sampler():
        nonlocal seen
        while not stop.is_set():
            cur=pids()
            for p in cur-seen:
                k=kinfo(p)
                if k and k[0]==me: kids[p]=k[1]
            seen|=cur
    t=threading.Thread(target=sampler); t.start(); time.sleep(0.2)
    def pidctr():
        p=os.posix_spawn("/usr/bin/true",["true"],os.environ); os.waitpid(p,0); return p
    a=pidctr()
    for _ in range(n):
        p=os.posix_spawnp(prog,[prog,"-o","lstart=","-p","1"],{"PATH":PATH,"LC_ALL":"C"}) if "/" not in prog else os.posix_spawn(prog,[prog,"-o","lstart=","-p","1"],{"PATH":PATH,"LC_ALL":"C"})
        os.waitpid(p,0)
    z=pidctr(); time.sleep(0.3); stop.set(); t.join()
    from collections import Counter
    return {"prog":prog,"spawns":n,"pids_consumed":z-a-1,"pids_per_spawn":round((z-a-1)/n,2),"sampler_saw":dict(Counter(kids.values()))}
print(json.dumps(run("ps",100))); print(json.dumps(run("/bin/ps",100)))
