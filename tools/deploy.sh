#!/usr/bin/env bash
# Copy the add-on folder at HEAD to the Home Assistant host's /local_apps as a local add-on; the
# Supervisor builds the image there. HA_SSH names the target (user@host); HA_SSH_KEY and
# HA_SSH_PORT are optional. Install or update with `ha apps` afterwards (README).
set -euo pipefail
: "${HA_SSH:?set HA_SSH to user@host of the Home Assistant host}"
ssh_args=(-o BatchMode=yes -p "${HA_SSH_PORT:-22}")
if [ -n "${HA_SSH_KEY:-}" ]; then
  ssh_args+=(-i "$HA_SSH_KEY")
fi
cd "$(git rev-parse --show-toplevel)"
git archive HEAD github_finds | ssh "${ssh_args[@]}" "$HA_SSH" \
  'rm -rf /local_apps/github_finds && tar -x -C /local_apps && ha store reload >/dev/null && echo "copied to /local_apps/github_finds"'
