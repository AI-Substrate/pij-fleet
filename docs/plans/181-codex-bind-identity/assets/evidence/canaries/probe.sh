#!/bin/sh
# usage: probe.sh <label> [nosend] — run by a canary Codex seat from its own tool shell
OUT=/private/tmp/claude-501/-Users-jordanknight-pi-hacking-fleet-181-codex-bind/726e715b-0991-4322-ab03-b9dd3ab9972f/scratchpad/ev/$1/probe.txt
{
echo "== env"; env | grep -E '^(PIJ_|TMUX|CODEX_|CLAUDE_CODE_SESSION|COPILOT_AGENT)' | sort
echo "== ancestry"; p=$$; while [ "$p" -gt 1 ]; do ps -o pid=,ppid=,lstart=,command= -p $p | cut -c1-200; p=$(ps -o ppid= -p $p | tr -d ' '); done
echo "== whoami"; pij whoami --json 2>&1
if [ "${2:-}" != nosend ]; then echo "== send"; pij send pij-codex-bind "canary $1 probe: whoami above" --json 2>&1; fi
} > $OUT 2>&1
echo done
