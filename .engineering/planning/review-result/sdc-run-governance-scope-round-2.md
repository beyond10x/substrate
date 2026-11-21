---
format: aep.planning-md/3
id: review-result:sdc-run-governance-scope-round-2
kind: review-result
status: active
title: 'Plan critic (scope), round 2: runs are governed and limit hits are events'
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
- reviews: story:exec-observation-reports-process-count-hits
- reviews: epic:runs-are-governed-and-limit-hits-are-events
revision: 1
---
needs-revision

epic:runs-are-governed-and-limit-hits-are-events — the Done-when promises "a consumer-written test in this repository runs one profile that names an executable allow-list, a deny-list and process bounds on the `linux` adapter in the delegated lane, sees a denied execution fail inside the run, and reads the limit that ended the run and the process-count hits on the run's observation and on its `run_ended` record", and no story claims that single combined test; the acceptances of `story:linux-executable-allow-list`, `story:executable-deny-list`, `story:run-observation-names-the-limit-that-ended-it` and `story:process-count-limit-hit-is-observed` each run one feature on its own profile, so `story:process-count-limit-hit-is-observed`, the last story of the first slice, should take the combined consumer-written case in its Acceptance, or the Done-when should say the per-story cases suffice — .engineering/planning/epic/runs-are-governed-and-limit-hits-are-events.md:124-127

**What you read:** 12 artifacts: the epic, the 9 drafted stories and the 2 existing drafts (`story:inbound-endpoint-for-a-confined-exec`, `story:sdk-confinement-requires`). I ran `aep plan artifact show` on the epic, `aep plan artifact graph` for existing edges, and read the story files and the round-1 scope review. I extracted 12 promises from the epic and traced 11 to an item.

Promises and where each is claimed:
- Allow-list: `story:linux-executable-allow-list`.
- Deny-list: `story:executable-deny-list`.
- Memory, CPU, wall-time and process-count enforcement, and egress: already in place, so no item is needed.
- Ingress: `story:inbound-endpoint-for-a-confined-exec` for the wire and host driver, `story:confinement-profile-ingress-routes` for the in-process field.
- The limit named on the observation: `story:run-observation-names-the-limit-that-ended-it`.
- A record for every ended run: `story:audit-records-run-end-and-limit`.
- A record for every process-count hit: `story:process-count-limit-hit-is-observed`. It arrives on `run_ended`, which matches the epic's out-of-scope line on records written while a run is still running.
- On the wire: `story:exec-start-carries-executable-lists` and `story:exec-observation-reports-process-count-hits`, and the existing host-driver limit names.
- `oci` and `pod`: `story:executable-lists-on-oci-and-pod`.
- Gap 8: `story:sdk-confinement-requires`.
- The combined consumer-written Done-when test: unclaimed, the finding above.
- "Every story in § Order is implemented": the stories themselves.

The round-1 finding (a record for a denied executable) is resolved. The epic now lists it under § Out of scope and the Done-when no longer promises it.

No story reaches beyond the epic, none lands in its exclusions, and no two stories claim the same outcome. `story:executable-lists-on-oci-and-pod` narrows enforcement to a recorded observation plus a refusal scenario. Its own Boundaries state this ("No enforcement code for `oci` or `pod`") and it files follow-up stories, so I did not count it as a silent narrowing.

**What you could not establish:**
- Whether the epic's Done-when integration test was meant to be a separate artifact or an extra acceptance on an existing story. I name `story:process-count-limit-hit-is-observed` as the natural home and leave the choice to the revision.
- Out of my lane, and not counted in the verdict: whether the `depends_on` edges match the § Order claim that every story depends on the one before it (design and parallel-safety critics). `story:executable-lists-on-oci-and-pod` and `story:exec-start-carries-executable-lists` each state a dependency in prose, and I did not check the graph edges for them.

```findings
- file: .engineering/planning/epic/runs-are-governed-and-limit-hits-are-events.md
  line: 124
  category: scope
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "The Done-when promises one consumer-written test that runs a profile with an allow-list, a deny-list and process bounds on linux, sees a denied execution fail, and reads the limit that ended the run and the process-count hits on the observation and the run_ended record, and no story claims that combined test; story:process-count-limit-hit-is-observed would most naturally take it in its Acceptance, or the Done-when should say the per-story cases suffice"
```
