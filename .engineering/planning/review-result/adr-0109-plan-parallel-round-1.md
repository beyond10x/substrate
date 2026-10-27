---
format: aep.planning-md/3
id: review-result:adr-0109-plan-parallel-round-1
kind: review-result
status: active
title: Parallel-safety critic (aep:plan-critic-parallel-safety), ADR 0109 plan round 1
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
revision: 1
---
needs-revision
story:oci-adapter — W2 holds three stories that each edit `crates/substrate-host/src/confinement/mod.rs` for adapter "(registration)" (this one, story:macos-adapter-on-seatbelt and story:systemd-scope-bounds-provider), the shared file is cited in all three bodies, and no body names the others; add an ordering edge that records mod.rs as its reason, or have story:confinement-contract-on-linux-and-local pre-register the adapter slots so the three drop mod.rs from their scope — .engineering/planning/story/oci-adapter.md:65, .engineering/planning/story/macos-adapter-on-seatbelt.md:59, .engineering/planning/story/systemd-scope-bounds-provider.md:54
story:confinement-consumer-surface — it lands on `crates/substrate-host/src/confinement/mod.rs` (cited) beside story:pod-adapter in W3 and neither body says so; its own Depends-on line says it "runs after the wave holding the adapters" but only a `depends_on` edge to story:confinement-contract-on-linux-and-local exists, so the plan puts it in the same wave as the adapter it is meant to follow; add the edge to story:pod-adapter that records mod.rs, or split the surface so the two no longer share it — .engineering/planning/story/confinement-consumer-surface.md:55, .engineering/planning/story/confinement-consumer-surface.md:61, .engineering/planning/story/pod-adapter.md:67
story:sandbox-repository-parity — its verb is written against `confinement::open` (cited) in W3 while story:confinement-consumer-surface rewrites the exported surface of that module in the same wave, and no `depends_on` edge or body sentence records it; add the edge from the reader, or merge the two — .engineering/planning/story/sandbox-repository-parity.md:49, .engineering/planning/story/confinement-consumer-surface.md:55
story:systemd-scope-bounds-provider — W2 stories story:macos-adapter-on-seatbelt and story:oci-adapter all append to `CHANGELOG.md` (cited, all three Scope sections), and none says so; record an ordering edge naming the shared file, or move each entry to a per-story fragment — .engineering/planning/story/systemd-scope-bounds-provider.md:55, .engineering/planning/story/oci-adapter.md:67, .engineering/planning/story/macos-adapter-on-seatbelt.md:61
story:macos-seatbelt-policy-runs-cargo-test — W3 stories story:pod-adapter, story:confinement-consumer-surface and story:sandbox-repository-parity all append to `CHANGELOG.md` (cited, all four Scope sections), and none says so; record an ordering edge naming the shared file, or move each entry to a per-story fragment — .engineering/planning/story/macos-seatbelt-policy-runs-cargo-test.md:55, .engineering/planning/story/pod-adapter.md:71, .engineering/planning/story/confinement-consumer-surface.md:57, .engineering/planning/story/sandbox-repository-parity.md:59

What I read: 9 stories with `aep plan artifact show` and `aep plan artifact waves`, plus `aep plan artifact graph` and `relations`, and the six W2 and W3 story files under `.engineering/planning/story/` for line numbers.
- Surfaces: 9 cited (every body has a Scope section naming paths). The store labels all 9 "inferred" and its scope columns match the body Scope sections. 0 unplaced.
- Acceptance reads traced to a producer in the set: 9 (contract to domain; macos-adapter, systemd, oci, pod and consumer-surface each to contract; macos-policy to macos-adapter; parity to contract and to oci). All 9 have a `depends_on` edge.
- W1 is sound: lib.rs is shared, but the `depends_on` edge orders the two.
- W2 and W3 are otherwise independent:
  - `macos.yml`, `macos.rs` and `seatbelt.rs` are shared only across the W2 to W3 edge.
  - `gate.yml` is shared by oci (W2) and pod (W3) only across waves.
  - `Cargo.toml` and `Cargo.lock` are touched only by pod.
  - `delegated-lane.sh` is touched only by systemd within W2.
- The CHANGELOG findings are the weakest: a one-entry append is a trivial merge, but the rubric counts it as an unnamed collision.

What I could not establish:
- Whether the adapter stories must add scenarios to `spec/confinement-suite.json`, which none of their Scope sections lists. This is the design and acceptance critics' lane and does not set my verdict.
- Whether `--dry-run` in story:sandbox-repository-parity can print adapter argv through the contract's trait. This is out of my lane.
- Anything about the other ~60 stories in the store, including the 756 collision lines `waves` prints for them.

```findings
- file: .engineering/planning/story/oci-adapter.md
  line: 65
  category: parallel-safety
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "W2 holds three stories that each edit crates/substrate-host/src/confinement/mod.rs for adapter registration (oci-adapter, macos-adapter-on-seatbelt, systemd-scope-bounds-provider) and no body names the others; an ordering edge naming mod.rs, or pre-registered adapter slots in the contract story, would resolve it"
- file: .engineering/planning/story/confinement-consumer-surface.md
  line: 61
  category: parallel-safety
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "it edits confinement/mod.rs beside pod-adapter in W3 and says it runs after the wave holding the adapters, but only a depends_on edge to the contract story exists, so the plan schedules it concurrently with pod-adapter; add the edge or split the surface"
- file: .engineering/planning/story/sandbox-repository-parity.md
  line: 49
  category: parallel-safety
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "its CLI verb is written against confinement::open while confinement-consumer-surface rewrites the exported surface of that module in the same wave, with no depends_on edge from the reader; add the edge or merge the two"
- file: .engineering/planning/story/systemd-scope-bounds-provider.md
  line: 55
  category: parallel-safety
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "systemd-scope-bounds-provider, macos-adapter-on-seatbelt and oci-adapter all append to CHANGELOG.md in W2 and none says so; record an ordering edge naming the file or use per-story fragments"
- file: .engineering/planning/story/macos-seatbelt-policy-runs-cargo-test.md
  line: 55
  category: parallel-safety
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "macos-seatbelt-policy-runs-cargo-test, pod-adapter, confinement-consumer-surface and sandbox-repository-parity all append to CHANGELOG.md in W3 and none says so; record an ordering edge naming the file or use per-story fragments"
```
