#!/bin/sh
# canary B: force the TUI onto the shared Codex app-server daemon (daemon-hosted mode seen on 2026-10-26)
exec /Users/jordanknight/.codex/packages/standalone/releases/0.164.0-aarch64-apple-darwin/bin/codex --remote unix:// "$@"
