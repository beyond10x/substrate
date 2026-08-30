---
format: aep.planning-md/1
id: story:aperture-byte-ceiling
kind: story
status: draft
title: A declared aperture carries a byte ceiling that refuses mid-run
summary: Design 10 § 5 row 5 names exec.aperture-byte-limit; the counter exists, the ceiling does not.
relations:
- decomposes: epic:byte-plane-completion
- depends_on: story:destination-bound-egress
revision: 3
---
# Story: A declared aperture carries a byte ceiling that refuses mid-run

## Outcome

An operator who declares an aperture can bound what crosses it, and a run that exceeds the bound is
stopped with a named refusal rather than left to drain the link. Today the bytes are counted and
reported but nothing reads them, so a confined run's egress volume is observable after the fact and
unbounded during it.

## Context

`docs/design/10-destination-bound-egress.md:126` names the condition and its answer:
*Declared byte ceiling exceeded mid-run* → class `exhausted`, code `exec.aperture-byte-limit`, no
address. `:284-287` records why it did not ship with ADR 0013: *"There is no declared byte ceiling
in the configuration surface, so there is nothing to exceed."*

Verified in the tree, not inferred — the half that exists:

- `ApertureBytes { to_destination, from_destination }`, `crates/substrate-wire/src/lib.rs:790-795`.
- The forwarder holds both counters as `AtomicU64` and the parent reads them without synchronising,
  `crates/substrate-host/src/egress.rs:621-622` and `:252-256`.
- Every applied observation already carries them, `crates/substrate-host/src/egress.rs:288-297`.

The half that does not:

- `EgressAperture { name, host, port, pinned }`, `crates/substrate-host/src/egress.rs:83-88` — four
  fields, no ceiling.
- `--egress-aperture name=host:port/tcp`, `crates/substrate-daemon/src/main.rs:108`, parsed at
  `:34-56` — the grammar has nowhere to put one.
- Nothing reads `counters` for a comparison; the only readers are `applied()` and the tests.

The refusal class already exists: `Exhausted`, `crates/substrate-wire/src/lib.rs:132`.

## Acceptance

An aperture declared with a ceiling refuses the run by name when the ceiling is crossed, and the
same aperture declared without one behaves exactly as it does today.

Evidence that satisfies it, in order:

1. An ADR fixing the declaration grammar, which direction(s) the ceiling counts, whether it is
   per-run or per-aperture-lifetime, and what the child observes at the moment of refusal
   (invariant 8: design before code).
2. A successor bundle carrying the ceiling on the aperture capability fact and the
   `exec.aperture-byte-limit` refusal class; earlier bundle bytes unchanged (invariant 6).
3. A delegated-lane vector: a child that reads more than the declared ceiling from the pinned
   destination ends `exhausted` with code `exec.aperture-byte-limit`, and the applied observation
   reports at least the ceiling in `bytes`.
4. A delegated-lane vector proving the negative: an aperture declared with no ceiling passes the
   same traffic to completion, so the existing `declared-aperture-is-reachable` case
   (`docs/design/10-destination-bound-egress.md:161`) is unchanged.
5. A vector proving the ceiling is a *deployment* declaration and not request data — a request that
   carries a ceiling is refused, the same way a raw destination is
   (`exec.aperture-destination-in-request`, `docs/design/10-destination-bound-egress.md:124`).

## Out of Scope

Rate limits, time ceilings, and any ceiling on the `none` network mode — there is no forwarder there
to count in.

## Open

Whether the ceiling is enforced in the forwarder (it holds the counters, and can stop reading) or by
the parent watching the atomics (it can end the operation with the right class but cannot stop the
bytes already in flight). The ADR decides; both are consistent with the counters as they stand.

## Design draft — 2026-08-30

`docs/design/12-aperture-byte-ceiling.md`, **proposed**. No ADR number is claimed: `adr/` admits
`accepted` and `superseded` only (`xtask/src/adrs.rs:12`), so the number is assigned at acceptance.
Design 10's deferred note now points at it instead of describing an open gap.

Decisions it fixes, each with what the alternative would have cost:

| Decision | Chosen | Alternative's cost |
|---|---|---|
| Grammar | optional `/max=<size>` term on `--egress-aperture name=host:port/tcp`; absent = today byte for byte | a separate flag lets a ceiling name an aperture that does not exist; comma is unavailable either way (`crates/substrate-daemon/src/main.rs:116`, `value_delimiter = ','`) |
| Direction | one ceiling over `to_destination + from_destination` summed | a per-direction pair is two numbers to get wrong and a bound a child evades by picking a direction |
| Scope | per run | per-lifetime needs durable cross-run accounting and makes a refusal unreproducible from its own request |
| Enforcement | in the relay, which stops at the ceiling; overshoot bounded by `RELAY_BUFFER = 16_384` (`crates/substrate-host/src/egress.rs:68`) | parent-only stops no byte: overshoot is bounded by the destination's throughput, not by substrate |

**Gap the draft found, verified not inferred.** At HEAD a mid-run bound has no code to report. Both
existing mid-run bounds fold into one state: `forced_cancellation = timed_out || cpu_exhausted ||
cancellation_requested` (`crates/substrate-host/src/process.rs:1318-1319`) sets
`ExecState::Cancelled` (`:1364`). The supervision loop that would notice a ceiling already exists
and already polls at 1 ms (`:1392`, `tokio::time::interval(Duration::from_millis(1))`), so the
mechanism is there and the vocabulary is not — `exhausted` / `exec.aperture-byte-limit`
(`docs/design/10-destination-bound-egress.md:126`) has nowhere to live in today's observation. The
draft therefore also proposes an optional class/code/message field on the exec observation, which is
what makes this a contract change rather than a host-local one.

## Blocked on

Operator acceptance of design 12. Invariant 8: an ADR before code. Nothing here is implementable
until that decision is made.
