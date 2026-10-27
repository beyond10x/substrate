---
format: aep.planning-md/3
id: review-result:adr-0109-plan-parallel-round-2
kind: review-result
status: active
title: Parallel-safety critic (aep:plan-critic-parallel-safety), ADR 0109 plan round 2
relations:
- reviews: epic:confinement-contract-family
- reviews: story:ess-pin-0-70-0
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

story:sandbox-repository-parity — no `depends_on` path runs from this story to story:macos-seatbelt-policy-runs-cargo-test, so they can run at once, and both append to `CHANGELOG.md` (cited, both Scope sections; `aep plan artifact waves` prints this collision). Its Scope line gives the entry without the sentence that a merge into the integration branch keeps every line, which the other six stories that carry a line have — .engineering/planning/story/sandbox-repository-parity.md:78, .engineering/planning/story/macos-seatbelt-policy-runs-cargo-test.md:70
story:confinement-consumer-surface — its committed list freezes the public items of the whole `confinement` module, which includes `confinement/macos.rs` (inferred from "the module's public items"). story:macos-seatbelt-policy-runs-cargo-test edits that file (cited, its Scope), and no `depends_on` edge runs between them, because the consumer's four adapter edges leave out the fifth adapter story. Add the edge, or state that the list excludes `macos.rs` items the S2 story may change — .engineering/planning/story/confinement-consumer-surface.md:53, .engineering/planning/story/confinement-consumer-surface.md:70, .engineering/planning/story/macos-seatbelt-policy-runs-cargo-test.md:67

**What I read:** 11 artifacts and round 1's review-result, with `aep plan artifact show` on the epic and the ten stories and on `review-result:adr-0109-plan-parallel-round-1`. I also ran `aep plan artifact waves --kind story --status draft`, `aep plan artifact graph` and `aep plan artifact validate` (which printed `valid`), and read line numbers from the story files.

Surfaces: 10 stories cited (every body has a Scope section naming paths), 0 inferred beyond the store's own label, 0 unplaced. The epic holds no file scope.

Acceptance reads traced to a producer in the set: 11. Edges: story:macos-seatbelt-policy-runs-cargo-test reads the macOS adapter's S1 refusals and has its edge. The consumer-surface freeze list reads `macos.rs` from the S2 story and has no edge (the finding above). All 10 others have their edges.

Round 1's five findings no longer hold.
- `mod.rs` is owned by the contract story and the adapter stories now replace one stub file each.
- story:confinement-consumer-surface depends on all four adapters and story:sandbox-repository-parity on the consumer surface.
- The `gate.yml` and `CHANGELOG.md` sharing is now stated in the bodies.

Other shared files are ordered by edges:
- `gate.yml` (story:oci-adapter and story:pod-adapter)
- `delegated-lane.sh` (the contract story, story:systemd-scope-bounds-provider and story:confinement-consumer-surface)
- `Cargo.lock` (story:pod-adapter and story:sandbox-repository-parity)
- `lib.rs` and `spec/README.md` (the contract/domain and domain/ESS-pin pairs)
- `macos.yml`, `macos.rs` and `seatbelt.rs` (the macOS S1 and S2 pair)

**What I could not establish:**
- Three stories do not carry the sentence that a merge keeps every line: story:ess-pin-0-70-0 (line 67), story:confinement-domain-specification (line 120) and story:confinement-contract-on-linux-and-local (line 115). Each is an ancestor of every other story, so none can run beside a CHANGELOG writer and I raise no collision. If the operator's wording applies to every story literally, add it there too.
- `waves` currently puts each story of the set in a wave of its own, serialised by the `CHANGELOG.md` and shared-file collisions. The bodies' phrase "the other units of its wave" therefore describes a wave shape the tool does not derive. That is wording, outside my lane.
- Whether a Kubernetes client crate in `Cargo.toml` collides with anything: only story:pod-adapter names it.

```findings
- file: .engineering/planning/story/sandbox-repository-parity.md
  line: 78
  category: parallel-safety
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "no depends_on path orders it against story:macos-seatbelt-policy-runs-cargo-test, both append to CHANGELOG.md, and its Scope line omits the sentence that a merge into the integration branch keeps every line, which the other six stories carrying a line have"
- file: .engineering/planning/story/confinement-consumer-surface.md
  line: 70
  category: parallel-safety
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "its public-items freeze list covers the whole confinement module including confinement/macos.rs (inferred surface), which story:macos-seatbelt-policy-runs-cargo-test edits, and no depends_on edge or body sentence orders the two because the consumer's four adapter edges omit the S2 story; add the edge or state that the list excludes macos.rs items"
```
