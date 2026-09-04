---
format: aep.planning-md/1
id: review-result:review-2026-09-03-acceptance-round-2
kind: review-result
status: active
title: Acceptance critic (aep-planning:plan-critic-acceptance), round 2
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
needs-revision

story:daemon-image-serves-exec-or-says-it-cannot — the revision fixed the either/or's image-contents-plus-delegated-lane branch (moved to Notes, now conditional framing only), but the acceptance still joins two independently-updatable artifacts with "and" ("The GHCR release notes and `README.md` state whether..."), so a revision can update one and not the other and neither party can tell from the sentence alone which failed — .engineering/planning/story/daemon-image-serves-exec-or-says-it-cannot.md:36

What I read: the epic and all 14 decomposing stories (15 artifacts total) via `aep artifact list --format json` filtered on the `decomposes` relation, `aep artifact show <id>` for each of the 15, `aep artifact show review-result:review-2026-09-03-acceptance-round-1`, `aep artifact lifecycle epic`, and `grep -n` on the store files for line citations. I also read the three other epics with a "Done When" section (`epic:byte-plane-completion`, `epic:container-driver-entry`, `epic:release-hardening`) to check this store's convention for epic-level acceptance before judging the new "## Done When" section.

What I could not establish: whether this store's epic "Done When" convention (a compound, multi-clause statement — seen in all three precedent epics, including `epic:release-hardening` which reached `implemented` with five semicolon-joined clauses) is meant to be exempt from the one-statement rule this critic applies to stories, or whether all four epics share the same latent defect. I treated the new epic's two-clause Done When ("every story implemented/rejected with a named closure" and "`aep artifact validate` reports the set valid") as consistent with established convention and did not flag it, but flag the uncertainty here rather than silently deciding it. Six of the seven round-1-fixed stories (`backend-recheck-hashes-only-on-change`, `confined-processes-cannot-nest-user-namespaces`, `exec-oom-kills-the-whole-tree`, `hosted-admission-reuses-identity-authority`, `metrics-streams-are-bounded`, `session-attachment-lifetime-is-an-accepted-decision`) now read as single, observable statements with the removed clauses correctly relocated to Notes, and carry no finding from me. Coupling, coverage of the epic's 14-finding register, and shared-surface risk are outside my lane and not reflected in this verdict.

```findings
- file: .engineering/planning/story/daemon-image-serves-exec-or-says-it-cannot.md
  line: 36
  category: acceptance
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: the revision fixed the either/or's image-contents-plus-delegated-lane branch (moved to Notes, now conditional framing only), but the acceptance still joins two independently-updatable artifacts with "and" ("The GHCR release notes and README.md state whether..."), so a revision can update one and not the other and neither party can tell from the sentence alone which failed
```
