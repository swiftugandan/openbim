#!/usr/bin/env bash
# SessionStart hook. In a cloud session, point .oracles at the copy in the environment's snapshot
# (see loop/cloud-env-setup.sh), installing it first if this environment has none yet. Does nothing locally.
[ "${CLAUDE_CODE_REMOTE:-}" = "true" ] || exit 0
cd "$CLAUDE_PROJECT_DIR" || exit 0
if [ ! -e /opt/oracles/ready ]; then
  ORACLES_PREFIX=/opt/oracles bash loop/setup.sh > /tmp/oracles-setup.log 2>&1 \
    && touch /opt/oracles/ready \
    || echo "Oracle setup failed; see /tmp/oracles-setup.log. Judges that need oracles will fail."
fi
ln -sfn /opt/oracles .oracles
