---
format: aep.planning-md/3
id: review-result:sdc-usage-and-file-changes-parallel-safety-round-2
kind: review-result
status: active
title: 'Plan critic (parallel-safety), round 2: usage on the observation and the file-change record'
tags:
- plan-critic
- sdc
relations:
- reviews: story:run-observation-carries-usage
- reviews: story:exec-file-changes-are-recorded
- reviews: story:exec-file-change-record-on-the-wire
revision: 1
---
needs-revision

story:exec-file-change-record-on-the-wire — it and story:executable-lists-on-oci-and-pod both list the directory `website/docs` (inferred on both sides, so no page is named), no `depends_on` path runs between them (the wire story reaches only `exec-file-changes-are-recorded` and `exec-observation-reports-process-count-hits`; the oci/pod story reaches only `executable-deny-list`, `process-count-limit-hit-is-observed` and `exec-file-changes-are-recorded`), and neither `## Order` section mentions the other; the fix is either an edge between the two recording `website/docs` as the reason, or narrowing both entries to the pages each edits (for example `website/docs/reference/contract.md`) so the shared path goes away — `.engineering/planning/story/exec-file-change-record-on-the-wire.md:115`, `.engineering/planning/story/exec-file-change-record-on-the-wire.md:134`, `.engineering/planning/story/executable-lists-on-oci-and-pod.md:31`, and `aep plan artifact waves --kind story` (`collision: story:exec-file-change-record-on-the-wire story:executable-lists-on-oci-and-pod website/docs (inferred)`)

What I read: 8 stories (the three under review and the five epic neighbours), with `aep plan artifact show` on each, `aep plan artifact list --format json` and `aep plan artifact waves --kind story`. I filtered the collisions to pairs with one of the three as a side and checked each against the transitive closure of the `depends_on` edges. That covers 22 pairs. 21 are ordered by an edge, direct or transitive. The claim holds for all except the pair above.

Surfaces placed: all three stories are placed. `story:run-observation-carries-usage` is wholly cited, with line numbers. `story:exec-file-changes-are-recorded` and `story:exec-file-change-record-on-the-wire` each have cited anchors, and the rest of their entries are inferred. None is unplaceable. Acceptance reads traced to a producer in the set: 5, all recorded by an edge.
- The wire story reads the record that `exec-file-changes-are-recorded` produces.
- `exec-file-changes-are-recorded` follows `run-observation-carries-usage`, as the `## Order` sections state.
- The wire story's bundle comes after the one `exec-observation-reports-process-count-hits` cuts.
- `run-observation-carries-usage` reads the `run_ended` record, which comes from `audit-records-run-end-and-limit` through `process-count-limit-hit-is-observed`.
- `run-observation-carries-usage` reads the carrier member in `observation.rs` that `process-count-limit-hit-is-observed` extends.

Not established:
- Out of the set, so not a finding: `story:run-observation-carries-usage` names `story:run-observation-names-the-limit-that-ended-it` as the producer of the host-internal carrier member and of "the limit" in its acceptance. That story is active, and no edge runs to it from this story or from `process-count-limit-hit-is-observed`. Someone should check the order.
- `website/docs` is listed (inferred) on all three stories and on most of their neighbours. Pairs that are ordered anyway are not findings, but the entry names no page.
- `## Order` in `exec-file-changes-are-recorded` says `story:executable-lists-on-oci-and-pod` shares `process.rs` and `probe.rs` with it. That story's scope does not list them, only the `spec/` files. The edge is still right. This is wording in the body, not my lane.

```findings
- file: .engineering/planning/story/exec-file-change-record-on-the-wire.md
  line: 134
  category: parallel-safety
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "this story and story:executable-lists-on-oci-and-pod both list the directory website/docs (inferred on both sides), no depends_on path orders them and neither Order section says so; add an edge recording website/docs as the reason, or narrow both entries to the pages each edits"
```
