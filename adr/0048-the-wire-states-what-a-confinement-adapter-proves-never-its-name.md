---
status: accepted
date: 2026-10-31
---

# ADR 0048: the wire states what a confinement adapter proves, never its name

Decider: operator.

Decides `story:confinement-adapter-on-the-wire`. **Accepted** by the operator on 2026-10-31 with
the recommended answer to each of the ten questions of
[design 30](../docs/design/30-confinement-adapter-on-the-wire.md) § 9, D1 to D10, closing
`decision-blocker:namespace-exec-needs-design-30` with its option A
(`approval-record:design-30-accepted`). The operator's answer, verbatim: "take decissions like the
above for me (A)". Design 30 moves to `accepted` with this record, and its design comes before code
(invariant 8).

## Context

- **The namespace driver cannot admit an exec.** Release 0.16.1 refuses every exec start on
  `--driver kubernetes` with `exec.sandbox-unavailable` (501). Admission
  (`crates/substrate-daemon/src/app/operations.rs`, `confinement_proved`) takes the four Linux
  `exec.*` facts or design 34 § 6.4's neutral set: `confinement.family.process-run`, `.wait`,
  `.output-read`, `.limits`, `confinement.whole-tree-kill` and `confinement.no-egress`. A namespace
  cannot state the Linux facts, and bundle `0.25.0` carries neither the families nor whole-tree
  kill (design 34 § 11).
- **The families are design 30's**, which stood `proposed`. Invariant 8 held code until its § 9
  questions were answered and, under its D8, an ADR recorded them.
- **Earlier records assumed a `HostDriverKind` change.**
  [ADR 0044](0044-confinement-is-a-contract-family-served-by-declared-adapters.md) § *Consequences*
  says a wire publication of adapters or facts "is a `HostDriverKind` change and needs a successor
  bundle". Atlas ADR 0109 decision 2 is titled "Facts, not names" and states the same expectation
  about the wire. Design 30 § 1 shows that publishing what an adapter proves needs no driver kind.

## Decision

The answers, each the recommendation of design 30 § 9:

1. **D1 A — facts only.** The wire publishes the families and mechanism-neutral properties the
   serving adapter proved. No adapter, mechanism or deployment-profile name appears on the client
   wire. `driver` keeps its meaning (the daemon's one driver contract); a confinement adapter is
   never a `driver` value.
2. **D2 — one boolean fact per family**, `confinement.family.<name>` in ADR 0044 decision 1's
   order, `true` when proved and absent otherwise, never written `false`.
3. **D3 — new names beside the frozen Linux facts.** The properties are `confinement.<name>` facts
   from the `Fact` type of `spec/confinement/`. `exec.namespaces`, `exec.cgroup-limits`,
   `exec.cgroup-kill` and `exec.no-egress` keep their bytes and meanings.
4. **D4 — a start carries `sandbox.requires` and the daemon checks it.** The closed object names
   families and properties without their prefixes. It is checked after `exec.capability-stale` and
   before every field-specific check; the first unserved family is `confinement.family-unserved`
   (501), the first unproved property `confinement.fact-unserved` (501), each recorded as refused
   before dispatch. An unknown name, a duplicate or an empty object is `request.schema-invalid`
   (422). The applied record echoes `requires` when the start carried it.
5. **D5 — `AppliedConfinement.cgroup` becomes optional now**, present exactly when an adapter
   applied a cgroup. No value is invented.
6. **D6 — the exec and session routes never serve `local`.** The declared-unconfined path stays the
   host-process family (ADR 0036). Neither `oci` nor `pod` reaches those routes (ADR 0044
   decision 8).
7. **D7 — the adapter is visible to the operator only**, as `substrate_adapter_info{adapter}`, a
   gauge that is always 1, on ADR 0045's operational listener. Its label takes the generated
   `AdapterName` values the daemon can open. This adds one family to ADR 0045's closed catalogue
   and changes no client contract.
8. **D8 — this ADR records the acceptance.** It narrows ADR 0044's consequence quoted above: a wire
   publication of what an adapter proves is a successor bundle of facts, not a `HostDriverKind`
   change. Atlas is told in this text; no atlas file changes here.
9. **D9 — later wire drivers get `driver` values in their own entry-gate stories.** ADR 0047
   N7 A already did so for `kubernetes`. This ADR fixes only that a confinement adapter is never
   one.
10. **D10 — the bundle is the next free version when the story starts.** `0.25.0` is taken by
    design 34 § 11, so design 30's bundle is **`0.26.0`**. It adds no route.

The snapshot digest binds the opened adapter's name and its proved families and properties
(design 30 § 4.6), so a daemon restarted on another adapter publishes a new snapshot and a start
naming the old one is `exec.capability-stale`.

## Consequences

- `story:confinement-adapter-on-the-wire` cuts `contracts/substrate-wire/0.26.0`: the optional
  family and property facts, the optional `sandbox.requires` on both starts, the optional
  `requires` and optional `cgroup` on the applied record, and the two refusals, each with a vector
  held to its `substrate_wire` constant. The daemon and SDK advertise it. Every released bundle
  stays byte for byte.
- The host driver publishes `linux`'s expected set (design 30 § 4.3). Its answers to a start
  without `requires` do not change.
- A namespace driver that observes the six facts of design 34 § 6.4 admits an exec on them.
  `confinement.no-egress` stays observed, never configured: on a cluster whose network plugin does
  not enforce NetworkPolicy the fact stays absent and an exec stays `exec.sandbox-unavailable`
  (invariant 3).
- Consumers that read facts as an open map (harness) keep working against a newer daemon; the
  Rust SDK, as for every successor, refuses a daemon advertising another bundle until its pin
  moves. No consumer repository changes here.
- The macOS daemon (ADR 0037 step 2) needs no further wire change: its facts and its applied record
  already fit.

## What this does not decide

- The macOS daemon itself, and a `b10x-substrate` CLI flag for `requires`.
- The reserved ingress-route and rate-bound fields (ADR 0044 decision 5); `network-ingress` stays
  unpublished.
- Which network plugin a deployment runs. The namespace lane picks one that enforces
  NetworkPolicy so that the observation can hold; a deployment's choice is the operator's.
