---
format: aep.planning-md/3
id: review-result:aperture-and-empty-list-parallel-safety-round-1
kind: review-result
status: active
title: 'Plan critic (parallel-safety), round 1: aperture dial outcome, resolver aperture, explicitly empty allow-list'
tags:
- plan-critic
relations:
- reviews: story:aperture-reports-dial-outcome
- reviews: story:resolver-aperture-serves-dns
- reviews: story:explicitly-empty-exec-subpaths-means-the-program-only
revision: 1
---
needs-revision

explicitly-empty-exec-subpaths-means-the-program-only — `spec/confinement/confinement.yaml`, `spec/confinement/scenarios`, `spec/confinement-suite.json`, `crates/substrate-host/tests/confinement-public-items.txt` and `crates/substrate-host/src/confinement/linux.rs` are changed by five stories that sit below `story:executable-deny-list` on a different branch from this one: `run-observation-names-the-limit-that-ended-it`, `audit-records-run-end-and-limit`, `process-count-limit-hit-is-observed`, `run-observation-carries-usage` and `exec-file-changes-are-recorded`. No edge orders either side and the § Order names only `executable-deny-list`. Add an edge to the chain tail with the shared files as the reason, or split the surface so they no longer share it. (cited for `confinement-public-items.txt`, `macos.rs` and `confinement.yaml`; inferred for the rest.) — .engineering/planning/story/explicitly-empty-exec-subpaths-means-the-program-only.md:98

explicitly-empty-exec-subpaths-means-the-program-only — it shares `spec/confinement/confinement.yaml`, the scenarios and the suite with `story:executable-lists-on-oci-and-pod` (that body cites `confinement.yaml:212-220`), and it shares `profile.rs`, `linux.rs`, `confinement.yaml` and `confinement-public-items.txt` with `story:confinement-profile-ingress-routes`. No edge exists in either case and the § Order names neither. The ingress-routes § Order claims to follow "every other story that changes `profile.rs`", which this story contradicts. Record an ordering edge with the shared files as its reason, or split the surface so the two items no longer share it. — .engineering/planning/story/explicitly-empty-exec-subpaths-means-the-program-only.md:98, .engineering/planning/story/confinement-profile-ingress-routes.md:123

aperture-reports-dial-outcome — it shares `crates/substrate-daemon/tests/runtime_vectors.rs` and `crates/substrate-host/src/confinement/linux.rs` (inferred on both sides) with `story:confinement-profile-ingress-routes` and with `story:explicitly-empty-exec-subpaths-means-the-program-only` (`linux.rs`). Each of those is a sibling descendant of `story:exec-file-change-record-on-the-wire` or `story:executable-deny-list`, so no edge orders it against this story. The § Order records only the edge to `exec-file-change-record-on-the-wire`. Record an ordering edge with the shared file as its reason, or split the surface. — .engineering/planning/story/aperture-reports-dial-outcome.md:109

aperture-reports-dial-outcome — it cuts a successor bundle on `crates/substrate-wire/src/lib.rs`, `scripts/gate.sh` and `runtime_vectors.rs`. The § Order says "the wire stories before it", but `story:host-process-adopt` and `story:host-process-container-records` (draft, each planning a successor bundle on the same three files) have no edge to this story or its chain. `story:docker-driver-entry-gate` (proposed) also shares `runtime_vectors.rs` (cited there). Both host-process stories are stale: they target bundle `0.23.0`, so two drafts would claim the next free version. Add an ordering edge naming the shared files, or settle which story cuts the next bundle. — .engineering/planning/story/aperture-reports-dial-outcome.md:109, .engineering/planning/story/host-process-adopt.md:54

resolver-aperture-serves-dns — the new ADR and its `adr/README.md` index row collide with `story:confinement-profile-ingress-routes`, which also adds an ADR and an index row. They also share `crates/substrate-host/src/probe.rs` (cited there) and `runtime_vectors.rs`. The two are siblings with no edge, and the § Order names only `aperture-reports-dial-outcome`. The ADR file does not exist yet, so both would claim the next free number. Record an ordering edge with the shared files as its reason, or split the surface. — .engineering/planning/story/resolver-aperture-serves-dns.md:112

resolver-aperture-serves-dns — it shares `runtime_vectors.rs`, `crates/substrate-wire/src/lib.rs`, `scripts/gate.sh` and the bundle source with the unordered `story:host-process-adopt` and `story:host-process-container-records`. The conditional scope ("only if the ADR adds a wire member") is not tied to an ordering edge. The § Order names only `aperture-reports-dial-outcome`. Record an ordering edge, or state in § Order that the story is held until the bundle owner is settled. — .engineering/planning/story/resolver-aperture-serves-dns.md:112

aperture-reports-dial-outcome — the scope names `docs/design` as a whole directory for one new document. The waves output therefore reports false collisions on designs 01, 04, 15 and 35 (`story:docker-driver-entry-gate`, `story:executable-lists-on-oci-and-pod`, `story:explicitly-empty-exec-subpaths-means-the-program-only`). Name the file instead (design 35 to 37 are taken, so presumably `docs/design/38-…`) and the real collisions become visible. — .engineering/planning/story/aperture-reports-dial-outcome.md:118

**What I read:** the three stories in full, plus the § Order and scope of `story:run-observation-names-the-limit-that-ended-it`, `audit-records-run-end-and-limit`, `process-count-limit-hit-is-observed`, `run-observation-carries-usage`, `exec-file-changes-are-recorded`, `confinement-profile-ingress-routes`, `executable-lists-on-oci-and-pod`, `exec-start-carries-executable-lists`, `host-process-adopt`, `host-process-container-records` and `docker-driver-entry-gate`. Commands: `aep plan artifact show`, `aep plan artifact list --format json` and `aep plan artifact waves --kind story`. I computed the transitive `depends_on` closure over the live stories (draft, active or proposed) against each collision pair, skipping `AGENTS.md`, `README.md` and `CHANGELOG.md`.

**What I could establish:**
- All 3 items have at least one cited surface and 0 are unplaceable.
- Most individual paths are inferred, and several findings rest on inferred paths (flagged above).
- 2 acceptance reads traced to a producer in the set. Both are recorded by an edge: the aperture story reads the file-change wire bundle, and the resolver story reads the aperture story's `egress.rs` and bundle.
- Pairs with every shared file ordered by edge (no finding):
  - aperture ↔ resolver
  - aperture and resolver ↔ the `exec-*`, `process-count`, `run-observation-*`, `audit-records` and `executable-deny-list` chain

**What I could not establish (out of my lane or unresolved):**
- Whether `explicitly-empty-exec-subpaths-means-the-program-only` should be ordered before or after `story:exec-start-carries-executable-lists`. Its type change to `exec_subpaths` is consumed by that story's start mapping, which lives in `process.rs` and `probe.rs`. The body defers this to "when planned" and no collision was derived. I did not report it.
- Whether the split of the three stories is right belongs to the design critic.

```findings
- file: .engineering/planning/story/explicitly-empty-exec-subpaths-means-the-program-only.md
  line: 98
  category: parallel-safety
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "spec/confinement/confinement.yaml, scenarios, suite, confinement-public-items.txt and linux.rs are also changed by run-observation-names-the-limit-that-ended-it, audit-records-run-end-and-limit, process-count-limit-hit-is-observed, run-observation-carries-usage and exec-file-changes-are-recorded, which are siblings under executable-deny-list with no edge to this story, and the Order names only executable-deny-list; add an ordering edge recording the shared files or split the surface (linux.rs and scenario paths inferred)"
- file: .engineering/planning/story/explicitly-empty-exec-subpaths-means-the-program-only.md
  line: 98
  category: parallel-safety
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "it shares confinement.yaml with executable-lists-on-oci-and-pod and profile.rs, linux.rs, confinement.yaml and confinement-public-items.txt with confinement-profile-ingress-routes, no edge orders either pair and the Order names neither (ingress-routes claims to follow every story changing profile.rs); add an ordering edge recording the shared files or split the surface"
- file: .engineering/planning/story/aperture-reports-dial-outcome.md
  line: 109
  category: parallel-safety
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "runtime_vectors.rs and confinement/linux.rs (inferred) are shared with confinement-profile-ingress-routes and explicitly-empty-exec-subpaths-means-the-program-only, siblings with no edge to this story, and the Order records only exec-file-change-record-on-the-wire; add an ordering edge recording the shared file or split the surface"
- file: .engineering/planning/story/aperture-reports-dial-outcome.md
  line: 109
  category: parallel-safety
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "the Order says 'the wire stories before it' but host-process-adopt and host-process-container-records (draft, successor bundle on crates/substrate-wire/src/lib.rs, scripts/gate.sh, runtime_vectors.rs, target 0.23.0) and docker-driver-entry-gate (runtime_vectors.rs) have no edge to this story; add an ordering edge recording the shared files or settle which story cuts the next bundle"
- file: .engineering/planning/story/resolver-aperture-serves-dns.md
  line: 112
  category: parallel-safety
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "its new ADR and adr/README.md index row (file does not exist yet), probe.rs and runtime_vectors.rs are shared with confinement-profile-ingress-routes, a sibling with no edge, and the Order names only aperture-reports-dial-outcome; add an ordering edge recording the shared files or split the surface (probe.rs and runtime_vectors.rs inferred on this side)"
- file: .engineering/planning/story/resolver-aperture-serves-dns.md
  line: 112
  category: parallel-safety
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "its conditional wire scope (runtime_vectors.rs, wire lib.rs, gate.sh, bundle source; inferred) is shared with the unordered host-process-adopt and host-process-container-records and the Order does not say so; add an ordering edge or state that the story is held until the bundle owner is settled"
- file: .engineering/planning/story/aperture-reports-dial-outcome.md
  line: 118
  category: parallel-safety
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "scope names the whole docs/design directory for one new document, which the waves output reports as false collisions on designs 01, 04, 15 and 35 and hides the real ones; name the new file (design 35 to 37 are taken) instead (inferred surface)"
```
