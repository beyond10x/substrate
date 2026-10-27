---
format: aep.planning-md/3
id: review-result:adr-0109-plan-acceptance-round-2
kind: review-result
status: active
title: Acceptance critic (aep:plan-critic-acceptance), ADR 0109 plan round 2
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
story:confinement-domain-specification — the suite floor demands an `adapter-unserved` scenario "for an absent platform or runtime" for every adapter including `local` and `macos`, but the runners execute "every `local` scenario" against the real adapter, and "every `macos` scenario" on the `macos-15` job, where the adapter always opens. A scenario that expects `adapter-unserved` cannot pass there, and `oci` makes it worse, since its lane fails the test on an absent runtime. The acceptance names no setup that makes the platform or runtime absent, nor which lane runs the scenario — .engineering/planning/story/confinement-domain-specification.md:89 (consumers: story/confinement-contract-on-linux-and-local.md:87, story/macos-adapter-on-seatbelt.md:47, story/oci-adapter.md:49)
story:confinement-consumer-surface — the first bullet has the consumer test cover loom's "toolchain root read-only" need and run "on `local` in the portable lane", but S0 says `local` refuses any profile naming a filesystem view with `confinement.family-unserved`. That case cannot pass in the portable lane, and the bullet does not say what the test does there instead — .engineering/planning/story/confinement-consumer-surface.md:47 (cites story/confinement-contract-on-linux-and-local.md:80)
story:macos-seatbelt-policy-runs-cargo-test — the "before" half of the first bullet says the three facts "are refused `confinement.fact-unserved`, which the S1 test asserts", but S1's acceptance asserts that refusal only for `whole_tree_kill`, and the domain suite floor names only `whole_tree_kill` for `macos`. No test asserts the "before" state this transition relies on — .engineering/planning/story/macos-seatbelt-policy-runs-cargo-test.md:47 (cites story/macos-adapter-on-seatbelt.md:51 and story/confinement-domain-specification.md:91)
story:pod-adapter — "A run that exceeds its memory bound ends `OOMKilled` and is reported as the bound's refusal" names no refusal code or observation, so two readers disagree on what "the bound's refusal" is. The domain's refusal list has no bound refusal, and the `oci` bullet at the same position names an observable (the fork refusal) — .engineering/planning/story/pod-adapter.md:61

What I read: 11 of 11 ids (`epic:confinement-contract-family`, the ten stories including `story:ess-pin-0-70-0`) via `aep plan artifact show <id>`, plus the rubric, the procedure file and `review-result:adr-0109-plan-acceptance-round-1`. I checked in the tree: the `requires: ess 0.69.2` lines, `gate.yml:127-128`, `exec_cgroup_limits`, and that `crates/substrate-wire/src/generated` exists.

Round 1's eleven findings did not recur. I checked the revisions:
- The domain suite floor now has a per-adapter minimum.
- `network_ingress` is excluded from the admission half of the floor.
- Each adversary or security-reviewer pass now names a `review-result` that `reviews:` the story, with a `review_outcome` on each finding.
- The consumer surface bullets and the `--net` behaviour are now stated.

What I could not establish:
- Whether ESS can express an "absent platform or runtime" setup for a scenario that runs against a real adapter. If it can, the first finding is a missing sentence and not a contradiction.
- Whether `ess` 0.70.0's `--strict-requires` passes on the new domain, and the `sandbox` and `atlas` citations, which are in other repositories.
- Out of my lane, not counted in the verdict: `epic:confinement-contract-family` § Delivery holds a stray bullet about plan 03's drivers that belongs under § Out of scope. Stories use 4-7 bullet acceptance lists, kept as house convention, as in round 1.

```findings
- file: .engineering/planning/story/confinement-domain-specification.md
  line: 89
  category: acceptance
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "the suite floor demands an adapter-unserved scenario \"for an absent platform or runtime\" for every adapter including local and macos, but the runners execute every local scenario against the real adapter and every macos scenario on the macos-15 job, where the adapter always opens, and the oci lane fails the test on an absent runtime; the acceptance names no setup that makes the platform or runtime absent or which lane runs the scenario"
- file: .engineering/planning/story/confinement-consumer-surface.md
  line: 47
  category: acceptance
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "the first bullet has the consumer test cover loom's toolchain-root-read-only need and run on local in the portable lane, but S0 says local refuses any profile naming a filesystem view with confinement.family-unserved, so that case cannot pass in the portable lane and the bullet does not say what the test does there"
- file: .engineering/planning/story/macos-seatbelt-policy-runs-cargo-test.md
  line: 47
  category: acceptance
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "the before state of the first bullet says the three facts are refused confinement.fact-unserved, which the S1 test asserts, but S1's acceptance and the domain suite floor assert that refusal only for whole_tree_kill, so no test asserts the before state the transition relies on"
- file: .engineering/planning/story/pod-adapter.md
  line: 61
  category: acceptance
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "\"A run that exceeds its memory bound ends OOMKilled and is reported as the bound's refusal\" names no refusal code or observation, and the domain's refusal list has no bound refusal, so two readers disagree on what is reported"
```
