---
format: aep.planning-md/1
id: review-result:review-2026-09-03-acceptance-round-1
kind: review-result
status: active
title: Acceptance critic (aep-planning:plan-critic-acceptance), round 1
tags:
- critic
- review
relations:
- reviews: epic:review-2026-09-03-findings
- reviews: story:line-patch-edits-apply-in-line-order
- reviews: story:directory-listing-survives-special-entries
- reviews: story:metrics-streams-are-bounded
- reviews: story:upgraded-connections-keep-their-permit
- reviews: story:lease-cleanup-reads-exec-state-only
- reviews: story:confined-processes-cannot-nest-user-namespaces
- reviews: story:seccomp-denies-af-vsock
- reviews: story:exec-oom-kills-the-whole-tree
- reviews: story:session-attachment-lifetime-is-an-accepted-decision
- reviews: story:hosted-admission-reuses-identity-authority
- reviews: story:daemon-image-serves-exec-or-says-it-cannot
- reviews: story:backend-recheck-hashes-only-on-change
- reviews: story:events-table-has-one-index
- reviews: story:unattached-claimed-session-is-contained
revision: 1
---
approve
needs-revision

epic:review-2026-09-03-findings — the epic carries no Acceptance or Done-When section (only Outcome, Why Now, Scope, Out of Scope, Risks), so there is no epic-level statement that closes independently of "all 14 stories implemented" — .engineering/planning/epic/review-2026-09-03-findings.md (headings at lines 16, 23, 32, 52, 58; no Acceptance heading)

story:backend-recheck-hashes-only-on-change — the acceptance joins two independent scenarios with "while" (unchanged-metadata skips the read; a replaced binary is still refused), so one can pass while the other fails — .engineering/planning/story/backend-recheck-hashes-only-on-change.md:30

story:confined-processes-cannot-nest-user-namespaces — the acceptance conjoins three separately-failable claims (unshare -U fails on a modern host, the probe passes the new flag, exec facts are withheld on a host that lacks it), and "observes both" names only two of the three it just listed — .engineering/planning/story/confined-processes-cannot-nest-user-namespaces.md:37

story:daemon-image-serves-exec-or-says-it-cannot — each side of the either/or bundles two independent claims (image contents plus the delegated lane running against it; three documents stating the limit plus naming what to add), so a revision can satisfy one half of a branch and not the other — .engineering/planning/story/daemon-image-serves-exec-or-says-it-cannot.md:36

story:exec-oom-kills-the-whole-tree — the acceptance joins the whole-cgroup kill and the refusal-code label with "and", and the story's own Context says the label is derived from a different code path (`record_resource_bound`, gated on `measurements`) than the kill mechanism (`memory.oom.group`), so the two are independently failable — .engineering/planning/story/exec-oom-kills-the-whole-tree.md:34

story:hosted-admission-reuses-identity-authority — each side of the either/or bundles multiple independent claims (at-most-one resolution, bounded revocation latency, and a capability-document statement on one side; an ADR and a deployment-guide cost statement on the other) — .engineering/planning/story/hosted-admission-reuses-identity-authority.md:33

story:metrics-streams-are-bounded — the acceptance names three independent mechanisms (per-subject cap/429, lifetime expiry, oversized-message close) and says "a test proves all three", so any one of the three can regress without the others noticing — .engineering/planning/story/metrics-streams-are-bounded.md:33

story:session-attachment-lifetime-is-an-accepted-decision — past the either/or (ADR vs. filed story), a third, unconditional clause ("the design doc and the published capability document agree with the code") is joined by a semicolon as an independent claim — .engineering/planning/story/session-attachment-lifetime-is-an-accepted-decision.md:32

What I read: the epic and all 14 decomposing stories, 15 artifacts total, via `aep artifact list --format json`, `aep artifact show <id>` for each of the 15, `aep artifact kinds`, `aep artifact lifecycle epic`, `aep artifact lifecycle story`, plus `grep -n` on the store files for line citations. I also read two other draft/proposed epics (`epic:container-driver-entry`, `epic:mcp-test-surface`) and two implemented ones to check this store's epic-acceptance convention before flagging the epic.

What I could not establish: whether this store treats an epic's Scope (a numbered list of its stories) as an intentional substitute for a Done-When/Acceptance section — other epics in this store are inconsistent (`epic:release-hardening` and `epic:container-driver-entry` carry "Done When"; `epic:mcp-test-surface` carries neither), so I flagged the gap rather than assuming it is accepted convention. Six stories (`directory-listing-survives-special-entries`, `events-table-has-one-index`, `lease-cleanup-reads-exec-state-only`, `line-patch-edits-apply-in-line-order`, `seccomp-denies-af-vsock`, `unattached-claimed-session-is-contained`, `upgraded-connections-keep-their-permit`) read as single, checkable statements and carry no finding from me. Coupling between stories, coverage of the epic's 14-finding register, and any shared-surface risk from stories 6/7's confinement tightening are outside my lane and not reflected in this verdict.

```findings
- file: .engineering/planning/epic/review-2026-09-03-findings.md
  line: 16
  category: acceptance
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: the epic carries no Acceptance or Done-When section, so there is no epic-level statement that closes independently of "all 14 stories implemented"
- file: .engineering/planning/story/backend-recheck-hashes-only-on-change.md
  line: 30
  category: acceptance
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: the acceptance joins two independent scenarios with "while" (unchanged-metadata skips the read; a replaced binary is still refused), so one can pass while the other fails
- file: .engineering/planning/story/confined-processes-cannot-nest-user-namespaces.md
  line: 37
  category: acceptance
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: the acceptance conjoins three separately-failable claims (unshare -U fails, the probe passes the new flag, exec facts are withheld on an unsupporting host), and "observes both" names only two of the three it just listed
- file: .engineering/planning/story/daemon-image-serves-exec-or-says-it-cannot.md
  line: 36
  category: acceptance
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: each side of the either/or bundles two independent claims (image contents plus the delegated lane running against it; three documents stating the limit plus naming what to add), so a revision can satisfy one half of a branch and not the other
- file: .engineering/planning/story/exec-oom-kills-the-whole-tree.md
  line: 34
  category: acceptance
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: the acceptance joins the whole-cgroup kill and the refusal-code label with "and", and the story's own Context says the label is derived from a different code path than the kill mechanism, so the two are independently failable
- file: .engineering/planning/story/hosted-admission-reuses-identity-authority.md
  line: 33
  category: acceptance
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: each side of the either/or bundles multiple independent claims (at-most-one resolution, bounded revocation latency and a capability-document statement on one side; an ADR and a deployment-guide cost statement on the other)
- file: .engineering/planning/story/metrics-streams-are-bounded.md
  line: 33
  category: acceptance
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: the acceptance names three independent mechanisms (per-subject cap/429, lifetime expiry, oversized-message close) and says "a test proves all three", so any one of the three can regress without the others noticing
- file: .engineering/planning/story/session-attachment-lifetime-is-an-accepted-decision.md
  line: 32
  category: acceptance
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: past the either/or (ADR vs. filed story), a third, unconditional clause ("the design doc and the published capability document agree with the code") is joined by a semicolon as an independent claim
```
