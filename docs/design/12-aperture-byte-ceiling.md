# Design 12: an egress aperture carries a declared byte ceiling

**Status:** accepted as [ADR 0014](../../adr/0014-apertures-carry-a-declared-byte-ceiling.md) · **Date:** 2026-08-30

This document precedes the ADR that `story:aperture-byte-ceiling` names as its first evidence.
It fixes the ceiling's declaration surface, what it counts, where it is enforced and what the
confined child observes when it is crossed. Accepted by the operator on 2026-08-30 as ADR 0014.

## Context

[ADR 0013](../../adr/0013-egress-apertures-are-declared-by-the-operator.md) gave a deployment its
first outbound authority: a named aperture to one destination pinned at declaration. It bounds
**where** a confined process may reach and says nothing about **how much** may cross.
[Design 10](10-destination-bound-egress.md) § 5 row 5 already names the answer — class
`exhausted`, code `exec.aperture-byte-limit` (`docs/design/10-destination-bound-egress.md:126`) —
and that row is the one thing ADR 0013 shipped without: *"There is no declared byte ceiling in the
configuration surface, so there is nothing to exceed"* (`:284-287`).

Half the mechanism is built. The relay adds every chunk it moves to a counter in a page shared with
the daemon (`crates/substrate-host/src/egress.rs:818`), and every applied observation carries the
pair (`ApertureBytes`, `crates/substrate-wire/src/lib.rs:790-795`, read at
`crates/substrate-host/src/egress.rs:289-297`). Nothing compares either number to anything. An
operator can see after the run that forty gigabytes crossed; nothing stopped the fortieth.

## Decision

**One flag, one declaration.** The grammar becomes
`--egress-aperture <name>=<host>:<port>/tcp[/max=<size>]`. The value already carries a required
`/tcp` term whose stated purpose is to stop a later slice silently reinterpreting a declaration
written today (`crates/substrate-daemon/src/main.rs:30-33`); the ceiling is a second term in the
same place, and an unrecognised term is a startup error rather than an ignored one. `<size>` is a
decimal byte count with an optional binary suffix — `1048576`, `512KiB`, `64MiB`, `2GiB` — and never
a decimal-power unit, because a `MB` that means two things is an operator error waiting in a
configuration file. A comma is not available as a separator: it splits repeated declarations
(`crates/substrate-daemon/src/main.rs:116`). **An aperture declared without the term keeps working
byte for byte**: the ceiling is an `Option<u64>` beside the four fields that exist
(`crates/substrate-host/src/egress.rs:83-88`), `None` installs exactly what installs today, and no
field of that run's observation moves.

**One ceiling over both directions, summed.** The bound is on `to_destination + from_destination`
(`crates/substrate-wire/src/lib.rs:790-795`), so an operator states "this run may move 100 MiB" and
a child cannot evade the bound by choosing a direction. Two ceilings would be two numbers to get
wrong and a refusal that has to say which half tripped; one sum is what *what crossed this aperture*
means.

**Per run, never per aperture lifetime.** The counters are created by `install` for one run and
unmapped when it ends (`crates/substrate-host/src/egress.rs:322-330`, `:300-310`); there is no
daemon-level total and inventing one would make this run's refusal depend on the previous run's
traffic — not reproducible from its own request, and eventually a deployment that reaches nothing
for a reason no request explains. A ceiling that can be spent is not a bound, it is an outage with a
schedule.

**Enforced in the relay; classified by the parent.** The relay is the only thing on the byte path.
It already counts (`crates/substrate-host/src/egress.rs:818`) and it is a post-fork child that
allocates nothing (`crates/substrate-host/src/egress.rs:35-48`), so comparing a `u64` fixed before
the fork against two relaxed loads from the page it already writes adds no allocation and no lock.
When the running total reaches the ceiling the relay stops relaying and closes both halves of its
connection. The overshoot is bounded and statable rather than hoped for: at most one relay buffer —
16 KiB (`crates/substrate-host/src/egress.rs:68`) — per live relay, and the run's own `pids` bound
is what caps how many relays exist (`crates/substrate-host/src/egress.rs:649-651`). The parent's
supervision loop, which already polls at 1 ms and kills the whole tree for a CPU budget
(`crates/substrate-host/src/process.rs:1392-1412`), reads the same counters, ends the run, and is
where the refusal gets its name.

**What the parent alone would have cost.** It can stop no byte; it can only `cgroup.kill_all()`
(`crates/substrate-host/src/process.rs:1407`), which does reach the forwarder, since the forwarder
joins the run's cgroup before its first accept (`crates/substrate-host/src/egress.rs:669`). But what
crosses between two polls is bounded by the destination's throughput and by nothing substrate owns —
at 1 ms on a fast link, megabytes past a number the operator wrote down. It would also leave the
counters advisory: a total nobody reads until after the fact is a report, not a bound, and invariant
3 is exactly the difference between a named refusal and a quiet degradation. The relay pays one
comparison per chunk for a bound that is a bound.

**The refusal needs somewhere to live.** At HEAD a mid-run bound has no code at all: a timeout and a
CPU exhaustion both end the run as `ExecState::Cancelled`, indistinguishable there from a client
cancel (`crates/substrate-host/src/process.rs:1318-1319`, `:1360-1366`), and even a `wait: true`
start returns an observation rather than an error
(`crates/substrate-host/src/process.rs:563-569`). So the class and code design 10 § 5 names —
`exhausted`, `exec.aperture-byte-limit`, no address — are carried by one new optional field on the
exec observation, holding that triple beside the state that is already `Cancelled`. The byte ceiling
is its only user in this decision; naming timeout and CPU exhaustion in the same field is a later
change with its own vectors. No new event kind: the observation already rides `exec.*` and
`session.*` (design 10 § 6).

**What the child observes is nothing addressed to it.** Its connection to the loopback listener ends
mid-stream — EOF or a reset, and under TLS a truncated record rather than a protocol error naming a
limit — and the tree is then killed, so the child usually does not outlive its own socket. Nothing
tells it the ceiling, the remaining budget, or which bound it hit. That is the rule already taken
for the port: nothing tells the child a port and there is no aperture environment variable
(`docs/design/10-destination-bound-egress.md:274-275`). A budget the child can read is a budget the
child can plan around, and reach is not the child's to know. The operator gets the name; the child
gets a closed socket.

**A ceiling is deployment vocabulary, never request data.** It is declared where reach is declared
and may not appear in a request, at any depth, in any field — the same rule that gives
`exec.aperture-destination-in-request` (`docs/design/10-destination-bound-egress.md:124`).
`ConfinementRequest` is `deny_unknown_fields` and gains no ceiling field
(`crates/substrate-wire/src/lib.rs:646-661`), so a conforming client's ceiling is `schema-invalid`
first, and the typed refusal `exec.aperture-ceiling-in-request` — `refused`, at
`sandbox.network.aperture` — exists so a rejected escalation reads as one rather than as a schema
typo. A request asking for a *lower* ceiling than the deployment's is not widening, and is still a
second place where reach is decided: it is not this decision and it is not served.

**Published and observed.** `EgressApertureFact` gains an optional ceiling
(`crates/substrate-wire/src/lib.rs:1838-1841`) so `/v1/machine` answers "how much could this daemon
ever pass", and `AppliedAperture` (`crates/substrate-wire/src/lib.rs:760-769`) states the ceiling the
run actually ran under beside the bytes that crossed — reported, not inferred, the idiom ADR 0010
set. Successor bundle `0.8.0`: predecessor `0.7.0`, `adds_routes: 0`, `preserves_routes: 26`
(`contracts/substrate-wire/0.7.0/bundle.json:5-10`), with its own `cargo xtask check-bundle 0.8.0`
added to `scripts/gate.sh`, because a bundle whose check is not in the gate is unverified from the
next commit onward. Earlier directories keep their bytes (invariant 6). (This document said `0.7.0`
when it was drafted; that number went to ADR 0011's grant attribution while this one waited for
acceptance.)

## Consequences

A deployment can bound volume as well as destination, and the first bound substrate enforces on
somebody else's link is one an operator wrote down rather than one a run discovered. The cost is
stated rather than hidden: the observed total may exceed the declared ceiling by up to one relay
buffer per live connection, so a ceiling is a stop and not a quota with a hard edge, and a run
stopped at the ceiling loses whatever its child had not yet flushed. Ceilings do not compose across
runs, so a deployment that wants a fleet-wide volume bound still has to hold it somewhere else.

The positive half stays unprovable on a hosted runner, exactly as it is for ADR 0013: a child
reading past a declared ceiling from a pinned destination needs the delegated lane on a self-hosted
runner. CI proves the request-side refusal, the declaration grammar and the schema shape, and
reports the rest **absent rather than passed**.
