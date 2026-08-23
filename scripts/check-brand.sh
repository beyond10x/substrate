#!/usr/bin/env bash
# The daemonloom string is banned at the surface of this repository. Allowed:
# - CHANGELOG history and the archived review records under docs/reviews/archived,
#   which are records of what happened, like the changelog.
# - Pinned provenance URLs (github.com/daemonloom/...) and the phrase
#   "the daemonloom monorepo" in extraction-provenance prose.
# - contracts/ and the render-/check-contract-bundle and contract_json_gate
#   scripts: the parked contract bytes and the machinery that renders and
#   verifies them, whose daemonloom tokens are the pinned wire bytes.
# - Pinned wire tokens outside those scripts: the x-daemonloom-contract*
#   HTTP headers, the https://daemonloom.invalid/ URI namespace, the
#   daemonloom.execution-capsule.v1 hash domain, and the `origin: daemonloom`
#   bundle marker — protocol bytes that rename only with a contract revision.
# - The daemonloom-bot GitHub App machinery (scripts/as-bot.sh, bot-token.sh,
#   bot-gh.sh, check-bot-files.py) and the daemonloom-bot identity in prose:
#   the App's name and its DAEMONLOOM_BOT_* env vars rename only with the App.
# - This check.
set -euo pipefail
root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
cd "$root"
hits=$(git grep -in 'daemonloom' -- \
  ':!CHANGELOG.md' ':!contracts' ':!docs/reviews/archived' \
  ':!scripts/check-brand.sh' \
  ':!scripts/as-bot.sh' ':!scripts/bot-token.sh' ':!scripts/bot-gh.sh' \
  ':!scripts/check-bot-files.py' \
  ':!scripts/render-contract-bundle*.py' ':!scripts/check-contract-bundle*.py' \
  ':!scripts/contract_json_gate.py' \
  | grep -viE 'github\.com/daemonloom|the daemonloom monorepo|daemonloom-bot|x-daemonloom-contract|daemonloom\.invalid|daemonloom\.execution-capsule\.v1|origin: daemonloom' || true)
if test -n "$hits"; then
  printf 'brand check: daemonloom at the surface:\n%s\n' "$hits" >&2
  exit 1
fi
printf 'brand check: clean\n'
