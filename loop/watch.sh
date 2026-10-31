#!/usr/bin/env bash
# Dashboard: follows the newest loop session log and redraws in place.
# Read-only, so Ctrl-C quits it without touching the loop. Optional arg: a session .jsonl to view.
exec python3 "$(dirname "$0")/watch.py" "$@"
