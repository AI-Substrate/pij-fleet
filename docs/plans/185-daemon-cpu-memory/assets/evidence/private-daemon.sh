#!/bin/bash
# Plan 185 recorded-workload replay on a PRIVATE daemon (R2):
# temp HOME + state dir, private -L tmux server, private port. Never the machine daemon.
# usage: private-daemon.sh <label> <pij-rs binary> <port> <rows>
set -u
LABEL=$1; BIN=$2; PORT=$3; ROWS=$4
S=${SCRATCH:?set SCRATCH}
RUN=$S/run-$LABEL; rm -rf "$RUN"; mkdir -p "$RUN/home" "$RUN/state"
SERVER="pij-185-$LABEL-$$"
OUT=$S/receipts/private-$LABEL.txt; : > "$OUT"
export HOME="$RUN/home" PIJ_RS_STATE_DIR="$RUN/state" PIJ_TMUX_SERVER="$SERVER" PIJ_RS_ADDR="127.0.0.1:$PORT"
tmux -L "$SERVER" -f /dev/null new-session -d -s bench -n w0 'sleep 100000'
start() { "$BIN" daemon --bind "127.0.0.1:$PORT" --state-dir "$RUN/state" >>"$RUN/daemon.log" 2>&1 & echo $! > "$RUN/pid"; for i in $(seq 1 100); do curl -s "http://127.0.0.1:$PORT/health" >/dev/null 2>&1 && [ -s "$RUN/state/daemon.key" ] && break; sleep 0.2; done; }
stop() { kill -TERM "$(cat "$RUN/pid")" 2>/dev/null; for i in $(seq 1 100); do kill -0 "$(cat "$RUN/pid")" 2>/dev/null || break; sleep 0.2; done; }
start; stop
DB="$RUN/state/pij.sqlite"
PAYLOAD=$(printf '{"job_id":"bg-bench","pad":"%0320d"}' 0)
sqlite3 "$DB" <<SQL
INSERT INTO seats (id, harness, folder, state, tombstoned_at, tombstone_reason, seq) VALUES ('pij-bench-owner','claude','/tmp','idle',1,'bench',1);
INSERT INTO background_jobs (job_id, owner, title, command, out_path, state, started_at, finished_at, kind, min_interval_ms) VALUES ('bg-bench-stranded','pij-bench-owner','bench','watch','$RUN/bg.log','lost',1,2,'events',0);
WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x < $ROWS)
INSERT INTO spine_events (v, at, kind, seat, payload) SELECT 1, x, 'bg.lost', 'pij-bench', '$PAYLOAD' FROM n;
SQL
touch "$RUN/bg.log"
start; PID=$(cat "$RUN/pid")
KEY=$(cat "$RUN/state/daemon.key")
ALIAS=$(curl -s -H "Authorization: Bearer $KEY" "http://127.0.0.1:$PORT/health" | jq -r '.data.machine')
echo "label=$LABEL pid=$PID alias=<redacted> rows=$ROWS" >> "$OUT"
seq0=$(sqlite3 -readonly "$DB" "select max(seq) from spine_events")
# chainglass-like client: attach with a cursor 10 behind, read 1 s, close; every 3.7 s.
( while kill -0 $PID 2>/dev/null; do
    cur=$(( $(sqlite3 -readonly "$DB" "select max(seq) from spine_events") - 10 ))
    since=$(printf '{"%s":%d}' "$ALIAS" "$cur" | jq -sRr @uri)
    curl -s -m 1 -H "Authorization: Bearer $KEY" "http://127.0.0.1:$PORT/v1/events?since=$since" >/dev/null 2>&1
    sleep 2.7
  done ) &
CLIENT=$!
t0=$(date +%s)
for mark in 15 60; do
  while [ $(( $(date +%s) - t0 )) -lt $mark ]; do sleep 1; done
  echo "+${mark}s rss_kb=$(ps -o rss= -p $PID | tr -d ' ')" >> "$OUT"
done
cpu0=$(ps -o time= -p $PID); sleep 60; cpu1=$(ps -o time= -p $PID)
echo "cpu_time_before=$cpu0 cpu_time_after_60s=$cpu1" >> "$OUT"
echo "+120s rss_kb=$(ps -o rss= -p $PID | tr -d ' ')" >> "$OUT"
seq1=$(sqlite3 -readonly "$DB" "select max(seq) from spine_events")
echo "spine_growth_over_run=$((seq1-seq0)) rows in $(( $(date +%s) - t0 ))s" >> "$OUT"
echo "bg_lost_rows_added=$(sqlite3 -readonly "$DB" "select count(*) from spine_events where seq>$seq0 and kind='bg.lost'") final_turn_dropped=$(sqlite3 -readonly "$DB" "select count(*) from spine_events where kind='bg.final-turn-dropped'")" >> "$OUT"
echo "daemon_log_lines=$(wc -l < "$RUN/daemon.log") reconciliation_lines=$(grep -c 'background reconciliation failed' "$RUN/daemon.log")" >> "$OUT"
curl -s -H "Authorization: Bearer $KEY" "http://127.0.0.1:$PORT/health" | jq -c '.data.spawns.total' | sed 's/^/spawns_total=/' >> "$OUT"
kill $CLIENT 2>/dev/null; stop
tmux -L "$SERVER" kill-server 2>/dev/null
cp "$RUN/daemon.log" "$S/receipts/private-$LABEL-daemon.log"
rm -rf "$RUN"
cat "$OUT"
