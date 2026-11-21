---
format: aep.planning-md/3
id: review-result:sdc-run-governance-parallel-safety-round-2
kind: review-result
status: active
title: 'Plan critic (parallel-safety), round 2: runs are governed and limit hits are events'
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
revision: 1
---
needs-revision

story:exec-start-carries-executable-lists — the epic orders story 6 (`story:executable-lists-on-oci-and-pod`) before this one and says each story carries an edge to its predecessor, but this story has no `depends_on` path to story 6, and story 8 has none either, so `aep plan artifact waves` puts 6 and 7 in wave 6 together. Their surfaces do not intersect (cited, both Scope sections), so the Order section should either record an edge to story 6 with its reason or say it is independent of story 6 and the epic should drop "one at a time" for that pair. — .engineering/planning/epic/runs-are-governed-and-limit-hits-are-events.md:103-105, .engineering/planning/story/exec-start-carries-executable-lists.md:117

**What I read:** 9 stories, the epic, and the `confinement.yaml`, `site_drift.rs` and website tables they reach. Commands:
- `aep plan artifact show` on each story and on the epic
- `aep plan artifact waves --kind story --status draft`
- `aep plan artifact list --format json`
- `git grep` for `exec_subpaths`, `ConfinementProperty`, `RunObservation` and `AuditRecord`

**Placement:**
- Cited: 3 of 9 (stories 4, 6 and 9, named by path in their bodies).
- Partly inferred: the other 6, each with at least one inferred path in its Scope; none has an empty scope.
- Unplaceable: 0.

**Reads and edges:**
- I traced 9 acceptance reads to a producer in the set.
  - Direct edges: 4 reads 3's field, 5 reads 4's record, 6 reads 2's field, 8 reads 5's counter and design section, and 9 reads the inbound-endpoint mechanism.
  - Transitive edges: 7 reads 1's mechanism through 2, and 9 reads 1's mechanism and 2's field.
  - None: 3 reads nothing from 2, and its Order says so.
- All 9 reads have a `depends_on` path from reader to producer.
- The chain 1, 2, 3, 4, 5 is linear. Stories 3, 4 and 5 share `confinement/mod.rs`, `linux.rs` and `confinement-public-items.txt`, and each Order names the shared files. 7 and 8 share `process.rs`, `wire/lib.rs`, `gate.sh` and `runtime_vectors.rs`, and 7's Order says so. 9 shares `confinement.yaml` and `runtime_vectors.rs` with 6 and 8, and its Order says so.

**What I could not establish:**
- Stories 6 and 7 might meet on a file neither names. I looked for one and found none, so the finding above rests on the cited scopes.
- Out of my lane (acceptance or scope critics):
  - Every story that edits `confinement.yaml` or the suite also owes `website/docs/concepts/adapters.md` and `website/docs/reference/contracts.md` for `check-docs`, because `xtask/src/site_drift.rs` compares them to the spec. Story 9 also owes `website/docs/reference/contract.md:172`, which says the daemon "never publishes `network-ingress`". No Scope names these pages. All the spec-editing stories are ordered, so no collision is unrecorded.
  - Story 7's Scope omits these files, which it must touch:
    - `crates/substrate-wire/src/confinement_facts_tests.rs`
    - `xtask/src/bundle/confinement_facts.rs`
    - `crates/b10x-substrate-sdk/src/model.rs`
    - `crates/substrate-kubernetes/src/namespace/probe_tests.rs`
- Story `story:inbound-endpoint-for-a-confined-exec` is outside the set and has no edges. I did not judge it.

```findings
- file: .engineering/planning/story/exec-start-carries-executable-lists.md
  line: 117
  category: parallel-safety
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "the epic orders story 6 before this story and says each story carries an edge to its predecessor, but this story has no depends_on path to story:executable-lists-on-oci-and-pod (nor does story 8), so waves puts 6 and 7 in wave 6 together; their cited surfaces do not intersect, so the Order section should either record an edge to story 6 with its reason or state that this story is independent of it, and the epic should stop claiming one at a time for that pair"
```
