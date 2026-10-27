---
format: aep.planning-md/3
id: review-result:adr-0109-plan-design-round-2
kind: review-result
status: active
title: Design critic (aep:plan-critic-design), ADR 0109 plan round 2
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
story:macos-seatbelt-policy-runs-cargo-test — its acceptance says that before this story `sealed_paths`, `exec_subpaths` and `private_tmpdir` are refused `confinement.fact-unserved` and that "the S1 test asserts" this, but S1's acceptance asserts no such refusal. If S1 did assert it, that test would fail once this story proves the facts, which contradicts "the S1 scenarios … still pass". The macos fact set changes between S1 and S2 while the suite is fixed by the domain story. The body must say which side holds: either drop the claim, or the domain story declares the macos set in its final state and the S1 and S2 scenario sets are split by name. — .engineering/planning/story/macos-seatbelt-policy-runs-cargo-test.md:46-47, .engineering/planning/story/macos-adapter-on-seatbelt.md:50-52
story:confinement-contract-on-linux-and-local — the `systemd` story opens `linux` "with the scope's cgroup as its delegated root" and takes an `inner` of `local` or `linux`. Its scope excludes `linux.rs` and `mod.rs`, and S0 says adapter stories "fill one file each and do not edit `mod.rs`". S0's contract says nothing about composing one adapter over another: no `inner` in `open`, and no way to hand `linux` a delegated cgroup root (`HostConfig.cgroup_root` exists at `lib.rs:268`). S0's body needs to state that composition surface, or the `systemd` story must take those files into scope. — .engineering/planning/story/confinement-contract-on-linux-and-local.md:70-72, .engineering/planning/story/systemd-scope-bounds-provider.md:40

**What you read**
- 12 artifacts: the epic, the ten stories and review-result round 1.
- Commands: `aep plan artifact show` for each, `relations`, `graph` and `validate` (printed `valid`). I also read ADR 0109 from atlas `origin/main` and `HostConfig` in `crates/substrate-host/src/lib.rs`.
- Edges: 18 in-set edges walked (10 `decomposes`, 11 `depends_on`, 2 `informed_by`), plus the outside edges of the epic and review-results; no cycle.
- Shape: a diamond (pin, domain, S0, then macos→S2, systemd and oci→pod in parallel, then consumer-surface, then parity). The serial tails each carry a written reason: shared `gate.yml` for oci→pod, the frozen surface for consumer-surface and parity.
- Acceptance reads traced: about 17 to producers in the set. 15 are recorded by an edge, directly or through S0. The two that are not are the findings above.
- Round 1's five findings are all resolved:
  - the argv `render` is now in S0 and oci;
  - consumer-surface has `depends_on` edges to the adapters;
  - the profile fields stay in S0 (the macos fact-state point above is new);
  - per-adapter scenarios are in the domain acceptance;
  - the pin is its own story.

**What you could not establish**
- Out of my lane, and they do not set the verdict:
  - The domain acceptance requires `confinement.family-unserved` for "each of the ten families that some adapter does not serve", but no adapter appears to omit `process_run`, `process_watch`, `output_read` or `wait`. That makes the acceptance unsatisfiable as written, and it belongs to the acceptance critic.
  - The epic body cites `approval-record:adr-0109-confinement-plan`, which does not exist in the store (`aep plan artifact show` errors).
  - The epic's plan-03 out-of-scope bullets now sit under `## Delivery`, spliced after the operator quote, not under `## Out of scope`. This is a body-writing matter.
- Whether the macos scenarios in the domain suite exercise `sealed_paths`, `exec_subpaths` or `private_tmpdir` at all. The domain acceptance does not say, which is why the first finding is stated as an either/or.

```findings
- file: .engineering/planning/story/macos-seatbelt-policy-runs-cargo-test.md
  line: 46
  category: design
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "it says sealed_paths, exec_subpaths and private_tmpdir are refused confinement.fact-unserved before this story and that the S1 test asserts it, but S1's acceptance asserts no such refusal, and if it did that assertion would fail once this story proves the facts, contradicting 'the S1 scenarios still pass'; the macos fact set moves between S1 and S2 while the suite is fixed by the domain story, so the body must drop the claim or the domain story must declare the final macos set with S1 and S2 scenarios split by name"
- file: .engineering/planning/story/confinement-contract-on-linux-and-local.md
  line: 70
  category: design
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "the systemd story opens linux with the scope's cgroup as its delegated root and takes an inner of local or linux without editing linux.rs or mod.rs, but S0's contract states no composition surface (no inner in open, no way to give linux a delegated cgroup root); S0 must state it or the systemd story must take those files into scope"
```
