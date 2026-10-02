#!/bin/sh
# Claude Code status line: folder • model • effort • context % • used/size • 5h rate
# Managed by pij. Wire via settings.json:
#   statusLine: { type: "command", command: "~/.pij-rs/claude-statusline-pij.sh" }
IFS= read -r input || :

ALT_FMT=""
case "${CLAUDE_CONFIG_DIR:-}" in
  *.claude-alt*) ALT_FMT='\033[33malt\033[0m ' ;;
esac

COPILOT_FMT=""
if [ -n "${CC_COPILOT_ACTIVE:-}" ]; then
  COPILOT_FMT='\033[33mcopilot\033[0m '
else
  case "${ANTHROPIC_BASE_URL:-}" in
    *localhost:4141*|*127.0.0.1:4141*) COPILOT_FMT='\033[33mcopilot\033[0m ' ;;
  esac
fi

PARSED=$(printf '%s' "$input" | jq -r '
  (.context_window.used_percentage // 0) as $pct |
  (.context_window.context_window_size // 200000) as $size |
  "MODEL=\((.model.display_name // .model.id // "?") | @sh)\n" +
  "CWD=\((.workspace.current_dir // ".") | @sh)\n" +
  "TRANSCRIPT=\((.transcript_path // "") | @sh)\n" +
  "SESSION_ID=\((.session_id // "") | @sh)\n" +
  "EFFORT_INPUT=\((.effort_level // .effortLevel // (.effort | if type=="object" then .level elif type=="string" then . else "" end) // "") | @sh)\n" +
  "PCT_INT=\(($pct | round) | @sh)\n" +
  "USED=\((($size * $pct / 100) | floor) | @sh)\n" +
  "SIZE=\($size | @sh)\n" +
  "RL5=\((.rate_limits.five_hour.used_percentage // "") | @sh)\n" +
  "RESET=\((if .rate_limits.five_hour.resets_at then (.rate_limits.five_hour.resets_at | localtime | strftime("%H:%M")) else "" end) | @sh)"
' 2>/dev/null)
eval "$PARSED"
MODEL=${MODEL:-?}
CWD=${CWD:-.}
# The git walk below strips one path segment per step; a relative CWD (jq
# missing, malformed payload) would never reach "/" and loop forever.
case "$CWD" in
  /*) ;;
  *) CWD=$(cd "$CWD" 2>/dev/null && pwd -P) || CWD=${PWD:-/} ;;
esac
TRANSCRIPT=${TRANSCRIPT:-}
SESSION_ID=${SESSION_ID:-}
EFFORT_INPUT=${EFFORT_INPUT:-}
PCT_INT=${PCT_INT:-0}
USED=${USED:-0}
SIZE=${SIZE:-200000}
RL5=${RL5:-}
RESET=${RESET:-}
DIR=${CWD##*/}

BRANCH=""
PROBE=$CWD
GIT_DIR=""
while [ -n "$PROBE" ]; do
  if [ -d "$PROBE/.git" ]; then
    GIT_DIR=$PROBE/.git
    break
  fi
  if [ -f "$PROBE/.git" ]; then
    IFS= read -r GIT_LINK <"$PROBE/.git" || GIT_LINK=""
    GIT_DIR=${GIT_LINK#gitdir: }
    case "$GIT_DIR" in /*) ;; *) GIT_DIR=$PROBE/$GIT_DIR ;; esac
    break
  fi
  [ "$PROBE" = "/" ] && break
  case "$PROBE" in
    */*) PROBE=${PROBE%/*} ;;
    *) break ;;
  esac
  [ -n "$PROBE" ] || PROBE=/
done
if [ -n "$GIT_DIR" ] && IFS= read -r HEAD_VALUE <"$GIT_DIR/HEAD"; then
  case "$HEAD_VALUE" in
    "ref: refs/heads/"*) BRANCH=${HEAD_VALUE#ref: refs/heads/} ;;
    *) BRANCH=$(printf '%.7s' "$HEAD_VALUE") ;;
  esac
fi
BRANCH_FMT=""
[ -n "$BRANCH" ] && BRANCH_FMT=" \\033[32m⎇ $BRANCH\\033[0m"

# Resolve the pane against rs first. Only an rs miss may start the slower legacy CLI.
PIJID=""
PIJ_RS_BIN=$(command -v pij-rs 2>/dev/null || :)
[ -z "$PIJ_RS_BIN" ] && [ -x "${HOME:-}/.npm-global/bin/pij-rs" ] && PIJ_RS_BIN=${HOME:-}/.npm-global/bin/pij-rs
if [ -n "${TMUX_PANE:-}" ] && [ -n "$PIJ_RS_BIN" ]; then
  RS_OUTPUT=$(unset PIJ_SESSION_ID; "$PIJ_RS_BIN" --json whoami --pane "$TMUX_PANE" 2>/dev/null)
  case "$RS_OUTPUT" in
    *'"id":"'*)
      PIJID=${RS_OUTPUT#*'"id":"'}
      PIJID=${PIJID%%'"'*}
      ;;
  esac
fi
PIJ_FMT=""
if [ -n "$PIJID" ]; then
  PIJ_FMT=" \033[33mrs\033[0m \033[34m⛓ $PIJID\033[0m"
  # Held FYIs (plan 158): whoami carries the count; show it only while N > 0.
  case "$RS_OUTPUT" in
    *'"pending_fyis":'*)
      FYIS=${RS_OUTPUT#*'"pending_fyis":'}
      FYIS=${FYIS%%[!0-9]*}
      [ -n "$FYIS" ] && [ "$FYIS" -gt 0 ] && PIJ_FMT="$PIJ_FMT \033[33m✉$FYIS\033[0m"
      ;;
  esac
elif command -v pij >/dev/null 2>&1; then
  LEGACYID=$(PIJ_DAEMON_GENERATION=legacy pij whoami 2>/dev/null | sed -n 's/^pij session:[[:space:]]*\([^[:space:]]*\).*/\1/p')
  [ -z "$LEGACYID" ] && LEGACYID=$(PIJ_DAEMON_GENERATION=legacy pij whoami --json 2>/dev/null | jq -r '.id // empty' 2>/dev/null)
  [ -n "$LEGACYID" ] && PIJ_FMT=" \033[2mlegacy\033[0m \033[34m⛓ $LEGACYID\033[0m"
fi

# The heal loop (plan 156 rule 4). An rs miss (that legacy does not own either),
# or an rs seat recording a different conversation, fires ONE detached adopt per
# pane per 15 s. The daemon derives the conversation itself; the payload's
# session_id only cross-checks it. Rendering never waits on the adopt.
if [ -n "${TMUX_PANE:-}" ] && [ -n "$PIJ_RS_BIN" ] && [ -n "$SESSION_ID" ] \
  && { [ -n "$PIJID" ] || [ -z "${LEGACYID:-}" ]; }; then
  case "$RS_OUTPUT" in
    *"\"session\":\"$SESSION_ID\""*) ;;
    *)
      HEAL_DIR=${PIJ_RS_STATE_DIR:-${HOME:-/tmp}/.pij-rs}/heal
      HEAL_KEY=$HEAL_DIR/pane-${TMUX_PANE#%}
      mkdir -p "$HEAL_DIR" 2>/dev/null
      if mkdir "$HEAL_KEY.lock" 2>/dev/null; then
        NOW=$(date +%s)
        LAST=0
        [ -f "$HEAL_KEY.last" ] && IFS= read -r LAST <"$HEAL_KEY.last"
        case "$LAST" in ''|*[!0-9]*) LAST=0 ;; esac
        if [ $((NOW - LAST)) -ge 15 ]; then
          printf '%s\n' "$NOW" >"$HEAL_KEY.last"
          (
            if OUT=$("$PIJ_RS_BIN" adopt "$TMUX_PANE" --harness claude --harness-session "$SESSION_ID" 2>&1); then
              RESULT="healed"
            else
              RESULT="refused reason=$(printf '%s' "$OUT" | tr '\n' ' ')"
            fi
            printf '%s statusline:%s pane=%s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$RESULT" "$TMUX_PANE" \
              >>"${HEAL_DIR%/heal}/hook.log" 2>/dev/null
          ) </dev/null >/dev/null 2>&1 &
        fi
        rmdir "$HEAL_KEY.lock" 2>/dev/null
      fi
      PIJ_FMT="$PIJ_FMT \033[33m⛓ registering…\033[0m"
      ;;
  esac
fi

EFFORT=""
if [ -n "${CLAUDE_CODE_EFFORT_LEVEL:-}" ]; then
  EFFORT="$CLAUDE_CODE_EFFORT_LEVEL"
else
  EFFORT=$EFFORT_INPUT
  if [ -z "$EFFORT" ] && [ -n "$TRANSCRIPT" ] && [ -f "$TRANSCRIPT" ]; then
    EFFORT=$(jq -rc 'select(.type=="user"
                            and (.message.content // "" | type=="string")
                            and (.message.content | test("<command-name>/effort</command-name>")))
                     | .message.content
                     | capture("<command-args>(?<v>[^<]+)</command-args>")
                     | .v' "$TRANSCRIPT" 2>/dev/null | tail -1)
  fi
  if [ -z "$EFFORT" ]; then
    for f in "$CWD/.claude/settings.local.json" "$CWD/.claude/settings.json" "${HOME:-}/.claude/settings.json"; do
      if [ -f "$f" ]; then
        v=$(jq -r '.effortLevel // empty' "$f" 2>/dev/null)
        if [ -n "$v" ]; then EFFORT="$v"; break; fi
      fi
    done
  fi
fi
EFFORT=${EFFORT:-auto}
fmt() {
  n=$1
  if [ "$n" -ge 1000000 ]; then
    FMT="$((n / 1000000)).$(((n % 1000000) / 100000))M"
  elif [ "$n" -ge 10000 ]; then
    FMT="$((n / 1000))k"
  elif [ "$n" -ge 1000 ]; then
    FMT="$((n / 1000)).$(((n % 1000) / 100))k"
  else
    FMT=$n
  fi
}
fmt "$USED"
USED_FMT=$FMT
fmt "$SIZE"
SIZE_FMT=$FMT

RL5_FMT=""
if [ -n "$RL5" ]; then
  [ -n "$RESET" ] && RL5_FMT=" • \\033[2m${RL5%.*}% ↻$RESET\\033[0m" \
                  || RL5_FMT=" • \\033[2m${RL5%.*}%\\033[0m"
fi

printf '%b\033[2m%s%%\033[0m %b\033[36m%s\033[0m%b%b • %s • \033[35m%s\033[0m • \033[2m%s/%s\033[0m%b' \
  "$COPILOT_FMT" "$PCT_INT" "$ALT_FMT" "$DIR" "$BRANCH_FMT" "$PIJ_FMT" "$MODEL" "$EFFORT" "$USED_FMT" "$SIZE_FMT" "$RL5_FMT"
exit 0
