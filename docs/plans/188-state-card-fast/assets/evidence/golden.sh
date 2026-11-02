#!/bin/bash
# golden.sh <port> <out.jsonl>: /v1/state for every seat in the seed, one JSON line each.
S=${SCRATCH:?set SCRATCH}
KEY=$(cat $S/seed/state/daemon.key); : > "$2"
sqlite3 -readonly $S/seed/state/pij.sqlite "select id from seats order by id" | while read id; do
  curl -s -H "Authorization: Bearer $KEY" -H 'content-type: application/json' -d "{\"id\":\"$id\"}" "http://127.0.0.1:$1/v1/state" | jq -c '.data' >> "$2"
done
echo "done $(wc -l < "$2")"
