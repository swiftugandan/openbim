#!/usr/bin/env bash
# Keeps cloud build sessions from waiting for a person. It does nothing outside a cloud session,
# so local sessions in this repo keep their normal permission prompts and questions.
#   PermissionRequest: approve the action a session would otherwise stop and wait on.
#   PreToolUse (AskUserQuestion): refuse the question and tell the session to decide for itself.
[ "${CLAUDE_CODE_REMOTE:-}" = "true" ] || exit 0

event=$(python3 -c 'import json,sys; print(json.load(sys.stdin).get("hook_event_name",""))')
case "$event" in
  PermissionRequest)
    printf '%s\n' '{"hookSpecificOutput":{"hookEventName":"PermissionRequest","decision":{"behavior":"allow"}}}'
    ;;
  PreToolUse)
    printf '%s\n' '{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"deny","permissionDecisionReason":"This is an unattended build and nobody will answer. Make the most reasonable decision, record it, and continue."}}'
    ;;
esac
