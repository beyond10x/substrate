---
format: aep.planning-md/1
id: epic:review-2026-09-03-findings
kind: epic
status: draft
title: Close the 2026-09-03 in-depth review findings
summary: 'Fourteen review findings: two correctness defects, three authenticated-client capacity gaps, three confinement hardening gaps, three design constraints to record, three minor costs.'
owner: substrate
tags:
- hardening
- review
revision: 2
---
# Epic: Close the 2026-09-03 in-depth review findings

## Outcome

Every finding of the 2026-09-03 whole-repository review (`~/.cache/substrate-review/2026-09-03-substrate-in-depth-review.md`) is either closed by a change that a
test or probe proves, or recorded as an accepted decision. Two correctness defects, three
authenticated-client capacity gaps, three confinement hardening gaps, three design constraints
and three minor costs; the review's own register carries the file and line for each.

## Why Now

The review read every file under `crates/` at `cc5671c` and ran both gates green
(`scripts/gate.sh`: 491 tests; `scripts/delegated-lane.sh`: 96 tests), so the findings are
defects the suite does not see. Finding 1 was reproduced at runtime: the same two line-patch edits
produce two different files depending on their order in the request. Findings 3 to 5 are reachable
by any authenticated local uid. The 0.5.1 release on `origin/main` changes none of the cited code
except `crates/substrate-host/src/fs.rs` executable-mode preservation.

## Scope

One story per finding, in the register's order. Correctness first, then capacity, then hardening,
then the three decisions, then the minor costs:

1. `story:line-patch-edits-apply-in-line-order`
2. `story:directory-listing-survives-special-entries`
3. `story:metrics-streams-are-bounded`
4. `story:upgraded-connections-keep-their-permit`
5. `story:lease-cleanup-reads-exec-state-only`
6. `story:confined-processes-cannot-nest-user-namespaces`
7. `story:seccomp-denies-af-vsock`
8. `story:exec-oom-kills-the-whole-tree`
9. `story:session-attachment-lifetime-is-an-accepted-decision`
10. `story:hosted-admission-reuses-identity-authority`
11. `story:daemon-image-serves-exec-or-says-it-cannot`
12. `story:backend-recheck-hashes-only-on-change`
13. `story:events-table-has-one-index`
14. `story:unattached-claimed-session-is-contained`

## Out of Scope

Any change to a frozen contract bundle. Where a story needs a wire change (story 2 may), it cuts a
successor bundle under invariant 6 and says so in its own body. Product policy, fleet scheduling
and the remote-serving track stay where they are.

## Risks

- Stories 6 and 7 tighten confinement; a host whose bubblewrap or kernel refuses the new flag must
  answer a named refusal, never a quieter sandbox (invariant 3).
- Story 4 rests on hyper 1.11.1 upgrade semantics inferred from the library, not observed; the
  story's first step is a test that observes it.
- Stories 9 to 11 may end as recorded decisions rather than code; that is a valid close.

## Done When

Every one of the 14 stories in Scope is `implemented` or `rejected` with a body that names the test, probe or ADR that closed it, and `aep artifact validate` reports the set valid.
