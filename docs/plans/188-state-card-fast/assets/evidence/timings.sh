#!/bin/bash
# timings.sh <label> <port> <keyfile>
LABEL=$1; PORT=$2; KEY=$(cat $3)
st(){ curl -s -o /dev/null -w "%{time_total} " -H "Authorization: Bearer $KEY" -H 'content-type: application/json' -d "{\"id\":\"$1\"}" http://127.0.0.1:$PORT/v1/state; }
echo "## $LABEL (port $PORT) $(date -u +%FT%TZ) load: $(uptime | sed 's/.*averages: //')"
for s in pij-state-cards pij-far-jackal pij-causal-skeram pij-different-smelt; do
  echo "single $s: $(st $s) $(st $s) $(st $s) s"
done
for round in 1 2 3; do
  r=$( (for i in 1 2 3 4; do st pij-far-jackal & done; wait) | tr '\n' ' ' ); echo "4 concurrent far-jackal (round $round): $r"
done
r=$( (for s in pij-state-cards pij-far-jackal pij-causal-skeram pij-different-smelt; do st $s & done; wait) | tr '\n' ' '); echo "4 concurrent, 4 different seats incl. smelt: $r"
code=$(curl -s -o /private/tmp/claude-501/-Users-jordanknight-pi-hacking-fleet-188-state-cards/eb24f8bb-847f-4ffd-91cd-50fc2d250835/scratchpad/188/cards.json -w "%{http_code} %{time_total}" -H "Authorization: Bearer $KEY" http://127.0.0.1:$PORT/v1/cards)
echo "GET /v1/cards: $code s, cards=$(jq '.data.cards | length' /private/tmp/claude-501/-Users-jordanknight-pi-hacking-fleet-188-state-cards/eb24f8bb-847f-4ffd-91cd-50fc2d250835/scratchpad/188/cards.json 2>/dev/null)"
rm -f /private/tmp/claude-501/-Users-jordanknight-pi-hacking-fleet-188-state-cards/eb24f8bb-847f-4ffd-91cd-50fc2d250835/scratchpad/188/cards.json
