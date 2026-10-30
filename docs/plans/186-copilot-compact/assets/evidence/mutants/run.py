import subprocess, shutil, sys, os, re
ROOT=os.getcwd()  # run from the repo root
OUT=sys.argv[2]
M={
 'M1-copilot-compact-refused': ('crates/core/src/control.rs',
   'if target.harness == Harness::Copilot && !COPILOT_COMMANDS.contains(&command) {',
   'if target.harness == Harness::Copilot {',
   [['-p','pij-core','--lib','control::'],['-p','pij-daemon','--test','control_commands','--test','copilot_compact']]),
 'M2-copilot-new-allowed': ('crates/core/src/control.rs',
   'if target.harness == Harness::Copilot && !COPILOT_COMMANDS.contains(&command) {',
   'if target.harness == Harness::Copilot && !["compact", "new"].contains(&command) {',
   [['-p','pij-core','--lib','control::'],['-p','pij-daemon','--test','control_commands']]),
 'M3-drain-claims-copilot-bodies': ('crates/daemon/src/pointer/worker.rs',
   '                Harness::Copilot => control_kind(&seat.id),\n',
   '',
   [['-p','pij-daemon','--lib','copilot_drain_never_claims'],['-p','pij-daemon','--test','copilot_compact']]),
 'M4-typed-over-draft': ('crates/harnesses/src/interaction.rs',
   '        if !verdict.permitted || !verdict.composer_idle {\n            transaction.abort().await?;',
   '        if false && (!verdict.permitted || !verdict.composer_idle) {\n            transaction.abort().await?;',
   [['-p','pij-daemon','--test','copilot_compact']]),
 'M5-no-busy-brake': ('crates/daemon/src/pointer/worker.rs',
   '            && recipient.state == SystemState::Working\n',
   '            && recipient.state == SystemState::Working\n            && false\n',
   [['-p','pij-daemon','--test','copilot_compact']]),
 'M6-control-blocks-native-parking': ('crates/store/src/queue.rs',
   "                     AND earlier.kind != 'delivery-control:' || jobs.serial_key \\\n",
   '',
   [['-p','pij-store','--test','queue_copilot_control']]),
}
name=sys.argv[1]
path,old,new,runs=M[name]
p=os.path.join(ROOT,path); bak=p+'.mutbak'
shutil.copy(p,bak)
try:
    s=open(p).read(); assert s.count(old)==1, (name, s.count(old)); open(p,'w').write(s.replace(old,new))
    os.utime(p, None)
    with open(OUT,'w') as f:
        f.write(f'# {name}\n# file: {path}\n# - {old.strip()}\n# + {new.strip() or "(deleted)"}\n')
        for r in runs:
            cmd=['timeout','-k','10','1800','cargo','test']+r+['--no-fail-fast']
            res=subprocess.run(cmd,cwd=ROOT,capture_output=True,text=True)
            f.write(f'\n$ {" ".join(cmd[4:])}\nexit={res.returncode}\n')
            for line in (res.stdout+res.stderr).splitlines():
                if re.match(r'^(test .*(FAILED|ok)$|test result|error\[|error:)',line) and 'filtered out; finished in 0.00s' not in line or 'panicked at' in line:
                    f.write(line+'\n')
finally:
    shutil.move(bak,p)
    # The backup's mtime predates the mutated build; without a fresh mtime cargo
    # keeps the MUTATED artifact for the next run (found 2026-10-30, plan 186).
    os.utime(p, None)
