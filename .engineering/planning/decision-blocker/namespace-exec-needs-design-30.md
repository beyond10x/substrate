---
format: aep.planning-md/3
id: decision-blocker:namespace-exec-needs-design-30
kind: decision-blocker
status: open
title: Accept design 30, so the namespace driver can admit an exec
relations:
- blocks: story:confinement-adapter-on-the-wire
revision: 1
---
# Decision blocker: accept design 30, so the namespace driver can admit an exec

Decider: operator

## Question

0.16.1 refuses every exec on `--driver kubernetes` with `exec.sandbox-unavailable` (501)
(`CHANGELOG.md` § 0.16.1). A downstream adopter is blocked on it. The refusal is by design, not a
defect:

- Admission (`confinement_proved`, `crates/substrate-daemon/src/app/operations.rs`) takes the four
  Linux `exec.*` facts or design 34 § 6.4's neutral set: `confinement.family.process-run`,
  `.wait`, `.output-read`, `.limits`, `confinement.whole-tree-kill` and `confinement.no-egress`.
- A namespace cannot state the Linux facts, and bundle `0.25.0` carries none of the families nor
  whole-tree kill (design 34 § 11: "U10 cannot admit an exec on the neutral set before that
  bundle"). The families are design 30's, which is `proposed`; invariant 8 holds code until its
  § 9 decisions D1 to D10 are taken and, under its D8, an ADR records them.
- `confinement.no-egress` is also unobserved: O3 did not hold on the kind lane (kindnet does not
  enforce NetworkPolicy), so the probe never publishes it
  (`crates/substrate-kubernetes/src/namespace/probe.rs`, module comment).

| option | what | cost |
|---|---|---|
| **A** | accept design 30 with every recommended answer (D1 A facts only, D2 one boolean per family, D3 new names, D4 daemon-checked `sandbox.requires`, D5 optional `cgroup` now, D6 never `local`, D7 operational listener only, D8 an ADR, D9 per entry-gate story, D10 next free bundle); then `story:confinement-adapter-on-the-wire` cuts the successor bundle, and a namespace-driver story publishes the families, whole-tree kill and an observed no-egress, with the lane installing a pinned enforcing CNI | one ADR and one bundle; exec on a cluster is served where its CNI enforces NetworkPolicy, and refused by name where it does not |
| **B** | a namespace-only successor bundle: the six neutral facts, no `sandbox.requires`, no applied-record change | still a contract change with its own design and ADR; design 30's client-side choice and the macOS daemon's wire fit are left for a second bundle |
| **C** | keep exec refused on the namespace driver; adopters run the host driver on a node with bubblewrap and a delegated cgroup v2 subtree | no change here; the adopter's cluster path stays blocked |

Recommendation: **A**. It is the design the accepted design 34 § 6.4 (N6 A) already admits on;
B builds a second shape for the same facts, and C leaves the blocker in place.

## What follows A

1. ADR (next free number) recording design 30's answers; design 30 moves to accepted.
2. `story:confinement-adapter-on-the-wire` (draft) to proposed, then a wave: the successor bundle,
   daemon and SDK advertisement, portable and delegated lane cases.
3. A new story: the namespace probe publishes the four families, `confinement.whole-tree-kill`
   and `confinement.no-egress` from observation (`egress_probe_address` is already in the driver
   config), and the namespace lane's exec cases move from absent to executed on a kind cluster with
   an enforcing CNI.

Filed 2026-10-30 by the substrate wave session (wave 2026-10-30), because no proposed story in this
store covers the exec refusal.
