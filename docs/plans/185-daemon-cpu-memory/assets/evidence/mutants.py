# Plan 185 mutants: each patches ONE fix out (or breaks it), runs the test that
# claims to defend it, and expects RED. Restores the file afterwards.
import subprocess, sys, shutil
ROOT=subprocess.run(['git','rev-parse','--show-toplevel'],capture_output=True,text=True).stdout.strip()
M=[
 ('M1 SqliteSpine::newest answers the oldest','crates/store/src/spine.rs',
  'SELECT COALESCE(MAX(seq), 0) FROM spine_events','SELECT COALESCE(MIN(seq), 0) FROM spine_events',
  ['-p','pij-store','--test','store_integration','--','the_sqlite_spine_honours_the_same_contract']),
 ('M2 subscribe scans the whole spine again','crates/daemon/src/events/mod.rs',
  'let newest = self.spine.newest().await?;',
  'let newest = self.spine.tail(None, Seq(0)).await?.last().and_then(|event| event.seq).unwrap_or(Seq(0));',
  ['-p','pij-daemon','--test','events_bus','--','a_cursor_attach']),
 ('M5 completion guard keyed by kind only','crates/daemon/src/background.rs',
  '.latest_matching_message(&job.owner, kind, &msg_id)',
  '.latest_matching(&job.owner, &[kind])',
  ['-p','pij-daemon','--lib','--','background::tests::every_new_job']),
 ('M6 failure log logs every repeat','crates/daemon/src/lifecycle.rs',
  'Some(text) if last.as_deref() == Some(text.as_str()) => None,',
  'Some(text) if last.as_deref() == Some(text.as_str()) => Some(text),',
  ['-p','pij-daemon','--lib','--','lifecycle::tests::a_repeated']),
 ('M7 sidecar re-reads terminal records','crates/sidecars/src/background.rs',
  '                if !scan.terminal.contains(id) {','                if true || !scan.terminal.contains(id) {',
  ['-p','pij-sidecars','--test','sidecars','--','bg_worker_never']),
 ('M8 sidecar lists the directory every pass','crates/sidecars/src/background.rs',
  '        if scan.dir_mtime != Some(mtime) {','        if true || scan.dir_mtime != Some(mtime) {',
  ['-p','pij-sidecars','--test','sidecars','--','bg_worker_never']),
 ('M9 drain opens an unchanged tap','crates/tmux/src/lib.rs',
  '        if length == tap.offset {\n','        if length == tap.offset && File::open(&tap.path).is_ok() {\n',
  ['-p','pij-tmux','--test','adapter','--','an_unchanged_tap']),
 ('M10 live tap never trimmed','crates/tmux/src/lib.rs',
  '            if length >= TAP_TRIM_BYTES\n','            if length >= TAP_TRIM_BYTES * 1000\n',
  ['-p','pij-tmux','--test','adapter','--','a_consumed_tap_is_trimmed']),
 ('M11 every adapter claims pre-ledger sinks','crates/tmux/src/lib.rs',
  'if self.claims_legacy_sinks && !ledger.legacy_claimed() {','if !ledger.legacy_claimed() {',
  ['-p','pij-tmux','--test','adapter','--','a_named_server_adapter']),
 ('M12 sweep ignores live panes','crates/tmux/src/lib.rs',
  'if live.contains(&name) || !is_sink_name(&name) {','if !is_sink_name(&name) {',
  ['-p','pij-tmux','--test','adapter','--','the_claiming_sweep']),
 ('M13 sweep without the mtime brake','crates/tmux/src/lib.rs',
  '.is_none_or(|age| age < ORPHAN_TAP_GRACE);','.is_none_or(|age| age < Duration::ZERO);',
  ['-p','pij-tmux','--test','adapter','--','the_claiming_sweep']),
 ('M14 claim ignores other servers\' ledgers','crates/tmux/src/lib.rs',
  'if !others.contains(&name) && owned.insert(name) {','if owned.insert(name) {',
  ['-p','pij-tmux','--test','adapter','--','the_claiming_sweep']),
 ('M15 observer sweeps every tick','crates/daemon/src/pane_observer.rs',
  '        if !self.orphans_swept.load(Ordering::Acquire) {','        if true {',
  ['-p','pij-daemon','--lib','--','pane_observer::tests::the_orphan']),
 ('M16 reconcile reads the whole spine','crates/daemon/src/http/governance.rs',
  'for event in self.seat_kind_events(seat, "delivery.outcome").await? {',
  'for event in self.event_bus.tail(None, Seq(0)).await?.into_iter().filter(|event| event.kind == "delivery.outcome" && event.seat.as_ref() == Some(seat)) {',
  ['-p','pij-daemon','--lib','--','http::governance::tests::reconciliation_projects']),
 ('M17 published-check reads the seat history','crates/daemon/src/http/governance.rs',
  'let history = self.seat_kind_events(&dispatch.to, "dispatch").await?;',
  'let history: Vec<Event> = self.event_bus.tail(Some(&dispatch.to), Seq(0)).await?.into_iter().filter(|event| event.kind == "dispatch").collect();',
  ['-p','pij-daemon','--lib','--','http::governance::tests::reconciliation_projects']),
 ('M18 sweep forgets the tracked-tap veto','crates/tmux/src/lib.rs',
  '                        .keys()\n','                        .keys()\n                        .take(0)\n',
  ['-p','pij-tmux','--test','adapter','--','the_sweep_spares']),
 ('M19 sweep flag rides world_reconciled again','crates/daemon/src/pane_observer.rs',
  '            if swept.is_ok() {','            if swept.is_ok() && !reconcile_world {',
  ['-p','pij-daemon','--lib','--','pane_observer::tests::a_failing_pane']),
 ('M20 sweep always rewrites the ledger','crates/tmux/src/lib.rs',
  '        if owned.len() != listed {','        if owned.len() != listed || true {',
  ['-p','pij-tmux','--test','adapter','--','the_claiming_sweep']),
 ('M21 a waiting source re-publishes every tick','crates/daemon/src/background.rs',
  '.latest_matching_message(&job.owner, kind, &msg_id)\n            .await?\n            .is_none()',
  '.latest_matching_message(&job.owner, kind, &msg_id)\n            .await?\n            .is_none() || true',
  ['-p','pij-daemon','--lib','--','background::tests::a_revived']),
]
only=sys.argv[1:]
for name,path,old,new,args in M:
    if only and not any(name.startswith(o) for o in only): continue
    full=f'{ROOT}/{path}'; src=open(full).read()
    if src.count(old)!=1:
        print(f'{name}: PATCH DID NOT APPLY ({src.count(old)} matches)'); continue
    shutil.copy(full, full+'.mutbak')
    try:
        open(full,'w').write(src.replace(old,new))
        r=subprocess.run(['timeout','-k','10','1500','cargo','test','-q']+args,cwd=ROOT,capture_output=True,text=True)
        out=r.stdout+r.stderr
        lines=[l for l in out.splitlines() if 'test result' in l or 'panicked at' in l or l.startswith('error')]
        verdict='KILLED (red)' if r.returncode!=0 and 'error[' not in out else ('COMPILE-ERROR' if 'error[' in out else 'SURVIVED (green)')
        print(f'{name}: {verdict}')
        for l in lines[:3]: print('   ',l.strip()[:200])
    finally:
        shutil.move(full+'.mutbak', full)
        # The backup's mtime predates the mutant build, so cargo would keep the
        # MUTANT compiled. Bump it so the next build sees the restored source.
        import os; os.utime(full, None)
