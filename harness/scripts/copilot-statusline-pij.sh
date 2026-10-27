#!/bin/sh
# Copilot CLI status line: context % • folder/branch/pij • model • effort • used/size • usage
# Managed by pij (plan 156): the daemon installs it at ~/.pij-rs/copilot-statusline-pij.sh
# and points ~/.copilot/settings.json "statusLine" at it, backing up the prior script.
#
# Plan 180: rendered by `pij-rs statusline copilot`, in this one process (exec, no
# fork). The plan-156 bash version launched ~20 processes per refresh, and
# Defender's EDR inspects every launch. The output bytes are the same: golden tests
# run the preserved plan-156 script (crates/cli/tests/fixtures/) on the same inputs.
# The pane is asked on every render, so a dead seat shows dead on the next refresh.
command -v pij-rs >/dev/null 2>&1 || { printf '\033[2m⛓ pij-rs not on PATH\033[0m'; exit 0; }
exec pij-rs statusline copilot
