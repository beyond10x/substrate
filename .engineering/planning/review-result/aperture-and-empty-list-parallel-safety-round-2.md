---
format: aep.planning-md/3
id: review-result:aperture-and-empty-list-parallel-safety-round-2
kind: review-result
status: active
title: 'Plan critic (parallel-safety), round 2: aperture dial outcome, resolver aperture, explicitly empty allow-list'
tags:
- plan-critic
relations:
- reviews: story:aperture-reports-dial-outcome
- reviews: story:resolver-aperture-serves-dns
- reviews: story:explicitly-empty-exec-subpaths-means-the-program-only
revision: 1
---
needs-revision

story:resolver-aperture-serves-dns — it shares `crates/substrate-daemon/tests/runtime_vectors.rs` with the unordered `story:docker-driver-entry-gate`, and its § Order says nothing about that story. The "Not ordered against this story" paragraph names only `story:host-process-adopt` and `story:host-process-container-records`. The fix is either to add `story:docker-driver-entry-gate` to that paragraph with the reason, as `story:aperture-reports-dial-outcome` § Order does, or to add an ordering edge. The path is cited in both scopes. The resolver's acceptance is a delegated-lane case, so it lands on that file whether or not the ADR adds a wire member. — `.engineering/planning/story/resolver-aperture-serves-dns.md:123`, `.engineering/planning/story/docker-driver-entry-gate.md:94`

**What I read.** 44 non-terminal stories (draft, active and proposed). I took each one's scope and `depends_on` edges from `aep plan artifact show`, plus the statuses from `aep plan artifact list --format json`. I computed shared paths and transitive `depends_on` reachability for each of the three stories, ignoring `AGENTS.md`, `README.md`, `CHANGELOG.md` and `website/docs`. I also read `aep plan artifact waves --kind story`.

**Result for the other pairs.**
- Every other pair with a shared path is ordered by a direct or transitive `depends_on` edge. That includes `story:exec-file-change-record-on-the-wire`, the limit, usage, file-change and process-count stories, the executable-list stories, and `story:confinement-profile-ingress-routes`.
- The unordered pairs with `story:host-process-adopt` and `story:host-process-container-records` are recorded in both aperture stories' § Order.
- The pair `story:aperture-reports-dial-outcome` × `story:docker-driver-entry-gate` is recorded at `aperture-reports-dial-outcome.md:136`.
- `story:explicitly-empty-exec-subpaths-means-the-program-only` has no unordered pair.
- The new documents `docs/design/38-aperture-dial-outcome.md` and `docs/design/37-executable-lists-on-the-wire.md` do not clash, and no other story claims number 38. The new ADR numbers are deferred to "next free number" and ordered by the edges.

**Surface counts (the three stories under review).** Cited: 3. Inferred: 0. Unplaceable: 0. Each names files, though most scope paths are marked inferred in the store. I traced 3 acceptance reads to a producer in the set, and a `depends_on` edge records all 3. The reads are the wire bundle (the predecessor wire stories), the `exec_subpaths` field and design 35 (the executable-list stories), and the egress dial (the dial-outcome story, for the resolver).

**What I could not establish.** None. `story:docker-driver-entry-gate` is proposed and not being worked, so its omission is a recording gap, not a live conflict. `crates/substrate-daemon/tests/runtime_vectors.rs` is listed in the resolver's scope with the caveat "only if the ADR adds a wire member", which is a scoping point for the design critic.

```findings
- file: .engineering/planning/story/resolver-aperture-serves-dns.md
  line: 123
  category: parallel-safety
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "this story and story:docker-driver-entry-gate both land on crates/substrate-daemon/tests/runtime_vectors.rs (cited in the older story, inferred here), but § Order's 'Not ordered' paragraph names only story:host-process-adopt and story:host-process-container-records; record the unordered pair with its reason or add an ordering edge"
```
