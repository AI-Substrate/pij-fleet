#!/bin/zsh
# usage: canary.sh <label> <cwd> <wait_seconds> [extra spawn args...]
set -u
S=/private/tmp/claude-501/-Users-jordanknight-pi-hacking-fleet-181-codex-bind/726e715b-0991-4322-ab03-b9dd3ab9972f/scratchpad
L=$1; CWD=$2; W=$3; shift 3
D=$S/ev/$L; mkdir -p $D
echo "$(date -u +%FT%TZ) start" > $D/timeline.txt
( pij-rs tail --json > $D/tail.jsonl 2>&1 ) & TAILPID=$!
CMD=(pij-rs spawn --harness codex --model gpt-6.1-sol --effort low --role worker --name "c181-$L" --cwd "$CWD" --wait-seconds $W --json "$@")
print -r -- "${CMD[@]}" > $D/spawn-cmd.txt
( "${CMD[@]}" > $D/spawn-out.json 2>&1; echo "exit=$?" >> $D/spawn-out.json; echo "$(date -u +%FT%TZ) spawn-returned" >> $D/timeline.txt ) & SPID=$!
PANE=""
for i in {1..40}; do
  PANE=$(tmux list-windows -a -F '#{window_name} #{pane_id}' | awk -v n="c181-$L" '$1==n{print $2}' | head -1)
  [[ -n $PANE ]] && break; sleep 0.25
done
echo "pane=$PANE" >> $D/timeline.txt
t=0
while kill -0 $SPID 2>/dev/null; do
  tmux capture-pane -p -t "$PANE" -S -200 > $D/pane-t$(printf %03d $t).txt 2>&1
  sleep 5; t=$((t+5))
done
tmux capture-pane -p -t "$PANE" -S -200 > $D/pane-at-return.txt 2>&1
sleep 20
tmux capture-pane -p -t "$PANE" -S -200 > $D/pane-settled.txt 2>&1
kill $TAILPID 2>/dev/null
echo "$(date -u +%FT%TZ) done" >> $D/timeline.txt
