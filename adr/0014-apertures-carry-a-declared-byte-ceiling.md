---
status: accepted
date: 2026-08-30
---

# ADR 0014: an egress aperture carries a declared byte ceiling

## Context

[ADR 0013](0013-egress-apertures-are-declared-by-the-operator.md) gave a deployment its first
outbound authority: a named aperture to one destination, pinned at declaration. It bounds **where**
a confined process may reach and says nothing about **how much** may cross. A confined vendor
harness with a model credential and a pinned endpoint can move bytes until the run ends.

The answer was already named and could not be raised.
[Design 10](../docs/design/10-destination-bound-egress.md) § 5 row 5 fixes class `exhausted` and
code `exec.aperture-byte-limit`, and ADR 0013 shipped that row as a name with nothing behind it:
*"There is no declared byte ceiling in the configuration surface, so there is nothing to exceed"*
(`docs/design/10-destination-bound-egress.md:284-287`). Half the mechanism exists — the relay counts
what crosses and every applied observation carries it. The declaration, the enforcement and the
refusal do not.

## Decision

**The grammar gains one optional term.**
`--egress-aperture <name>=<host>:<port>/tcp[/max=<size>]`. The value already carries a required
`/tcp` term whose purpose is to stop a later slice silently reinterpreting a declaration written
today; the ceiling is a second term in the same place, and an unrecognised term is a startup error
rather than an ignored one. `<size>` is a decimal byte count with an optional binary suffix and
never a decimal-power unit. **An aperture declared without the term keeps working byte for byte.**

**One ceiling over both directions, summed.** An operator states "this run may move 100 MiB"; a
child cannot evade the bound by choosing a direction. Two ceilings would be two numbers to get
wrong and a refusal that has to say which half tripped.

**Per run, never per aperture lifetime.** A ceiling that can be spent is not a bound, it is an
outage with a schedule — and a refusal that depends on the previous run's traffic is not
reproducible from its own request.

**Enforced in the relay, classified by the parent.** The relay is the only thing on the byte path;
it already counts and it allocates nothing after the fork, so it stops relaying at the ceiling. The
overshoot is stated rather than hoped for: at most one relay buffer, 16 KiB, per live relay. The
parent's supervision loop, which already polls at 1 ms and kills the tree for a CPU budget, reads
the same counters, ends the run and names the refusal. Parent-only enforcement stops no byte — what
crosses between two polls is bounded by the destination's throughput and by nothing substrate owns,
and counters nobody reads until after the fact are a report, not a bound.

**The refusal gets somewhere to live.** At HEAD a mid-run bound has no code: a timeout and a CPU
exhaustion both end the run as `ExecState::Cancelled`, indistinguishable there from a client cancel.
One new optional field on the exec observation carries the class, code and message beside that
state. The byte ceiling is its only user here; naming timeout and CPU exhaustion in the same field
is a later change with its own vectors.

**The child is told nothing.** Its connection ends mid-stream and the tree is killed. Nothing tells
it the ceiling, the remaining budget or which bound it hit — the rule already taken for the port. A
budget the child can read is a budget the child can plan around, and reach is not the child's to
know. The operator gets the name; the child gets a closed socket.

**A ceiling is deployment vocabulary, never request data.** It may not appear in a request, at any
depth, in any field — the rule that gives `exec.aperture-destination-in-request`. The typed refusal
`exec.aperture-ceiling-in-request` exists so a rejected escalation reads as one rather than as a
schema typo.

**Published and observed, not inferred.** The aperture capability fact gains an optional ceiling, so
`/v1/machine` answers *how much could this daemon ever pass*, and the applied observation states the
ceiling the run actually ran under beside the bytes that crossed. Successor bundle `0.8.0`, with its
own `cargo xtask check-bundle 0.8.0` in the gate; earlier directories keep their bytes
(invariant 6).

## Consequences

A deployment can bound volume as well as destination, and the first bound substrate enforces on
somebody else's link is one an operator wrote down rather than one a run discovered.

The cost is stated rather than hidden. The observed total may exceed the declared ceiling by up to
one relay buffer per live connection, so a ceiling is a stop and not a quota with a hard edge, and a
run stopped at the ceiling loses whatever its child had not yet flushed. Ceilings do not compose
across runs, so a fleet-wide volume bound still has to live somewhere else.

The positive half stays unprovable on a hosted runner, exactly as for ADR 0013: a child reading past
a declared ceiling needs the delegated lane on a self-hosted runner. CI proves the request-side
refusal, the declaration grammar and the schema shape, and reports the rest **absent rather than
passed** (invariant 3).

The full reasoning, with every `file:line` the decision was checked against, is
[design 12](../docs/design/12-aperture-byte-ceiling.md).
