---
format: aep.planning-md/3
id: review-result:aperture-and-empty-list-design-round-2
kind: review-result
status: active
title: 'Plan critic (design), round 2: aperture dial outcome, resolver aperture, explicitly empty allow-list'
tags:
- plan-critic
relations:
- reviews: story:aperture-reports-dial-outcome
- reviews: story:resolver-aperture-serves-dns
- reviews: story:explicitly-empty-exec-subpaths-means-the-program-only
revision: 1
---
needs-revision

aperture-reports-dial-outcome — its change 3 sends the counts "through the `linux` adapter to an embedder", which can only mean a new member on the public `RunObservation`, because that struct has no network or applied field (`linux.rs:507-526`). Neither the body nor its design-document decisions name that member, and its scope omits `confinement/mod.rs`, `confinement-public-items.txt` and `spec/confinement`. The `RunObservation` members belong to the sdc run-observation stories. Either drop the embedder route, or decide it in the design document, add those files to the scope and order it after those stories. Without the route, the `linux.rs` edge from `story:explicitly-empty-exec-subpaths-means-the-program-only` has no shared file behind it. — `.engineering/planning/story/aperture-reports-dial-outcome.md:95`, `crates/substrate-host/tests/confinement-public-items.txt:152-159`

explicitly-empty-exec-subpaths-means-the-program-only — the body says "No wire change" and leaves the wire meaning of an empty list to `story:exec-start-carries-executable-lists`, which also "decides … what each means". The enforcement of an empty list on `linux` lives in `process.rs` and the exec gate, and neither is in this story's scope. The semantics of "empty" are split across two stories, so the seam is unowned. If the wire story ships empty as "runs as today", this story changes the enforcement of already-released wire bytes (invariants 3 and 6). If it ships empty as "program only", it owes that enforcement itself. State which of the two this story assumes, and what the daemon does with a wire-empty list until this story lands. — `.engineering/planning/story/explicitly-empty-exec-subpaths-means-the-program-only.md:97`, `.engineering/planning/story/exec-start-carries-executable-lists.md:82-84`

resolver-aperture-serves-dns — the body makes the successor bundle conditional ("if the ADR adds a wire member"), but its own decisions and acceptance already require one. The ADR must decide "the capability fact, the applied record and each named refusal", and the portable-lane check requires the fact absent and a start refused by name before dispatch. Facts and named refusals are published only in a bundle (invariant 6). State the bundle as certain, so the Order and Scope sections hold it to the same bundle-version ordering as `story:aperture-reports-dial-outcome`. — `.engineering/planning/story/resolver-aperture-serves-dns.md:83-84`, `.engineering/planning/story/resolver-aperture-serves-dns.md:103`

Not a finding: the dial-outcome → resolver and dial-outcome → empty-subpaths edges are recorded shared-file orderings, and each has its reason written beside it. The code claims in the three stories hold in this checkout:
- `exec_subpaths: Vec<String>` and the non-empty rule behind the required fact (`profile.rs:72`, `:216`).
- The macos program-only default (`macos.rs:3-5`).
- The blocking `connect` in `relay_body` (`egress.rs:1216`).
- The `AppliedAperture` members the story lists (`lib.rs:1864-1886`).

**What I read:** 3 stories whole, plus `story:exec-start-carries-executable-lists`, `story:linux-executable-allow-list` and `story:executable-deny-list`, plus the run-observation stories by grep. I ran `aep plan artifact show`, `aep plan artifact relations`, `aep plan artifact validate` (reports "valid"), and `aep plan artifact list --format json` with a full-store (530 artifacts) cycle walk; I also read the code at the cited lines.

**What I could not establish:**
- The graph walk covered every `depends_on` edge in the store, outside the set included, and found no cycle.
- I traced 3 acceptance reads to a producer in the set, and 2 of them have an edge: the empty-subpaths reads on `linux.rs` and on the wire story's absent-versus-empty decision; the dial-outcome read is the sibling bundle version. The third, `RunObservation`, has none because the story names no such surface.
- `docs/design/35-executable-lists.md` is not in this checkout, so I cannot check what it decides about empty lists.
- Whether the dial-outcome ordering after `story:exec-file-change-record-on-the-wire` is enough for the bundle version is for the parallel-safety critic.
- Whether each acceptance line can be checked belongs to the acceptance critic. I did not judge whether the resolver ADR's exfiltration limit is adequate, which is a security question, not design shape.

```findings
- file: .engineering/planning/story/aperture-reports-dial-outcome.md
  line: 95
  category: design
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "change 3 reports the counts 'through the linux adapter to an embedder', which can only be a new member on the public RunObservation that the sdc run-observation stories own; neither the design-document decisions, the scope (confinement/mod.rs, confinement-public-items.txt, spec/confinement) nor the order name it, so either drop the route or decide it, scope it and order it after those stories, and without it the linux.rs edge from story:explicitly-empty-exec-subpaths-means-the-program-only has no shared file behind it"
- file: .engineering/planning/story/explicitly-empty-exec-subpaths-means-the-program-only.md
  line: 97
  category: design
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "it states 'No wire change' while story:exec-start-carries-executable-lists decides what an empty list means on the wire, and the linux enforcement it needs lives in process.rs and the exec gate, which are not in its scope; the meaning of empty is split between the two stories, so the body must say whether the wire already means 'program only' (then this story only retypes the profile) or means 'as today' (then the same released bytes change enforcement here), and what the daemon does with a wire-empty list until this story lands"
- file: .engineering/planning/story/resolver-aperture-serves-dns.md
  line: 84
  category: design
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "the successor bundle is conditional ('if the ADR adds a wire member') but the ADR must decide a capability fact, an applied record and named refusals and the portable-lane check requires the fact absent and a named refusal before dispatch, all of which are published only in a bundle (invariant 6), so the body must state the bundle as certain and order it with the other bundle-cutting stories"
```
