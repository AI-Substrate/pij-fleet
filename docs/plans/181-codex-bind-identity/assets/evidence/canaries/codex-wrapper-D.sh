#!/bin/sh
# canary D: daemon-hosted (where the update menu appeared) + check_for_update_on_startup=false
exec /Users/jordanknight/.codex/packages/standalone/releases/0.164.0-aarch64-apple-darwin/bin/codex --remote unix:// -c check_for_update_on_startup=false "$@"
