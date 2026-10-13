#!/usr/bin/env bash
# pij-skill-check — structural gates for skills/pij (plans 030 and 042).
# Checks: registry↔module parity (pending-marked rows exempt) · sibling-blindness ·
# line budgets · CLI-verb coverage · duplicated-prose scope · prime payload integrity
# and portability · stream-orchestrator journey and worktree lifecycle.
set -u
cd "$(git rev-parse --show-toplevel 2>/dev/null || pwd)"
SKILL=${PIJ_SKILL_ROOT:-skills/pij}
FAIL=0
err() { echo "✗ $1"; FAIL=1; }
ok()  { echo "✓ $1"; }
warn() { echo "⚠ $1"; }

# 1. registry ↔ module parity
registry_rows=$(sed -n '/^## Registry/,/^## /p' "$SKILL/SKILL.md" | grep '^| `' || true)
prime_rows=$(printf '%s\n' "$registry_rows" | grep '^| `prime` |' || true)
prime_row_count=$(printf '%s\n' "$prime_rows" | grep -c '^| `prime` |' || true)
valid_prime_row_count=$(printf '%s\n' "$prime_rows" \
  | grep -Ec '^\| `prime` \|.*\| `references/routes/prime\.md` \|$' || true)
pending_prime_row_count=$(printf '%s\n' "$prime_rows" \
  | grep -Ec 'lands Phase 2|future|no route module' || true)
if [ "$prime_row_count" -eq 1 ] \
  && [ "$valid_prime_row_count" -eq 1 ] \
  && [ "$pending_prime_row_count" -eq 0 ]; then
  ok "registry: exactly one active 'prime' row → references/routes/prime.md"
else
  err "registry: expected exactly one active 'prime' row → references/routes/prime.md"
fi

while IFS= read -r line; do
  route=$(printf '%s' "$line" | sed -n 's/^| `\([a-z]*\)`.*/\1/p')
  [ -z "$route" ] && continue
  mod="$SKILL/references/routes/$route.md"
  if printf '%s' "$line" | grep -q 'lands Phase 2\|future\|no route module'; then
    [ -f "$mod" ] && err "registry: '$route' marked pending but module EXISTS ($mod) — unmark the row" \
                  || ok "registry: '$route' pending (no module, as marked)"
  else
    [ -f "$mod" ] && ok "registry: '$route' → $mod" || err "registry: '$route' has no module at $mod"
  fi
done <<< "$registry_rows"

# 2. sibling-blindness: no route module names another route module or "/pij <other>".
# prime's deterministic worker-redirect row is the one scoped exception.
for f in "$SKILL"/references/routes/*.md; do
  [ -e "$f" ] || continue
  self=$(basename "$f" .md)
  for r in ready pair delegate agent skill peer ops prime watch node; do
    [ "$r" = "$self" ] && continue
    hits=$(grep -En "routes/$r\.md|/pij $r\b" "$f" 2>/dev/null || true)
    if [ "$self" = prime ] && { [ "$r" = pair ] || [ "$r" = peer ]; }; then
      hits=$(printf '%s\n' "$hits" | grep -Ev '^[0-9]+:\|.*\| worker \|.*`/pij pair`.*`/pij peer`' || true)
    fi
    if [ -n "$hits" ]; then
      err "sibling-blind: $(basename "$f") references route '$r'"
    fi
  done
done
ok "sibling-blindness scanned"

# 3. line budgets
budget() {
  [ -f "$1" ] || return 0
  n=$(wc -l < "$1" | tr -d ' ')
  if [ "$n" -le "$2" ]; then ok "budget: $1 ($n/$2)"; else err "budget: $1 is $n lines (max $2)"; fi
}
budget "$SKILL/SKILL.md" 150
budget "$SKILL/references/00-routing.md" 250
budget "$SKILL/references/routes/pair.md" 350
for m in ready peer agent skill ops delegate node; do budget "$SKILL/references/routes/$m.md" 150; done

soft_budget() {
  [ -f "$1" ] || return 0
  n=$(wc -l < "$1" | tr -d ' ')
  if [ "$n" -le "$2" ]; then
    ok "budget: $1 ($n/$2, advisory)"
  else
    warn "budget: $1 is $n lines (advisory max $2)"
  fi
}
soft_budget "$SKILL/references/routes/prime.md" 90
for m in bootstrap kickoff batons reports incidents; do
  soft_budget "$SKILL/references/prime/rituals/$m.md" 90
done
soft_budget "$SKILL/references/prime/protocol.md" 170
budget "$SKILL/references/prime/orchestrator.md" 120

# 4. CLI-verb coverage: every required bin family is mapped in the coverage table.
cli_rows=$(sed -n '/^## CLI-verb coverage/,/^## /p' "$SKILL/SKILL.md" | grep '^| `' || true)
for v in spawn close adopt tree link daemon compact-self commit-trailers telegram agent whoami list send tail state phonehome path models orchestration baton prime project spine task node anomalies chore; do
  printf '%s\n' "$cli_rows" | grep -Fq "\`$v\`" \
    || err "verb coverage: '$v' unmapped in CLI-verb coverage table"
done
ok "CLI-verb coverage scanned"

# 5. duplicated-prose: convention headings (### C<n> —) live only in 00-routing.md
dups=$(grep -rlE '^### C[0-9]+ — ' "$SKILL" --include='*.md' 2>/dev/null | grep -v '00-routing.md' || true)
[ -n "$dups" ] && err "dup-prose: convention headings outside 00-routing.md: $dups" || ok "dup-prose: conventions single-owner"

# 6. completion-first compaction: root interrupt, C3 ownership, pair order, and C7 delivery.
root_skill="$SKILL/SKILL.md"
routing="$SKILL/references/00-routing.md"
pair_route="$SKILL/references/routes/pair.md"

section() {
  file=$1
  heading=$2
  awk -v heading="$heading" '
    $0 == heading { found=1; print; next }
    found && /^##[#]? / { exit }
    found { print }
  ' "$file"
}

completion_marker() {
  file=$1
  marker=$2
  label=$3
  if [ -f "$file" ] && grep -Fq "$marker" "$file"; then
    ok "$label"
  else
    err "$label — missing '$marker' in $file"
  fi
}

require_count() {
  file=$1
  marker=$2
  expected=$3
  label=$4
  count=$(grep -Fc "$marker" "$file" 2>/dev/null || true)
  [ "$count" -eq "$expected" ] \
    && ok "$label" \
    || err "$label — expected $expected occurrence(s) of '$marker' in $file, found $count"
}

require_order() {
  file=$1
  label=$2
  shift 2
  previous=0
  for marker in "$@"; do
    line=$(grep -nF "$marker" "$file" | head -1 | cut -d: -f1 || true)
    if [ -z "$line" ]; then
      err "$label — missing '$marker'"
      return
    elif [ "$line" -le "$previous" ]; then
      err "$label — '$marker' is out of order"
      return
    fi
    previous=$line
  done
  ok "$label"
}

reject_compact_progress_gate() {
  contract=$1
  label=$2
  hits=$(printf '%s\n' "$contract" \
    | tr '[:upper:]' '[:lower:]' \
    | sed -E 's/[.!?;]+/\n/g; s/[[:space:]]+(but|however)[[:space:]]+/\n/g' \
    | grep -Ev '(never|do not|don.t|without)[^.!?;]*(wait|block|gate|delay|require|depend)' \
    | grep -E '(receipt|executed[[:space:]]*:[[:space:]]*`?true|acknowledg|confirmation|compact(ion)?.*(complete|completion|finish|done|return|confirm|ack|deliver|succeed)|(complete|completion|finish|done|return|confirm|ack|deliver)[[:space:]]+(of|from)[[:space:]]+(the[[:space:]]+)?compact(ion)?)' \
    | grep -E '(wait|until|before|after|once|block|gate|delay|require|depend|must.*(arrive|complete|confirm|ack))' \
    | grep -E '(report|review|fix|next[- ]pointer|progress|continue|proceed|dispatch|handle|process|advance)' \
    || true)
  if [ -n "$hits" ]; then
    err "$label — compact receipt/completion cannot gate progress"
  else
    ok "$label"
  fi
}

require_count "$root_skill" "**Completion interrupt**" 1 \
  "completion root: exactly one always-loaded interrupt"
completion_marker "$root_skill" "§ C3 owns the lifecycle boundary and command contract." \
  "completion root: interrupt points to C3"
completion_marker "$root_skill" "**Delivery-owned waiting**" \
  "completion root: delivery-owned waiting invariant preserved"
completion_marker "$root_skill" 'non-tmux external peers block on `pij inbox --wait`' \
  "completion root: external inbox waiting preserved"
completion_marker "$root_skill" 'Never sit in a `pij state` wait loop.' \
  "completion root: state wait loop forbidden"

require_count "$routing" "### C3 — Compact discipline (early, not late)" 1 \
  "completion C3: exactly one convention owner"
c3=$(section "$routing" '### C3 — Compact discipline (early, not late)')
for marker in \
  "reusable/live" \
  "first tool action" \
  "30–90s compact latency overlaps" \
  "fire-and-forget" \
  'without `--wait`' \
  "Continue immediately" \
  'one-shot `--once`' \
  '`E-DEAD`' \
  "observe-only diagnostics, never progress gates" \
  "compact, keep, reuse"; do
  printf '%s\n' "$c3" | grep -Fq "$marker" \
    && ok "completion C3: $marker" \
    || err "completion C3: missing '$marker'"
done
printf '%s\n' "$c3" \
  | grep -Eq '`pij send [^`]*(--command compact|"/compact")[^`]*--wait[^`]*`' \
  && err "completion C3: compact command must not include --wait" \
  || ok "completion C3: compact command is non-blocking"
reject_compact_progress_gate "$c3" \
  "completion C3: no additive receipt/completion progress gate"

require_count "$pair_route" "## Completion interrupt — compact EARLY" 1 \
  "completion pair: exactly one compact-early block"
require_order "$pair_route" "completion pair: coder compact precedes report handling" \
  "**Coder completion**" "compact the coder FIRST" "handle the report"
require_order "$pair_route" "completion pair: reviewer compact precedes verdict handling" \
  "**Reviewer verdict**" "compact the reviewer FIRST" 'follow the `FIX` or `APPROVE` path'
require_order "$pair_route" "completion pair: reload-first safety precedes compact" \
  "**Buggy-extension safety**" "reload FIRST" "then compact"
pair_completion=$(section "$pair_route" '## Completion interrupt — compact EARLY')
printf '%s\n' "$pair_completion" | grep -Fq 'without `--wait`' \
  && ok "completion pair: compact dispatch forbids --wait" \
  || err "completion pair: missing no-compact-wait marker"
printf '%s\n' "$pair_completion" | grep -Fq "continue immediately" \
  && ok "completion pair: report/review/fix work continues immediately" \
  || err "completion pair: missing immediate-continuation marker"
printf '%s\n' "$pair_completion" | grep -Fq "Never wait for compact receipts or completion" \
  && ok "completion pair: compact receipt/completion cannot gate progress" \
  || err "completion pair: missing no-receipt-gate marker"
printf '%s\n' "$pair_completion" \
  | grep -Eq '`pij send [^`]*(--command compact|"/compact")[^`]*--wait[^`]*`' \
  && err "completion pair: compact command must not include --wait" \
  || ok "completion pair: compact command is non-blocking"
reject_compact_progress_gate "$pair_completion" \
  "completion pair: no additive receipt/completion progress gate"

c7=$(section "$routing" '### C7 — Push when owned; block on inbox when pull-owned')
for marker in \
  "pi/tmux push modes" \
  'never poll `pij state`' \
  "external pull mode" \
  '`pij inbox --wait`' \
  "This blocking inbox read is the delivery primitive"; do
  printf '%s\n' "$c7" | grep -Fq "$marker" \
    && ok "completion C7: $marker" \
    || err "completion C7: missing '$marker'"
done

# 7. ready route stays terminal: register, report, and wait without work discovery.
ready_route="$SKILL/references/routes/ready.md"
for marker in \
  'pij adopt "$TMUX_PANE" --harness <h>' \
  'pij inbox register --json' \
  'reply with exactly:' \
  'Ready.' \
  '**STOP.**' \
  'ending the turn is the wait' \
  'Do not read plans, briefs, government files, repository docs, git state, task' \
  'Do not run a harness boot, inspect peers, spawn, send,'; do
  completion_marker "$ready_route" "$marker" "ready terminal: $marker"
done

# 8. rs node contracts replace retired tree/link/list projections; docs/how/pij-rs-api.md#unsupported-status.
peer_route="$SKILL/references/routes/peer.md"
prime_route="$SKILL/references/routes/prime.md"
kickoff="$SKILL/references/prime/rituals/kickoff.md"
handover="$SKILL/references/prime/templates/seat-handover.md"

for marker in \
  'pij node show <seat> --json' \
  '`tree` still refuses' \
  'Link takes a seat with no live parent, or re-roles one you parent.' \
  'pij adopt "$TMUX_PANE" --harness <h> ${PIJ_PARENT_ID:+--parent "$PIJ_PARENT_ID"}' \
  'recorded `parent`, not a spawnedBy fallback' \
  'Close requires self or the current recorded parent.' \
  '`--scope local`' \
  'Legacy `--role`, `--prime` and `--archived` remain refused.' \
  'Both shim and native `pij-rs list --here` select the caller' \
  'pij inbox register --json' \
  'pij inbox --wait' \
  'matching long-lived host incarnation'; do
  completion_marker "$peer_route" "$marker" "rs peer: $marker"
done

# No oldPrime/list --prime projection is ported: require designation evidence, not role inference; docs/how/pij-rs-api.md#unsupported-status.
for marker in \
  '`pij list --json` is the unfiltered active rs roster' \
  '`pij node show <id> --json` is its governed subtree' \
  'List `--prime`/`--archived` and tree are unported, not alternative probes.' \
  '**not prime designation**' \
  'There is no dedicated rs prime-list/getter projection in this cutover' \
  'A missing local directory or empty active roster never proves no prime exists.' \
  '`pij spine events --json`'; do
  completion_marker "$prime_route" "$marker" "rs prime triage: $marker"
done

# Commit attribution has one rule owner; commit boundaries cite it.
require_count "$routing" "### C11 — Commit attribution" 1 \
  "commit attribution: exactly one convention owner"
c11=$(section "$routing" '### C11 — Commit attribution')
for marker in 'pij-rs commit-trailers' 'Pij-Seat' 'Pij-Prime' 'Pij-Plan' 'never guess'; do
  printf '%s\n' "$c11" | grep -Fq "$marker" \
    && ok "commit attribution C11: $marker" \
    || err "commit attribution C11: missing '$marker'"
done
for file in \
  "$peer_route" "$pair_route" "$prime_route" \
  "$SKILL/references/prime/protocol.md" \
  "$SKILL/references/prime/orchestrator.md" "$kickoff"; do
  completion_marker "$file" '§ C11' "commit attribution: citation in $file"
done

# Node evidence replaces tree; role assertion follows verified parent, never link repair; docs/how/pij-rs-api.md#unsupported-status.
require_order "$kickoff" "rs kickoff: recorded parent verified before canary" \
  "**Verify placement.**" \
  '`pij node show <id> --json`' \
  "**Canary legs (a) and (b).**"
require_order "$kickoff" "rs kickoff: canary identity before role before brief" \
  "Complete canary leg (a) round-trip and leg (b) identity proof" \
  '`pij role <id> pm --json` after verifying the recorded parent' \
  "**Deliver the brief by pointer as canary leg (c).**"

# Rs designation names the incoming seat; retirement history comes from spine, not oldPrime/tree; docs/how/pij-rs-api.md#unsupported-status.
require_order "$handover" "rs handover: set before writers, retire after final relay" \
  '`pij orchestration prime set <incoming-pij-id> --json`' \
  "before changing any writer line" \
  "outgoing seat's FINAL send" \
  '`pij orchestration prime retire <outgoing-pij-id> --json`' \
  '`pij spine events --peer <outgoing-pij-id> --json`'
completion_marker "$handover" '`prime.state` is retired' \
  "rs handover: retired designation remains spine history"
completion_marker "$handover" 'No `oldPrime` tree/list field is implied.' \
  "rs handover: no fabricated legacy projection"

# 9. prime payload: exact tree + every relative markdown pointer resolves.
prime_required=(
  "$SKILL/references/routes/prime.md"
  "$SKILL/references/prime/orchestrator.md"
  "$SKILL/references/prime/orient-oprime.md"
  "$SKILL/references/prime/orient-global.md"
  "$SKILL/references/prime/prime-flow.schema.json"
  "$SKILL/references/prime/biome.json"
  "$SKILL/references/prime/rituals/bootstrap.md"
  "$SKILL/references/prime/rituals/kickoff.md"
  "$SKILL/references/prime/rituals/batons.md"
  "$SKILL/references/prime/rituals/reports.md"
  "$SKILL/references/prime/rituals/incidents.md"
  "$SKILL/references/prime/templates/spine.md"
  "$SKILL/references/prime/templates/baton-book.md"
  "$SKILL/references/prime/templates/stream-brief.md"
  "$SKILL/references/prime/templates/orient-local.md"
  "$SKILL/references/prime/templates/seat-handover.md"
  "$SKILL/references/prime/protocol.md"
  "$SKILL/references/prime/exemplars/canary-record.md"
  "$SKILL/references/prime/exemplars/grant-log.md"
)
for f in "${prime_required[@]}"; do
  [ -f "$f" ] && ok "prime payload: $f" || err "prime payload missing: $f"
done

check_links() {
  file=$1
  dir=$(dirname "$file")
  while IFS= read -r target; do
    target=${target%%#*}
    [ -z "$target" ] && continue
    case "$target" in
      /*|http://*|https://*|mailto:*) continue ;;
    esac
    [ -e "$dir/$target" ] || err "prime pointer: $file → $target is missing"
  done < <(sed '/^[[:space:]]*```/,/^[[:space:]]*```/d' "$file" \
    | grep -Eo '\]\([^)]*\)' | sed 's/^](//; s/)$//' || true)
}
check_links "$SKILL/references/routes/prime.md"
while IFS= read -r f; do check_links "$f"; done < <(find "$SKILL/references/prime" -type f -name '*.md' | sort)
ok "prime pointer-integrity scanned"

# 10. stream-orchestrator route, ordered journey, and lifecycle markers.
orchestrator="$SKILL/references/prime/orchestrator.md"
prime_route="$SKILL/references/routes/prime.md"

stream_rows=$(sed -n '/^## Role triage/,/^## /p' "$prime_route" | grep -E '^\|.*\| stream \|' || true)
stream_row_count=$(printf '%s\n' "$stream_rows" | grep -c 'stream' || true)
stream_pointer_count=$(printf '%s\n' "$stream_rows" \
  | grep -Ec '\]\(\.\./prime/orchestrator\.md\)' || true)
if [ "$stream_row_count" -eq 1 ] && [ "$stream_pointer_count" -eq 1 ]; then
  ok "orchestrator route: one stream row → prime/orchestrator.md"
else
  err "orchestrator route: expected one stream row → prime/orchestrator.md"
fi
printf '%s\n' "$stream_rows" | grep -Fq '../prime/orient-global.md' \
  && err "orchestrator route: stream row bypasses module-first landing" \
  || ok "orchestrator route: stream row does not bypass the landing module"

orchestrator_registry_rows=$(printf '%s\n' "$registry_rows" | grep '^| `orchestrator` |' || true)
[ -z "$orchestrator_registry_rows" ] \
  && ok "orchestrator registry: no second top-level route row" \
  || err "orchestrator registry: forbidden second top-level 'orchestrator' row"

require_marker() {
  file=$1
  marker=$2
  label=$3
  if [ -f "$file" ] && grep -Fq "$marker" "$file"; then
    ok "$label"
  else
    err "$label — missing '$marker' in $file"
  fi
}

if [ -f "$orchestrator" ]; then
  require_marker "$orchestrator" "You are a stream orchestrator" "orchestrator order: role"
  previous=0
  while IFS='|' read -r label marker; do
    [ -z "$marker" ] && continue
    line=$(section "$orchestrator" '## Ordered entry' | grep -nF "$marker" | head -1 | cut -d: -f1 || true)
    if [ -z "$line" ]; then
      err "orchestrator order: missing $label marker '$marker'"
    elif [ "$line" -lt "$previous" ]; then
      err "orchestrator order: $label marker '$marker' is out of order"
    else
      ok "orchestrator order: $label"
      previous=$line
    fi
  done <<'EOF'
global orient|orient-global.md
local orient|<government-root>/orient-local.md
brief|item brief
thesis|/thesis
real invocation|host skill mechanism
preamble|human preamble
planning|guided `/builder`
validation|/validate-v2
wait gate|WAITING_FOR_BUILD_CONFIG
construction|stream worktree, branch, approved base
delegation|/pij pair
landing|/builder 8 ship
EOF

  require_marker "$orchestrator" "A plausible thesis written from memory does not satisfy this step." \
    "orchestrator contract: thesis anti-fake wording"
  require_marker "$orchestrator" "separate Copilot gpt-5.6-sol @ xhigh coder" \
    "orchestrator contract: exact default coder profile"
  require_marker "$orchestrator" "separate Copilot gpt-5.6-sol @ xhigh reviewer" \
    "orchestrator contract: exact default reviewer profile"
  require_marker "$orchestrator" "read it back verbatim" \
    "orchestrator contract: verbatim profile read-back"
  require_marker "$orchestrator" "never the o-prime's window" \
    "orchestrator contract: anti-prime-window topology"
  require_marker "$orchestrator" "source-verify every claimed seam" \
    "orchestrator contract: source-verified seams"
  require_marker "$orchestrator" "immutable" \
    "orchestrator contract: immutable packets"
  require_marker "$orchestrator" "reviewer forms findings" \
    "orchestrator contract: reviewer-owned findings"
  require_marker "$orchestrator" "stop and re-brief" \
    "orchestrator contract: frozen review lane"
  require_marker "$orchestrator" "persisted findings" \
    "orchestrator contract: non-empty fix evidence"
  require_marker "$orchestrator" "shared-tree fallback" \
    "orchestrator contract: shared-tree is fallback"
  require_marker "$orchestrator" "outage-first" \
    "orchestrator recovery: silence is outage-first"
  require_marker "$orchestrator" "15-minute cadence" \
    "orchestrator recovery: bounded silence cadence"
  require_marker "$orchestrator" '`COMPLETE`, `CONTINUING`, or `BLOCKED`' \
    "orchestrator recovery: explicit status response"
  require_marker "$orchestrator" "poke before redispatch" \
    "orchestrator recovery: poke before redispatch"
  require_marker "$orchestrator" "repeated short-interval polling" \
    "orchestrator recovery: no polling regression"
  require_marker "$orchestrator" "current provided-peer path" \
    "orchestrator pair config: provided-peer path named"
  require_marker "$orchestrator" "persist the plan roster" \
    "orchestrator pair config: plan roster persistence"
  require_marker "$orchestrator" "current flow-pair engine does not persist" \
    "orchestrator pair config: engine persistence ceiling"

  previous=0
  while IFS='|' read -r label marker; do
    line=$(grep -nF -- "$marker" "$orchestrator" | head -1 | cut -d: -f1 || true)
    if [ -z "$line" ]; then
      err "orchestrator pair order: missing $label marker '$marker'"
    elif [ "$line" -lt "$previous" ]; then
      err "orchestrator pair order: $label marker '$marker' is out of order"
    else
      ok "orchestrator pair order: $label"
      previous=$line
    fi
  done <<'EOF'
human confirmation|After the human confirms the fleet
coder override|--coder-model <confirmed>
reviewer override|--reviewer-model <confirmed>
phase delegation|Delegate each whole phase
EOF

  direct_build=$(grep -Ein \
    'implement (it |the phase |the plan )?(yourself|directly)|start implementing|write (the )?code yourself|build directly' \
    "$orchestrator" || true)
  [ -z "$direct_build" ] \
    && ok "orchestrator contract: no direct-build permission" \
    || err "orchestrator contract: forbidden direct-build language"

  authored_findings=$(grep -Ein \
    '(author|write|form|invent) (the )?(reviewer |review )?findings|review findings before the reviewer' \
    "$orchestrator" || true)
  [ -z "$authored_findings" ] \
    && ok "orchestrator contract: no orchestrator-authored findings" \
    || err "orchestrator contract: forbidden orchestrator-authored findings"
else
  err "orchestrator contract: missing $orchestrator"
fi

for f in \
  "$SKILL/references/prime/rituals/kickoff.md" \
  "$SKILL/references/prime/templates/stream-brief.md" \
  "$SKILL/references/prime/templates/spine.md"; do
  spawn_cwd_leak=$(grep -En 'pij spawn.*--cwd' "$f" 2>/dev/null || true)
  [ -z "$spawn_cwd_leak" ] \
    && ok "peer spawn cwd: $(basename "$f") does not invent --cwd" \
    || err "peer spawn cwd: $(basename "$f") documents unsupported pij spawn --cwd"
done

# Governance has a permanent worktree; only stream construction belongs to kickoff.
bootstrap_worktree_commands=$(grep -Eo 'git( -C "[^"]+")? worktree add[^`]*' \
  "$SKILL/references/prime/rituals/bootstrap.md" || true)
bootstrap_worktree_add_count=$(printf '%s\n' "$bootstrap_worktree_commands" | grep -c 'worktree add' || true)
bootstrap_governance_count=$(printf '%s\n' "$bootstrap_worktree_commands" \
  | grep -Ec '^git -C "<repo>" worktree add (--orphan -b prime-governance "<standing-worktree>"|"<standing-worktree>" prime-governance)$' || true)
if [ "$bootstrap_worktree_add_count" -eq 2 ] && [ "$bootstrap_governance_count" -eq 2 ]; then
  ok "worktree lifecycle: bootstrap creates/restores governance only"
else
  err "worktree lifecycle: bootstrap may create/restore only the standing prime-governance worktree"
fi
require_marker "$SKILL/references/prime/rituals/bootstrap.md" \
  "Kickoff is the sole construction owner" \
  "worktree lifecycle: kickoff is sole construction owner"
# Rs stream allocation owns creation; handwritten git worktree add is retired here; docs/how/pij-rs-api.md#governance-route-inventory.
require_count "$kickoff" 'pij stream create --project' 1 \
  "worktree lifecycle: kickoff has exactly one stream allocation command"
require_order "$kickoff" "worktree lifecycle: verified allocation precedes spawn" \
  'pij stream create --project' \
  'it owns worktree creation/resume and branch/base verification' \
  '**Spawn into the worktree.**'
require_marker "$SKILL/references/prime/rituals/kickoff.md" "process.cwd()" \
  "worktree lifecycle: kickoff explains peer cwd derivation"
require_marker "$SKILL/references/prime/rituals/kickoff.md" "/pij prime" \
  "worktree lifecycle: kickoff enters module-first"
require_marker "$SKILL/references/prime/rituals/kickoff.md" "PR merge" \
  "worktree lifecycle: kickoff waits for PR merge"
require_marker "$SKILL/references/prime/rituals/kickoff.md" "explicit abandonment" \
  "worktree lifecycle: kickoff permits ruled abandonment"
require_marker "$SKILL/references/prime/templates/stream-brief.md" "**Worktree**" \
  "worktree lifecycle: brief records worktree"
require_marker "$SKILL/references/prime/templates/stream-brief.md" "**Branch**" \
  "worktree lifecycle: brief records branch"
require_marker "$SKILL/references/prime/templates/stream-brief.md" "**Base**" \
  "worktree lifecycle: brief records base"
require_marker "$SKILL/references/prime/templates/stream-brief.md" "/builder 8 ship" \
  "worktree lifecycle: brief records ship seam"
require_marker "$SKILL/references/prime/templates/spine.md" "Worktree" \
  "worktree lifecycle: spine roster records worktree"
require_marker "$SKILL/references/prime/templates/spine.md" "Branch" \
  "worktree lifecycle: spine roster records branch"
require_marker "$SKILL/references/prime/templates/spine.md" "Base" \
  "worktree lifecycle: spine allocation records base"
require_marker "$SKILL/references/prime/templates/orient-local.md" "Worktree root" \
  "worktree lifecycle: local orient derives root"
require_marker "$SKILL/references/prime/templates/orient-local.md" "Worktree naming" \
  "worktree lifecycle: local orient derives naming"
require_marker "$SKILL/references/prime/templates/orient-local.md" "Base branch" \
  "worktree lifecycle: local orient derives base"
require_marker "$SKILL/references/prime/templates/orient-local.md" "Landing policy" \
  "worktree lifecycle: local orient derives landing"
require_marker "$SKILL/references/prime/orient-oprime.md" "worktree and branch" \
  "worktree lifecycle: o-prime defaults to isolated construction"
require_marker "$SKILL/references/prime/protocol.md" "worktree-primary" \
  "worktree lifecycle: protocol names primary construction"
require_marker "$SKILL/references/prime/protocol.md" "/builder 8 ship" \
  "worktree lifecycle: protocol names Builder ship"
require_marker "$SKILL/references/prime/protocol.md" "shared-tree fallback" \
  "worktree lifecycle: protocol preserves fallback"
require_marker "$SKILL/references/prime/rituals/batons.md" "shared-tree fallback" \
  "worktree lifecycle: batons narrow shared-tree serialization"
require_marker "$SKILL/references/prime/rituals/batons.md" "timing" \
  "worktree lifecycle: batons preserve timing purity"
require_marker "$SKILL/references/prime/rituals/incidents.md" "shared-tree fallback" \
  "worktree lifecycle: incidents preserve INC-004 as fallback evidence"
require_marker "docs/how/pij-prime.md" "prime/orchestrator.md" \
  "prime guide: links orchestrator landing"
require_marker "docs/how/pij-prime.md" "/builder 8 ship" \
  "prime guide: summarizes Builder ship"
require_marker "docs/domains/pij-skill/domain.md" "Stream Orchestrator Landing" \
  "pij-skill domain: names orchestrator landing concept"

# 10b. question doctrine: ask_user_question may appear in the prime payload and
# root skill only inside ban language (never/No …) — regression guard on the
# modal-question ban; and the 2IC mission line must survive edits.
modal_hits=$(grep -rn 'ask_user_question' "$SKILL/SKILL.md" \
  "$SKILL/references/prime" "$SKILL/references/routes/prime.md" 2>/dev/null \
  | grep -Ev 'never|Never|No ' || true)
[ -z "$modal_hits" ] \
  && ok "question doctrine: ask_user_question appears only inside ban language" \
  || err "question doctrine: unguarded ask_user_question reference: $modal_hits"
require_marker "$SKILL/references/prime/orient-oprime.md" \
  "You are the human's 2IC" \
  "prime posture: 2IC mission line present"
require_marker "$SKILL/references/prime/templates/spine.md" "Pending decisions" \
  "question doctrine: spine template carries the pending-decision queue"

# 12. runtime payload must stand alone after the transitional source repo disappears.
severance_paths=("$SKILL")
[ -f docs/how/pij-prime.md ] && severance_paths+=(docs/how/pij-prime.md)
# local-path-check: allow -- literal is the forbidden transitional source path.
leaks=$(grep -rIl '/Users/jordanknight/games/SecondCrack' "${severance_paths[@]}" 2>/dev/null || true)
[ -n "$leaks" ] && err "prime portability: transitional SecondCrack path leaked into runtime docs: $leaks" \
                  || ok "prime portability: transitional source path severed"

if [ "$FAIL" -eq 0 ]; then echo "✅ pij-skill-check: all green"; else echo "❌ pij-skill-check failed"; fi
exit $FAIL
