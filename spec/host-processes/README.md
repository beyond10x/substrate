# Host processes

The ESS domain `substrate.host_processes` specifies the resource family of
[ADR 0036](../../adr/0036-host-processes-are-a-supervised-resource-family.md): the *host process*
(supervised process), a long-lived process tree that a session starts through the daemon, with an
owner, a reason, a lease or a reasoned keep, an optional hard deadline, one cgroup, observations, a
bounded log and an event stream. Under the `host` profile the process runs unconfined, and the
record says so (`confinement: none`).

No route, wire bundle or generated type is derived from this domain yet; the successor bundle that
carries the family is cut from it. `cargo xtask check-ess` validates it, compares the committed
synthesized suite [`../host-processes-suite.json`](../host-processes-suite.json) byte for byte, and
runs the synthesized and authored scenarios against the ESS interpreted reference.

```console
ess specify validate --path spec/host-processes --strict-requires
ess verify conform synthesize --path spec/host-processes --strict-requires \
  --out spec/host-processes-suite.json
ess verify conform run --path spec/host-processes --scenarios spec/host-processes \
  --target interpreted --report-format 2 --strict-requires
```

The pin is ESS 0.60.0 (`ess-inputs.yaml`). The authored scenarios under `scenarios/` are the
acceptance set, one or more per command. A run against the interpreted reference checks the model
only. It is not evidence that any implementation conforms.

## How the open points were settled

ADR 0036 left sixteen points for the specification. The wave decision of 2026-10-17 fixes one
option for each, the one that can be undone, and each is a `DECIDED (point n)` comment beside the
place it governs:

- **Modelled:** stream delivery at least once with a per-process cursor (1), refused appends
  dropped (2), the subject as the only authority, with the daemon's supervisor as its own actor (6),
  owner-name uniqueness among non-terminal records of one owner session (7), cumulative CPU at
  observation time (13), the byte-bounded log that discards its oldest bytes and records how many
  (14), and an append for an unknown process refused inside the daemon (15).
- **Implementation-side, because ESS 0.60.0 cannot state it:** the lease expiry instant (4), the
  retention sweep that removes ended records (5), the supervisor clock that fires lease expiry and
  deadlines (8, 9), and the cursor's monotonicity (16), which the ordering scenarios check.
- **Deferred to the S8/S9 wave:** the profile of container and adopted records (3), adopted
  supervision (10), the container label keys (11) and a container timeout (12). The `StartContainer`
  and `AdoptProcess` declarations stay drafted and are marked deferred; nothing in this wave builds
  them.

## What this model does not cover

The kernel half of these claims belongs to a native suite. That covers killing the whole cgroup on
every stop, the process tree being gone before a record ends, the refusal of the `host` profile
under a hosted or static-bearer TCP posture, the per-subject partition of records, and the byte
bound on the log file. Here a record reaches `Ended` only through the supervisor's empty-cgroup
report (`RecordExit`). Lease expiry and the hard deadline are supervisor commands, because ESS has
no clock-driven transition.
