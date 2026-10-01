#!/usr/bin/env bash
# Build the static binary and copy the add-on, binary included, to the HA host's /local_apps.
# Run from Git Bash on desktop. Install or update with ha apps afterwards (README).
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
cargo zigbuild --release --target x86_64-unknown-linux-musl
stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT
git archive HEAD github_finds | tar -x -C "$stage"
cp target/x86_64-unknown-linux-musl/release/github-finds "$stage/github_finds/github-finds"
tar -C "$stage" -cf - github_finds | ssh -i ~/.ssh/ha_ed25519 -o BatchMode=yes root@192.0.2.10 'rm -rf /local_apps/github_finds && tar -x -C /local_apps && ha store reload >/dev/null && echo "copied to /local_apps/github_finds"'
