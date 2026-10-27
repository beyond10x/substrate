---
format: aep.planning-md/3
id: review-result:adr-0109-plan-scope-round-1
kind: review-result
status: active
title: Scope critic (aep:plan-critic-scope), ADR 0109 plan round 1
relations:
- reviews: epic:confinement-contract-family
- reviews: story:confinement-domain-specification
- reviews: story:confinement-contract-on-linux-and-local
- reviews: story:macos-adapter-on-seatbelt
- reviews: story:macos-seatbelt-policy-runs-cargo-test
- reviews: story:systemd-scope-bounds-provider
- reviews: story:oci-adapter
- reviews: story:pod-adapter
- reviews: story:confinement-consumer-surface
- reviews: story:sandbox-repository-parity
revision: 2
---
needs-revision

- story:confinement-domain-specification — ADR decision 2 promises "A profile that needs a fact the adapter lacks is refused with the fact's name" and decision 7 "an absent fact is a refusal scenario, never a skip", but the body's five refusal codes name only a family, an adapter, `local` or a reserved field, and `admit_profile` takes only `required_families`; add a fact-unserved refusal and a required-facts input (without them, `macos`'s "refused by name" for `paths_are_host_paths` and whole-tree kill has no code) — atlas:architecture/adr/0109-confinement-is-one-substrate-contract-with-adapters.md:58 and :74-75; .engineering/planning/story/confinement-domain-specification.md:76-82; epic line 26-27
- story:confinement-domain-specification — ADR decision 7 promises "one conformance scenario per promise and adapter", and the acceptance narrows it to "for each of the ten families, one admission on an adapter that serves it", so nothing requires any scenario for `macos`, `systemd`, `oci` or `pod`, whose stories each "execute every <adapter> scenario of the suite" (possibly zero); add a per-adapter scenario floor — atlas:architecture/adr/0109-confinement-is-one-substrate-contract-with-adapters.md:74-75; .engineering/planning/story/confinement-domain-specification.md:92
- story:confinement-contract-on-linux-and-local — ADR decision 1 says the profile states "IPC (socket families, signals)", and the S0 profile list has workspaces, filesystem view, network, bounds, I/O bounds, secret slots, audit sink and the reserved fields but no IPC; it is neither shipped, nor reserved (F1 B reserves only ingress and rate), nor excluded in the epic, so the profile body must claim it or the epic must record the deferral — atlas:architecture/adr/0109-confinement-is-one-substrate-contract-with-adapters.md:55-56; .engineering/planning/story/confinement-contract-on-linux-and-local.md:49-50
- epic:confinement-contract-family — ADR Consequences promise "`agent-stack/stack.yaml` gains a `substrate` node beneath the runtime, with edges from loom and mantle", and no story claims it and the epic's Out of scope does not name it (the natural fix is a line under "Downstream hops" or a story in agent-stack) — atlas:architecture/adr/0109-confinement-is-one-substrate-contract-with-adapters.md:112; .engineering/planning/epic/confinement-contract-family.md:60-71
- story:confinement-domain-specification — the acceptance moves every `spec/*/ess-inputs.yaml` pin, `gate.yml` and the regenerated `crates/substrate-wire/src/generated/*.rs` to ESS 0.70.0, though ADR 0109 asks only for a new domain; this lands in the wire-crate territory the epic excludes ("Any wire change") and contradicts the story's own "any wire or bundle change" exclusion, so the pin move needs its own story or a stated authority and an unchanged-bytes condition for the wire crate — .engineering/planning/story/confinement-domain-specification.md:87-90 and :116-122; .engineering/planning/epic/confinement-contract-family.md:62
- story:confinement-consumer-surface — its first acceptance ("exports one module, `confinement`, whose public items are the profile, the adapter request, `open`, the facts with `served_families`, the refusal type, and run handles") is the surface S0 already delivers ("`confinement::open`", `facts()`, `start/observe/output/wait/signal`), so both would be marked done for one outcome; narrow this story to the consumer-driven changes, the snapshot and the docs — .engineering/planning/story/confinement-consumer-surface.md:37-40; .engineering/planning/story/confinement-contract-on-linux-and-local.md:24-37

**What I read:** 10 artifacts (the epic and nine stories) plus ADR 0109 in full, via `aep plan artifact show` on each, `aep plan artifact graph`, and `grep` over the bodies. From the ADR and epic I extracted 23 promises and traced 19 to an item; the 4 not fully traced are the findings above on fact refusals, per-adapter scenarios, IPC and the agent-stack node. Two items outside those 23 count as covered by exclusion: F2 and the downstream hops. The successor wire bundle is likewise excluded.

**What I could not establish:**
- `approval-record:adr-0109-confinement-plan`, which the epic cites for the operator's words, is absent from the store (`aep plan artifact show` errors), so I could not read the quote. Which story should own the fix is the caller's call.
- I did not run ESS, so I could not tell whether the pin move changes the generated wire bytes.
- `sandbox-layer.md` and `chain-scope.md` were not read.
- The `confine run` verb is traceable to ADR decision 5 ("Atlas's O6 loop then calls Substrate's command line"), so I did not call it reach.
- Out of lane for acceptance: the domain acceptance requires "for each of the ten families, one admission on an adapter that serves it", but `network_ingress` is served by no adapter (S0 body), so that bullet cannot hold as written.
- Out of lane for design: `oci` refuses images named by tag, while `sandbox-repository-parity` carries `--image`, which looks inconsistent with `sandbox` parity.

```findings
- file: .engineering/planning/story/confinement-domain-specification.md
  line: 76
  category: scope
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "ADR 0109 decision 2 promises a profile needing a fact the adapter lacks is refused with the fact's name and decision 7 an absent fact as a refusal scenario, but the body's refusal codes name only a family, an adapter, local or a reserved field and admit_profile takes only required_families; add a fact-unserved refusal and a required-facts input"
- file: .engineering/planning/story/confinement-domain-specification.md
  line: 92
  category: scope
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "ADR 0109 decision 7 promises one conformance scenario per promise and adapter, and the acceptance narrows it to one admission per family on one adapter, so nothing requires any scenario for macos, systemd, oci or pod; add a per-adapter scenario floor"
- file: .engineering/planning/story/confinement-contract-on-linux-and-local.md
  line: 49
  category: scope
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "ADR 0109 decision 1 says the profile states IPC (socket families, signals) and the S0 profile list omits it, so it is neither shipped, reserved nor excluded by the epic; claim it in the profile or record the deferral"
- file: .engineering/planning/epic/confinement-contract-family.md
  line: 60
  category: scope
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "ADR 0109 Consequences promise agent-stack/stack.yaml gains a substrate node with edges from loom and mantle, and no story claims it and the epic's Out of scope does not name it; claim it in a story or list it under the downstream hops"
- file: .engineering/planning/story/confinement-domain-specification.md
  line: 87
  category: scope
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "the acceptance moves every spec ess-inputs pin and regenerates crates/substrate-wire/src/generated to ESS 0.70.0, which ADR 0109 does not ask for and which reaches the wire territory the epic excludes and the story itself disclaims; split the pin move out or state its authority and require unchanged wire bytes"
- file: .engineering/planning/story/confinement-consumer-surface.md
  line: 37
  category: scope
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "the first acceptance (one exported confinement module holding the profile, request, open, facts, refusal type and run handles) is the surface story:confinement-contract-on-linux-and-local already delivers, so two items claim one outcome; narrow this story to the consumer-driven changes, snapshot and docs"
```
