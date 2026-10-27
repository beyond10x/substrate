---
format: aep.planning-md/3
id: approval-record:adr-0109-delivery-rule-8
kind: approval-record
status: active
title: Reviews never hold a merge into the ADR 0109 integration branch
tags:
- adr-0109
- non-interactive
relations:
- decides: story:confinement-contract-on-linux-and-local
- decides: story:macos-adapter-on-seatbelt
- decides: story:systemd-scope-bounds-provider
- decides: story:oci-adapter
- decides: story:pod-adapter
- decides: story:macos-seatbelt-policy-runs-cargo-test
- decides: story:confinement-consumer-surface
- decides: story:sandbox-repository-parity
revision: 2
transitions:
- {from: "draft", to: "active", at: "2026-10-27T02:26:49Z", actor: "human:timo", revision: 2}
---
# Delivery order changed: reviews never hold a merge into the integration branch

Received 2026-10-27 from the coordinator's process brief, rule 8, set by the operator: "make
delivery faster".

Rule 8, quoted: "Reviews never hold a merge into the integration branch. A worker commits in its
tree as soon as its tests and clippy are green; no brief says \"commit nothing\". The unit merges
into the integration branch at green. Its adversary and security-reviewer passes then run in
parallel, against the integration branch, while the next units build; each finding lands there as a
fix commit or a story before the PR opens. The PR to `main` is the gate. A unit starts the moment
the units it `depends_on` are merged into the integration branch, never after their reviews; code
units do not wait for a spec unit's review."

Rule 4, quoted: "No adversary on a pin-only change."

## What changes for this epic

- `story:confinement-contract-on-linux-and-local` merges into `integration/adr-0109-confinement`
  at green tests and clippy; `story:macos-adapter-on-seatbelt`,
  `story:systemd-scope-bounds-provider` and `story:oci-adapter` start the moment it is merged, in
  parallel, without waiting for its adversary and security-reviewer passes.
- `story:pod-adapter` starts when `story:oci-adapter` is merged;
  `story:macos-seatbelt-policy-runs-cargo-test` when `story:macos-adapter-on-seatbelt` is merged.
- Adversary and security-reviewer passes on code units run against the integration branch; each
  finding lands there as a fix commit or a filed story before the single pull request opens.
- The spec unit's fix pass for its adversary findings lands as a fix commit on the integration
  branch; the domain was pushed at `a086e93` before its review completed.
- The one pull request to `main` is the gate.
