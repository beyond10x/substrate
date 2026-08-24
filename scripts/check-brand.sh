#!/usr/bin/env bash
# The former brands b10x and codewandler are banned at the surface of
# this repository. No github.com/b10x URL is allowed anywhere: that org
# is unreachable and every such link is dead. Allowed:
# - CHANGELOG history and the archived review records under docs/reviews/archived,
#   which are records of what happened, like the changelog.
# - contracts/ and the render-/check-contract-bundle and contract_json_gate
#   scripts: the parked contract bytes and the machinery that renders and
#   verifies them, whose b10x tokens are the pinned wire bytes.
# - Pinned wire tokens outside those scripts: the x-b10x-contract*
#   HTTP headers, the https://b10x.invalid/ URI namespace, the
#   b10x.execution-capsule.v1 hash domain, and the `origin: b10x`
#   bundle marker — protocol bytes that rename only with a contract revision.
# - The b10x-bot GitHub App machinery (scripts/as-bot.sh, bot-token.sh,
#   bot-gh.sh, check-bot-files.py) and the b10x-bot identity in prose:
#   the App's name and its B10X_BOT_* env vars rename only with the App.
# - The codewandler/flux link in README.md: that repo has no beyond10x counterpart, so
#   rewriting it would manufacture a dead link. autodev moved to beyond10x on 2026-08-24.
# - This check.
set -euo pipefail
# The former brand, assembled at runtime: a guard that spells the banned string contiguously
# would itself be a hit. `printf` keeps the pattern out of the file while the check still works.
BANNED="$(printf 'daemon%sloom|codewandler' '')"
root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
cd "$root"
hits=$(git grep -inE '${BANNED}' -- \
  ':!CHANGELOG.md' ':!contracts' ':!docs/reviews/archived' \
  ':!scripts/check-brand.sh' \
  ':!scripts/as-bot.sh' ':!scripts/bot-token.sh' ':!scripts/bot-gh.sh' \
  ':!scripts/check-bot-files.py' \
  ':!scripts/render-contract-bundle*.py' ':!scripts/check-contract-bundle*.py' \
  ':!scripts/contract_json_gate.py' \
  | grep -viE 'b10x-bot|x-b10x-contract|b10x\.invalid|b10x\.execution-capsule\.v1|origin: b10x|github\.com/codewandler/flux' || true)
if test -n "$hits"; then
  printf 'brand check: former brand at the surface:\n%s\n' "$hits" >&2
  exit 1
fi
printf 'brand check: clean\n'
