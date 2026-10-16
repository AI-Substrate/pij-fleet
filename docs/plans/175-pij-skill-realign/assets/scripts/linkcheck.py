import re,os,sys,unicodedata
root=sys.argv[1]
def slug(h):
    h=h.strip().lower()
    h=re.sub(r"[^\w\- ]","",h)  # drop punctuation, keep word chars, hyphen, space
    return h.replace(" ","-")
def anchors(path):
    out=set(); counts={}
    for line in open(path,encoding="utf8"):
        m=re.match(r"^#{1,6}\s+(.*?)\s*#*\s*$",line)
        if m:
            s=slug(re.sub(r"`","",m.group(1)))
            n=counts.get(s,0); counts[s]=n+1
            out.add(s if n==0 else f"{s}-{n}")
        for a in re.findall(r'<a id="([^"]+)"',line): out.add(a)
    return out
bad=0; total=0
for dp,_,fs in os.walk(root):
    for f in fs:
        if not f.endswith(".md"): continue
        p=os.path.join(dp,f); text=open(p,encoding="utf8").read()
        text_nocode=re.sub(r"```.*?```","",text,flags=re.S)
        for m in re.finditer(r"\]\(([^)\s]+)\)",text_nocode):
            t=m.group(1)
            if re.match(r"^[a-z]+:",t) or t.startswith("<"): continue
            total+=1
            file,_,frag=t.partition("#")
            target=os.path.normpath(os.path.join(dp,file)) if file else p
            if not os.path.exists(target):
                print(f"MISSING {p}: {t}"); bad+=1; continue
            if frag and target.endswith(".md"):
                if frag not in anchors(target):
                    print(f"ANCHOR {p}: {t}"); bad+=1
print(f"links checked: {total}; broken: {bad}")
sys.exit(1 if bad else 0)
