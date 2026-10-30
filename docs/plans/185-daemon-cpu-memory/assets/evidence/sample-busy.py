import re,sys,collections
WAIT={'__psynch_cvwait','semaphore_wait_trap','kevent','__recvfrom','semaphore_timedwait_trap','__semwait_signal','__psynch_mutexwait','read','__select','mach_msg2_trap','__workq_kernreturn','__ulock_wait2'}
lines=open(sys.argv[1]).read().split('\n')
start=next(i for i,l in enumerate(lines) if l.startswith('Call graph:'))
end=next(i for i,l in enumerate(lines) if l.startswith('Total number in stack'))
pat=re.compile(r'^(\s*[+!:| ]*)(\d+) (.+?)  \(in ([^)]+)\)')
tpat=re.compile(r'^\s+(\d+) (Thread_\S+.*)$')
nodes=[]  # (depth,count,sym)
stack=[]
incl=collections.Counter(); total_busy=0; leafc=collections.Counter()
def demangle(s):
    s=re.sub(r'\s*\+ \d.*','',s)
    return s
rows=[]
for l in lines[start+1:end]:
    m=pat.match(l)
    if m:
        rows.append((len(m.group(1)),int(m.group(2)),demangle(m.group(3)),m.group(4)))
    else:
        t=tpat.match(l)
        if t: rows.append((0,int(t.group(1)),'THREAD '+t.group(2).split(':')[-1].strip(),'' ))
# compute self counts
for i,(d,c,s,lib) in enumerate(rows):
    ch=0;j=i+1
    while j<len(rows) and rows[j][0]>d:
        if all(rows[k][0]>rows[j][0] or k<=i for k in [] ) : pass
        j+=1
    # children = rows in (i,j) with depth == min depth among them directly
    k=i+1; 
    while k<j:
        cd=rows[k][0]; ch+=rows[k][1]
        k2=k+1
        while k2<j and rows[k2][0]>cd: k2+=1
        k=k2
    selfc=c-ch
    if selfc>0:
        rows[i]=(d,c,s,lib,selfc)
    else: rows[i]=(d,c,s,lib,0)
# walk with stack
st=[]
for (d,c,s,lib,selfc) in rows:
    while st and st[-1][0]>=d: st.pop()
    st.append((d,s))
    if selfc and s not in WAIT:
        total_busy+=selfc; leafc[s]+=selfc
        seen=set()
        for _,a in st:
            if a in seen: continue
            seen.add(a); incl[a]+=selfc
print('busy samples',total_busy)
key=sys.argv[2] if len(sys.argv)>2 else 'pij'
for s,c in incl.most_common(400):
    if re.search(key,s): print(c, s[:230])
