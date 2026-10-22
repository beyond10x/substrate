---
format: aep.planning-md/3
id: approval-record:gates-policy-substrate-021-jwt-fixtures
kind: approval-record
status: active
title: Admit the 0.21.0 delegated-context JWT vectors in gates-policy
revision: 3
transitions:
- {from: "draft", to: "active", at: "2026-10-22T07:23:59Z", actor: "agent:b10x-chef-orchestrator", revision: 3}
---
## Decision

Admit the 16 delegated-context JWT vectors of wire bundle 0.21.0 in gates-policy, in the form of
gates-policy `77d7b73` (the 0.20.0 admission).

Approved by session `b10x-chef-orchestrator` under the grant in the operator's workspace
instructions, quoted: "Go: a bot commit to gates-policy main that adds exactly the 16 exceptions
for the 0.21.0 JWT vectors, in the same form as 77d7b73 (bot|bot, read by me). Class C under the
grant ("Gates baseline move for findings the message names")."

## Done

- gates-policy `4c4047d` on `main`, author and committer `b10x-bot[bot]`: 112 insertions, 0
  deletions.
- Each of the 16 files is byte-identical to its 0.20.0 counterpart (`cmp`), so every entry has the
  same `line` and `content_sha256` as one already admitted.
- `.gitleaks.toml` in this repository is unchanged; its path pattern already covers every bundle
  version.

## Open

The `B10X_GATES_POLICY` organization secret is refreshed by its owner, not by the bot. Until then
CI's `common / Security and privacy` reads the old policy.
