#!/bin/bash
# Plan 187 private replay (R2, R3): temp HOME + state dir, private -L tmux server,
# private port. Never the machine daemon, never ~/.pij-rs (read-only seed source).
#
#   private-replay.sh seed <pij-rs binary>
#     One seed, reused: a fresh store whose spine_events copies the LIVE spine's
#     seq, v, at, kind and seat (read-only ATTACH), with a synthetic payload of the
#     same length. Same gaps between resume kinds as live; no live content; no seats,
#     jobs or registry rows, so the private daemon has nothing to act on.
#   private-replay.sh run <label> <pij-rs binary> <port> <client seconds> [cursor]
#     Boots the daemon on the seed, waits 10 s idle, runs chainglass-like.mjs for
#     <client seconds>, sampling CPU time and RSS every 5 s.
set -u
S=${SCRATCH:?set SCRATCH}
HERE=$(cd "$(dirname "$0")" && pwd)
SEED=$S/seed
LIVE=$HOME/.pij-rs/pij.sqlite
mode=$1; shift

boot() { # <bin> <port> <log>
  export HOME="$SEED/home" PIJ_RS_STATE_DIR="$SEED/state" PIJ_TMUX_SERVER="$SERVER" PIJ_RS_ADDR="127.0.0.1:$2"
  tmux -L "$SERVER" -f /dev/null new-session -d -s bench -n w0 'sleep 100000'
  "$1" daemon --bind "127.0.0.1:$2" --state-dir "$SEED/state" >>"$3" 2>&1 &
  PID=$!
  for _ in $(seq 1 300); do
    curl -s "http://127.0.0.1:$2/health" >/dev/null 2>&1 && [ -s "$SEED/state/daemon.key" ] && return 0
    sleep 0.2
  done
  echo "daemon did not come up" >&2; return 1
}
halt() {
  kill -TERM "$PID" 2>/dev/null
  for _ in $(seq 1 150); do kill -0 "$PID" 2>/dev/null || break; sleep 0.2; done
  tmux -L "$SERVER" kill-server 2>/dev/null
}
cpu_s() { ps -o time= -p "$1" | awk -F'[:.]' '{ if (NF==4) print $1*3600+$2*60+$3+$4/100; else print $1*60+$2+$3/100 }'; }

case $mode in
seed)
  BIN=$1; SERVER="pij-187-seed-$$"
  rm -rf "$SEED"; mkdir -p "$SEED/home" "$SEED/state"
  boot "$BIN" 17871 "$SEED/seed-daemon.log" && halt
  head=$(sqlite3 -readonly "$LIVE" "select max(seq) from spine_events")
  sqlite3 "$SEED/state/pij.sqlite" <<SQL
-- The private boot's own few rows would collide with live seqs; this is the
-- private seed's spine, never the live one (R5).
DELETE FROM spine_events;
ATTACH 'file:$LIVE?mode=ro' AS live;
INSERT INTO spine_events (seq, v, at, kind, seat, payload)
  SELECT seq, v, at, kind, seat,
         '{"pad":"' || substr(replace(hex(zeroblob(length(payload))), '0', 'x'), 1, max(length(payload) - 10, 0)) || '"}'
  FROM live.spine_events WHERE seq <= $head;
DETACH live;
SQL
  sqlite3 -readonly "$SEED/state/pij.sqlite" "select 'seed rows=' || count(*) || ' head=' || max(seq) || ' bytes=' || sum(length(payload)) from spine_events"
  du -sh "$SEED"
  ;;
run)
  LABEL=$1; BIN=$2; PORT=$3; SECS=$4; CURSOR=${5:-0}; SERVER="pij-187-$LABEL-$$"
  OUT=$S/receipts/$LABEL.txt; mkdir -p "$S/receipts"; : > "$OUT"
  LOG=$S/receipts/$LABEL-daemon.log; : > "$LOG"
  boot "$BIN" "$PORT" "$LOG" || exit 1
  KEY=$(cat "$SEED/state/daemon.key")
  ALIAS=$(curl -s -H "Authorization: Bearer $KEY" "http://127.0.0.1:$PORT/health" | jq -r '.data.machine')
  head=$(sqlite3 -readonly "$SEED/state/pij.sqlite" "select max(seq) from spine_events")
  echo "label=$LABEL bin=$(basename "$BIN") pid=$PID head=$head start_cursor=$CURSOR client_seconds=$SECS" >> "$OUT"
  sleep 10
  echo "idle +10s cpu_s=$(cpu_s $PID) rss_kb=$(ps -o rss= -p $PID | tr -d ' ')" >> "$OUT"
  node "$HERE/chainglass-like.mjs" "$PORT" "$KEY" "$ALIAS" "$SECS" "$CURSOR" > "$S/receipts/$LABEL-client.jsonl" &
  CLIENT=$!
  t0=$(date +%s); prev=$(cpu_s $PID)
  while kill -0 $CLIENT 2>/dev/null; do
    sleep 5
    now=$(cpu_s $PID)
    echo "+$(( $(date +%s) - t0 ))s cpu_pct=$(awk -v a="$prev" -v b="$now" 'BEGIN{printf "%.0f", (b-a)*20}') rss_kb=$(ps -o rss= -p $PID | tr -d ' ')" >> "$OUT"
    prev=$now
  done
  wait $CLIENT
  echo "daemon_log_lines=$(wc -l < "$LOG") reconciliation_lines=$(grep -c 'background reconciliation failed' "$LOG")" >> "$OUT"
  halt
  cat "$OUT"; tail -1 "$S/receipts/$LABEL-client.jsonl"
  ;;
esac
