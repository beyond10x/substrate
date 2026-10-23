---
format: aep.planning-md/3
id: story:check-secrets-ignores-a-local-graft-file
kind: story
status: implemented
title: check-secrets ignores a local graft file
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-10-23T08:28:31Z", actor: "agent:b10x-chef-orchestrator", revision: 2}
- {from: "proposed", to: "active", at: "2026-10-23T08:28:32Z", actor: "agent:b10x-chef-orchestrator", revision: 3}
- {from: "active", to: "implemented", at: "2026-10-23T14:58:54Z", actor: "agent:b10x-chef-orchestrator", revision: 4, decided_on: {"recorded":{"test_result":1,"review_outcome":1,"verification":2}}}
---
# Story: check-secrets ignores a local graft file

## Context

Found by the 2026-10-23 wave (unit u1, adversary pass 2, open item). `cargo xtask check-secrets`
runs every Git process and Gitleaks with `GIT_NO_REPLACE_OBJECTS=1` (`xtask/src/secrets.rs:18`), so a
replace ref cannot hide history from the scan. A `.git/info/grafts` file is not neutralised by that
variable: a graft can cut a commit's parents, so both the expected count and Gitleaks' own log stop
at the graft and the commits behind it go unscanned while the counts still agree. Local-only: grafts
are never pushed, so CI is unaffected.

## Acceptance

With a graft file that hides a commit carrying a credential, `cargo xtask check-secrets` either
scans that commit and refuses it, or refuses to run, naming the graft file; a test builds that
history in a temporary repository.
