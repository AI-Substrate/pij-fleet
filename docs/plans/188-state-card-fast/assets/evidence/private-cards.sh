#!/bin/bash
# Plan 188 private card bench (R2, R3): temp HOME + state dir, private -L tmux
# server, private port. Never the machine daemon; ~/.pij-rs is a read-only seed source.
#
#   private-cards.sh seed <pij-rs binary>
#     One seed, reused. spine_events copies the LIVE spine's seq, v, at, kind and
#     seat (read-only ATTACH). Payloads are synthetic padding of the same length,
#     EXCEPT the five kinds the card decodes (report.now, report.state,
#     delivery.held, delivery.released, delivery.outcome), which keep their real
#     payload so every seat's card is a real card. seats and seat_roles are copied
#     with pid, proc_start, pane and harness_session nulled, so the private daemon
#     has no process, pane or session it could reach. No jobs, fyis or watchdogs.
#   private-cards.sh boot <pij-rs binary> <port>    # prints PID KEY
#   private-cards.sh halt <pid> <server>
set -u
S=${SCRATCH:?set SCRATCH}
SEED=$S/seed
LIVE=$HOME/.pij-rs/pij.sqlite
mode=$1; shift

boot() { # <bin> <port> <log> <server>
  export HOME="$SEED/home" PIJ_RS_STATE_DIR="$SEED/state" PIJ_TMUX_SERVER="$4" PIJ_RS_ADDR="127.0.0.1:$2"
  tmux -L "$4" -f /dev/null new-session -d -s bench -n w0 'sleep 100000'
  "$1" daemon --bind "127.0.0.1:$2" --state-dir "$SEED/state" >>"$3" 2>&1 &
  PID=$!
  for _ in $(seq 1 600); do
    curl -s "http://127.0.0.1:$2/health" >/dev/null 2>&1 && [ -s "$SEED/state/daemon.key" ] && return 0
    sleep 0.2
  done
  echo "daemon did not come up" >&2; return 1
}
halt() { # <pid> <server>
  kill -TERM "$1" 2>/dev/null
  for _ in $(seq 1 150); do kill -0 "$1" 2>/dev/null || break; sleep 0.2; done
  tmux -L "$2" kill-server 2>/dev/null
}

case $mode in
seed)
  BIN=$1; SERVER="pij-188-seed-$$"
  rm -rf "$SEED"; mkdir -p "$SEED/home" "$SEED/state"
  boot "$BIN" 17881 "$SEED/seed-daemon.log" "$SERVER" && halt "$PID" "$SERVER"
  head=$(sqlite3 -readonly "$LIVE" "select max(seq) from spine_events")
  sqlite3 "$SEED/state/pij.sqlite" <<SQL
DELETE FROM spine_events;
DELETE FROM seats;
DELETE FROM seat_roles;
ATTACH 'file:$LIVE?mode=ro' AS live;
INSERT INTO spine_events (seq, v, at, kind, seat, payload)
  SELECT seq, v, at, kind, seat,
         CASE WHEN kind IN ('report.now','report.state','delivery.held','delivery.released','delivery.outcome')
              THEN payload
              ELSE '{"pad":"' || substr(replace(hex(zeroblob(length(payload))), '0', 'x'), 1, max(length(payload) - 10, 0)) || '"}'
         END
  FROM live.spine_events WHERE seq <= $head;
INSERT INTO seats SELECT * FROM live.seats;
UPDATE seats SET pid = NULL, proc_start = NULL, pane = NULL, harness_session = NULL, native_extension_delivery = 0;
INSERT INTO seat_roles SELECT * FROM live.seat_roles;
DETACH live;
SQL
  sqlite3 -readonly "$SEED/state/pij.sqlite" "select 'seed rows=' || count(*) || ' head=' || max(seq) from spine_events; select 'seats=' || count(*) || ' live=' || sum(tombstoned_at is null) from seats"
  du -sh "$SEED"
  ;;
boot)
  BIN=$1; PORT=$2; SERVER="pij-188-$PORT-$$"
  boot "$BIN" "$PORT" "$S/daemon-$PORT.log" "$SERVER" || exit 1
  echo "$PID $SERVER $(cat "$SEED/state/daemon.key")"
  ;;
halt)
  halt "$1" "$2"
  ;;
esac
