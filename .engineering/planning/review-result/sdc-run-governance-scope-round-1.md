---
format: aep.planning-md/3
id: review-result:sdc-run-governance-scope-round-1
kind: review-result
status: active
title: 'Plan critic (scope), round 1: runs are governed and limit hits are events'
tags:
- plan-critic
- sdc
relations:
- reviews: story:linux-executable-allow-list
- reviews: story:executable-deny-list
- reviews: story:executable-lists-on-oci-and-pod
- reviews: story:exec-start-carries-executable-lists
- reviews: story:run-observation-names-the-limit-that-ended-it
- reviews: story:audit-records-run-end-and-limit
- reviews: story:process-count-limit-hit-is-observed
- reviews: story:confinement-profile-ingress-routes
- reviews: epic:runs-are-governed-and-limit-hits-are-events
revision: 1
---
needs-revision

epic:runs-are-governed-and-limit-hits-are-events — "reads a record for a denied executable" is a promised Done-when check and no story claims it; `story:executable-deny-list` ("No record of a denied execution attempt") and `story:audit-records-run-end-and-limit` ("No record for a denied execution or a denied connection") each exclude it, and the epic's § Out of scope does not. Either add the record to `story:audit-records-run-end-and-limit`, which would most naturally take it, or strike it from the Done-when with the decision recorded. — .engineering/planning/epic/runs-are-governed-and-limit-hits-are-events.md:98-101 (excluded at .engineering/planning/story/executable-deny-list.md:166-167 and .engineering/planning/story/audit-records-run-end-and-limit.md:511)

**What you read:** 11 artifacts (the epic, the 8 drafted stories and the 2 existing drafts). I ran `aep plan artifact show` on each and `aep plan artifact graph` for existing claims, then checked `process.rs:3275-3325` and the wire event union. I extracted 12 promises from the epic and traced 11 to an item.

Promises extracted and where each is claimed:
- Executable allow-list: `story:linux-executable-allow-list`.
- Deny-list: `story:executable-deny-list`.
- Allow-list and deny-list on `oci` and `pod`: `story:executable-lists-on-oci-and-pod`.
- Executable lists on the wire: `story:exec-start-carries-executable-lists`.
- Memory, CPU and wall-time limits: already in place, so no item is needed.
- Process-count limit: `story:process-count-limit-hit-is-observed` covers the hit.
- Egress: already in place.
- Ingress: `story:inbound-endpoint-for-a-confined-exec` and `story:confinement-profile-ingress-routes`.
- The limit that ended a run, on the observation: `story:run-observation-names-the-limit-that-ended-it`.
- A record for every limit hit: `story:audit-records-run-end-and-limit` and `story:process-count-limit-hit-is-observed`.
- SDK `requires` (gap 8): `story:sdk-confinement-requires`.
- A record for a denied executable: unclaimed (the finding above).

No story reaches beyond the epic: every item traces to a sentence in it, and none lands in its § Out of scope. No two stories claim the same outcome.

**What you could not establish:**
- I had no drafter's report, so I cannot tell whether the denied-execution exclusion was a deliberate decision or an oversight. I found no such decision in the epic.
- The `process-count-limit-hit-is-observed` outcome promises the wire observation unconditionally, while its acceptance and summary make the wire change optional ("if the design chooses it"). That inconsistency is for the acceptance and design critics and is out of my lane. I did not count it as a narrowing, because the body records the choice as deferred to the design.

```findings
- file: .engineering/planning/epic/runs-are-governed-and-limit-hits-are-events.md
  line: 98
  category: scope
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "The Done-when promises a consumer 'reads a record for a denied executable' and no story claims it; story:executable-deny-list and story:audit-records-run-end-and-limit both exclude a record of a denied execution, and the epic's Out of scope does not, so either story:audit-records-run-end-and-limit takes it or the Done-when drops it with the decision recorded"
```
