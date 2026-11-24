#!/usr/bin/env bash
# Run one gh command with a b10x-bot installation token.
#
# The token is minted before `gh` starts. A mint that fails, or that prints no token, ends this
# script non-zero and `gh` is not run: with no `GH_TOKEN`, `gh` acts under the login of the
# caller's keyring.
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
if ! bot_token="$("${repo_root}/scripts/bot-token.sh")"; then
  echo "bot-gh: scripts/bot-token.sh failed; gh is not run" >&2
  exit 1
fi
if [ -z "$bot_token" ]; then
  echo "bot-gh: scripts/bot-token.sh printed no token; gh is not run" >&2
  exit 1
fi

GH_TOKEN="$bot_token" exec gh "$@"
