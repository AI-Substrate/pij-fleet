#!/bin/bash
# Plan 210 phase 2, AC7: re-point the live governance roles whose prompts cite
# docs/project/roadmaps/ at docs/project/workstreams/, with the directory
# writer (one `pij directory edit --file` per role, never a hand edit), when
# the rename merges so the paths flip together.
#
#   ac7-repoint.sh [--dry-run]
#
# Each role's document is read from the governance branch head, only the path
# changes, and `edit --file` keeps its changelog and adds one row.
set -euo pipefail
DRY=${1:-}
ROOT=$(bash "$(dirname "$0")/../../../../../harness/scripts/government-root.sh")
ROLES="$ROOT/community/roles"
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
for role in $(grep -l 'docs/project/roadmaps/' "$ROLES"/*/role.dd.json | xargs -n1 dirname | xargs -n1 basename); do
  python3 - "$ROLES/$role/role.dd.json" "$TMP/$role.dd.json" <<'PY'
import json, sys
text = open(sys.argv[1]).read()
doc = json.loads(text.replace("docs/project/roadmaps/", "docs/project/workstreams/"))
# The writer adds this edit's changelog row; keep the document's own rows.
open(sys.argv[2], "w").write(json.dumps(doc, indent="\t") + "\n")
PY
  if [ "$DRY" = "--dry-run" ]; then
    echo "would edit $role: $(diff <(python3 -m json.tool "$ROLES/$role/role.dd.json") <(python3 -m json.tool "$TMP/$role.dd.json") | grep -c '^>') line(s)"
  else
    pij directory edit "$role" --why 'C29 rename (plan 210)' --file "$TMP/$role.dd.json"
  fi
done
