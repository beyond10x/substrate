---
format: aep.planning-md/3
id: review-result:adr-0109-plan-acceptance-round-1
kind: review-result
status: active
title: Acceptance critic (aep:plan-critic-acceptance), ADR 0109 plan round 1
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
story:confinement-domain-specification — the suite floor says "at least … for each of the ten families, one admission on an adapter that serves it", which no adapter can meet for `network_ingress` (S0 says no adapter serves it), and the "(where one exists)" qualifier covers only the refusal half — .engineering/planning/story/confinement-domain-specification.md:92
story:confinement-domain-specification — the floor requires no scenario for any of `macos`, `systemd`, `oci` or `pod`, so the stories that "execute every `<adapter>` scenario of the suite" pass with zero scenarios; the acceptance names no per-adapter minimum — .engineering/planning/story/confinement-domain-specification.md:92 (consumers: story/oci-adapter.md:49, story/pod-adapter.md:56, story/systemd-scope-bounds-provider.md:47)
story:macos-adapter-on-seatbelt — "Adversary pass plus security-reviewer pass (confinement)." states no outcome and no place the result is recorded, so it is true whenever someone says it is — .engineering/planning/story/macos-adapter-on-seatbelt.md:54
story:macos-seatbelt-policy-runs-cargo-test — "Adversary pass plus security-reviewer pass (confinement)." states no outcome and no record, so it cannot be checked — .engineering/planning/story/macos-seatbelt-policy-runs-cargo-test.md:49
story:systemd-scope-bounds-provider — "Adversary pass plus security-reviewer pass (confinement, cgroup delegation)." states no outcome and no record, so it cannot be checked — .engineering/planning/story/systemd-scope-bounds-provider.md:49
story:oci-adapter — "Adversary pass plus security-reviewer pass (confinement; the Docker socket is root-equivalent …)" states no outcome and no record, so it cannot be checked — .engineering/planning/story/oci-adapter.md:59
story:pod-adapter — "Adversary pass plus security-reviewer pass (cluster credentials, confinement)." states no outcome and no record, so it cannot be checked — .engineering/planning/story/pod-adapter.md:63
story:macos-adapter-on-seatbelt — "No claim of a local macOS run anywhere in the change; the only macOS evidence is the job's log" is a negative nobody can check by looking at an output, a record or an exit status — .engineering/planning/story/macos-adapter-on-seatbelt.md:53
story:macos-seatbelt-policy-runs-cargo-test — the first bullet says `SeatbeltPolicy` "gains" writable subtrees, sealed paths, executable subpaths and a private `TMPDIR`, with `exec.sandbox-path-unrepresentable` refusals, but no case observes the private `TMPDIR` or that refusal, so the bullet reads the same before the work and after — .engineering/planning/story/macos-seatbelt-policy-runs-cargo-test.md:39-41
story:confinement-consumer-surface — the first bullet ("exports one module, `confinement`, whose public items are the profile, the adapter request, `open`, the facts … run handles") is already true once S0 lands, because S0's scope creates `confinement/mod.rs` and its `lib.rs` exports, so the acceptance names no state before this story and no change it makes — .engineering/planning/story/confinement-consumer-surface.md:37
story:sandbox-repository-parity — "`--net` under `linux` is admitted only as `network_egress` with no aperture refused, or `local` with a reason" does not say whether `confine run --backend linux --net` succeeds or is refused, or what the verb prints, so two readers get different answers — .engineering/planning/story/sandbox-repository-parity.md:53

What I read: 10 of 10 ids (the epic and nine stories) with `aep plan artifact show <id>` for each, plus `aep plan artifact kinds` and `lifecycle story|epic`. I checked the named facts in the tree:
- `PORTABLE_CASES` 118 and `DELEGATED_CASES` 180 (`crates/substrate-daemon/tests/runtime_vectors.rs:7110-7111`).
- Seven `seatbelt_adversary` tests.
- `exec.sandbox-path-unrepresentable` (`crates/substrate-host/src/seatbelt.rs:30`).
- `SUBSTRATE_VECTORS_CGROUP_ROOT`, and `macos.yml`.
- `adr/` ends at 0043 and `docs/design/` at 27, so ADR 0044 and design 28 are free numbers.
- ESS 0.70.0 is the latest release, via `gh release list -R beyond10x/ess`; `gate.yml:127` pins 0.69.2.
- `aep plan artifact validate` reports valid.

What I could not establish:
- Whether an adversary or security-reviewer pass is recorded as a `review-result` in this repository. The tree gave no convention, so the five "pass" findings assume that the acceptance should name the record.
- The `sandbox` and `atlas` line citations (`src/docker.rs:42-130`, `1b0377d`) are in other repositories, so I did not check them.
- Every story's acceptance is a 4–6 bullet list rather than the "one acceptance statement per story" of `SKILL.md:456`. I did not file this, because the implemented `story:macos-seatbelt-confinement` uses the same bullet style (house convention).
- Out of my lane, not counted in the verdict:
  - `story:confinement-consumer-surface` says it "runs after the wave holding the adapters" but only declares `depends_on` S0.
  - S0 and the consumer surface both own `confinement/mod.rs` and `lib.rs` exports, which is parallel-safety.
  - The domain story widens the ESS pin across every `spec/*` suite, which is parallel-safety.

```findings
- file: .engineering/planning/story/confinement-domain-specification.md
  line: 92
  category: acceptance
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "the suite floor says \"at least … for each of the ten families, one admission on an adapter that serves it\", which no adapter can meet for network_ingress (S0 says no adapter serves it), and the \"(where one exists)\" qualifier covers only the refusal half"
- file: .engineering/planning/story/confinement-domain-specification.md
  line: 92
  category: acceptance
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "the floor requires no scenario for any of macos, systemd, oci or pod, so the stories that execute every <adapter> scenario of the suite pass with zero scenarios; the acceptance names no per-adapter minimum"
- file: .engineering/planning/story/macos-adapter-on-seatbelt.md
  line: 54
  category: acceptance
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "\"Adversary pass plus security-reviewer pass (confinement).\" states no outcome and no place the result is recorded, so it is true whenever someone says it is"
- file: .engineering/planning/story/macos-seatbelt-policy-runs-cargo-test.md
  line: 49
  category: acceptance
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "\"Adversary pass plus security-reviewer pass (confinement).\" states no outcome and no record, so it cannot be checked"
- file: .engineering/planning/story/systemd-scope-bounds-provider.md
  line: 49
  category: acceptance
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "\"Adversary pass plus security-reviewer pass (confinement, cgroup delegation).\" states no outcome and no record, so it cannot be checked"
- file: .engineering/planning/story/oci-adapter.md
  line: 59
  category: acceptance
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "\"Adversary pass plus security-reviewer pass (confinement; the Docker socket is root-equivalent …)\" states no outcome and no record, so it cannot be checked"
- file: .engineering/planning/story/pod-adapter.md
  line: 63
  category: acceptance
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "\"Adversary pass plus security-reviewer pass (cluster credentials, confinement).\" states no outcome and no record, so it cannot be checked"
- file: .engineering/planning/story/macos-adapter-on-seatbelt.md
  line: 53
  category: acceptance
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "\"No claim of a local macOS run anywhere in the change; the only macOS evidence is the job's log\" is a negative nobody can check by looking at an output, a record or an exit status"
- file: .engineering/planning/story/macos-seatbelt-policy-runs-cargo-test.md
  line: 39
  category: acceptance
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "the first bullet says SeatbeltPolicy gains writable subtrees, sealed paths, executable subpaths and a private TMPDIR with exec.sandbox-path-unrepresentable refusals, but no case observes the private TMPDIR or that refusal, so the bullet reads the same before the work and after"
- file: .engineering/planning/story/confinement-consumer-surface.md
  line: 37
  category: acceptance
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "the first bullet (exports one module confinement whose public items are the profile, the adapter request, open, the facts, run handles) is already true once S0 lands, so the acceptance names no state before this story and no change it makes"
- file: .engineering/planning/story/sandbox-repository-parity.md
  line: 53
  category: acceptance
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "\"--net under linux is admitted only as network_egress with no aperture refused, or local with a reason\" does not say whether confine run --backend linux --net succeeds or is refused, or what the verb prints, so two readers get different answers"
```
