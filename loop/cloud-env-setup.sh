#!/bin/bash
src=$(mktemp -d) && git clone -q --depth 1 https://github.com/swiftugandan/openbim "$src" && cd "$src" && ORACLES_PREFIX=/opt/oracles bash loop/setup.sh && touch /opt/oracles/ready || echo "oracle setup did not finish; the SessionStart hook will retry"
exit 0
# The Setup script of the openbim-build cloud environment (CLOUD_ENV): the lines above are its exact text at claude.ai/code.
# That environment is for this project only; do not put this script in one shared with other projects.
# It fetches its own copy of this repo because it is not documented to run inside the session's clone.
# It never fails a session: loop/cloud-session-start.sh installs the oracles if this did not.
