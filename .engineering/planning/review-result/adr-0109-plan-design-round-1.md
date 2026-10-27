---
format: aep.planning-md/3
id: review-result:adr-0109-plan-design-round-1
kind: review-result
status: active
title: Design critic (aep:plan-critic-design), ADR 0109 plan round 1
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

story:sandbox-repository-parity — its acceptance asserts every flag is present in the `linux` and `oci` adapters' argv for a profile, and `--dry-run` prints that argv, but no story in the set creates an argv-rendering surface, and the parity scope excludes `crates/substrate-host/src`. S0's trait is `facts`/`admit`/`start`/`observe`/`output`/`wait`/`signal`, and the bwrap argv is built privately in `process.rs`. S0 and `story:oci-adapter` need an acceptance line exposing a profile-to-argv render (or the parity scope must take those source files), with the existing `depends_on` edges kept. — .engineering/planning/story/sandbox-repository-parity.md:44, .engineering/planning/story/sandbox-repository-parity.md:50, .engineering/planning/story/confinement-contract-on-linux-and-local.md:51-52

story:confinement-consumer-surface — it describes the public surface of `confinement` (profile, request, `open`, facts, refusal, run handles), which S0 already defines and exports. It then says it "runs after the wave holding the adapters, so the surface it fixes is the one they implement", yet it declares `depends_on` only on S0. The public API is therefore split between two stories that both edit `confinement/mod.rs` and `lib.rs`, and a surface change here would force rework in five adapters. Either add `depends_on` edges to the five adapter stories, or move the surface definition into S0 and shrink this story to the consumer test, snapshot and docs page. — .engineering/planning/story/confinement-consumer-surface.md:38-39, .engineering/planning/story/confinement-consumer-surface.md:61

story:macos-seatbelt-policy-runs-cargo-test — it says writable subtrees, sealed paths, executable subpaths and a private `TMPDIR` are each "a profile field of the `macos` adapter". The body does not say whether these extend S0's `ConfinementProfile` (so profile.rs and the domain `Profile` entity change, neither in its scope or edges) or stay `SeatbeltPolicy`-only. If they stay `macos`-only, a consumer that picks its adapter by `served_families` cannot reach them. S0's profile has no sealed-path or exec-subpath field, while its `workspaces` field presumably already covers writable roots. The body should state which. — .engineering/planning/story/macos-seatbelt-policy-runs-cargo-test.md:39-40, .engineering/planning/story/confinement-contract-on-linux-and-local.md:49-51

story:confinement-domain-specification — it is the only story that authors scenarios, and its suite acceptance requires one admission per family and one refusal per family where one exists. It does not require scenarios for each of `macos`, `systemd`, `oci` and `pod`. The domain has no `inner` for `systemd { inner: local|linux }`, which the systemd story opens. Four adapter stories nevertheless read "every `<adapter>` scenario" from that suite and have no `spec/` in their scope. The acceptance should require a scenario set per adapter, including the systemd inner variants, or the adapter stories should take `spec/confinement/scenarios` and the suite into scope. — .engineering/planning/story/confinement-domain-specification.md:92-99, .engineering/planning/story/systemd-scope-bounds-provider.md:36-37

story:confinement-domain-specification — it combines two separable things: the new `substrate.confinement` domain with ADR 0044, and a repository-wide ESS pin move. The pin move rewrites every `spec/*/ess-inputs.yaml`, `gate.yml`, every existing `*-suite.json`, `spec/generated/cli` and `crates/substrate-wire/src/generated`. S0's "no wire change" check then depends on that regeneration being byte-neutral. Splitting the pin bump into its own story, with the domain depending on it, leaves each tree green on its own. — .engineering/planning/story/confinement-domain-specification.md:88-90, .engineering/planning/story/confinement-domain-specification.md:116-117

**What you read**
- 11 artifacts: the epic, the nine stories, and the first 20–60 lines of the four related ones (`story:macos-seatbelt-confinement`, `story:kubernetes-namespace-driver-entry-gate`, `epic:container-driver-entry`, `story:docker-driver-entry-gate`).
- Commands: `aep plan artifact show` for each set artifact, `relations`, `graph` and `validate` (it printed `valid`), plus ADR 0109 and `crates/substrate-host/src` greps.
- Edges walked: 9 `depends_on` and 9 `decomposes` inside the set. Outside the set I followed 4 `informed_by` edges and the `depends_on` edges of `epic:container-driver-entry` and `story:kubernetes-namespace-driver-entry-gate`. No cycle was found.
- The shape is a diamond: the domain story feeds S0, S0 feeds the adapters, and the adapters feed consumer-surface and parity. It is not a chain.
- Acceptance reads traced to a producer in the set: 14. Of those, 13 have an edge (direct or transitive through S0), and the argv read in `story:sandbox-repository-parity` has no producer at all.

**What you could not establish**
- Whether the adapter-specific scenarios in finding 4 are meant to live in the suite. The domain story's intro says "one scenario per promise and adapter", but its acceptance does not require them.
- Out of my lane, not counted in the verdict:
  - `story:oci-adapter` and `story:pod-adapter` write Docker and Kubernetes code and edit `docs/design/04-security-and-isolation.md`. That overlaps `story:docker-driver-entry-gate`, which is cited on design 04, and the gate story's "no Docker code before the gate" framing. This is a scope or parallel-safety matter.
  - The ADR lists S1 as a wire change and the epic deliberately defers it. This is a scope matter.
  - `mod.rs`, `gate.yml`, `delegated-lane.sh` and `CHANGELOG.md` are shared by several stories. This is a parallel-safety matter.

```findings
- file: .engineering/planning/story/sandbox-repository-parity.md
  line: 44
  category: design
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "its acceptance asserts flags are present in the linux and oci adapters' argv and --dry-run prints that argv, but no story in the set creates an argv-rendering surface (S0's trait has no render, oci's body none) and the parity scope excludes crates/substrate-host/src; S0 and story:oci-adapter need an acceptance line exposing a profile-to-argv render, or the parity scope must take those files"
- file: .engineering/planning/story/confinement-consumer-surface.md
  line: 61
  category: design
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "it defines the public surface of confinement that S0 already defines and exports, says it runs after the adapters so the surface it fixes is the one they implement, and declares depends_on only on S0; add depends_on edges to the five adapter stories or move the surface definition into S0"
- file: .engineering/planning/story/macos-seatbelt-policy-runs-cargo-test.md
  line: 39
  category: design
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "writable subtrees, sealed paths, executable subpaths and private TMPDIR are called profile fields of the macos adapter without saying whether they extend S0's ConfinementProfile and the domain Profile entity (neither in scope or edges) or stay SeatbeltPolicy-only and so unreachable by a consumer choosing by served_families"
- file: .engineering/planning/story/confinement-domain-specification.md
  line: 92
  category: design
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "the suite acceptance requires per-family scenarios but none per adapter (macos, systemd with its inner variants, oci, pod), while four adapter stories execute every scenario of their adapter and have no spec/ in their scope; require a scenario set per adapter here or move scenario authoring into the adapter stories"
- file: .engineering/planning/story/confinement-domain-specification.md
  line: 88
  category: design
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "the item holds two separable things, the new substrate.confinement domain with ADR 0044 and a repository-wide ESS pin move that regenerates every existing suite and the wire generated files; split the pin bump into its own story that the domain story depends on"
```
