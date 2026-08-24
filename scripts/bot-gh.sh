#!/usr/bin/env bash
# Run one gh command with a b10x-bot installation token.
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
GH_TOKEN="$(${repo_root}/scripts/bot-token.sh)" exec gh "$@"
