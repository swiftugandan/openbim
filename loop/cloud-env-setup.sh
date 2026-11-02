#!/bin/bash
# The cloud environment's Setup script (paste this text into its dialog at claude.ai/code).
# Its result is snapshotted, whichever repo a session starts on, so it fetches its own copy of this repo.
# It never fails the session: loop/cloud-session-start.sh installs the oracles if this did not.
(
  set -e
  start=$SECONDS
  src=$(mktemp -d)
  git clone -q --depth 1 https://github.com/swiftugandan/openbim "$src"
  cd "$src"
  ORACLES_PREFIX=/opt/oracles bash loop/setup.sh
  touch /opt/oracles/ready
  echo "oracles installed in $((SECONDS - start))s"
) || echo "oracle setup did not finish; the SessionStart hook will retry"
exit 0
