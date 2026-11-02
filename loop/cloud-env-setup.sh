#!/bin/bash
src=$(mktemp -d) && git clone -q --depth 1 https://github.com/swiftugandan/openbim "$src" && cd "$src" && ORACLES_PREFIX=/opt/oracles bash loop/setup.sh && touch /opt/oracles/ready || echo "oracle setup did not finish; the SessionStart hook will retry"
exit 0
# The Setup script of the cloud environment named by CLOUD_ENV: the lines above are its exact text at claude.ai/code.
# The snapshot it builds serves any repo the environment is used with, so it fetches its own copy of this repo.
# It never fails a session: loop/cloud-session-start.sh installs the oracles if this did not.
