# Host processes

The ESS domain `substrate.host_processes` specifies the resource family of
[ADR 0036](../../adr/0036-host-processes-are-a-supervised-resource-family.md): the *host process*
(supervised process), a long-lived process tree that a session starts through the daemon, with an
owner, a reason, a lease or a reasoned keep, an optional hard deadline, one cgroup, observations, a
bounded log and an event stream. Under the `host` profile the process runs unconfined, and the
record says so (`confinement: none`).

The wire types of S1-S7 and S11 are declared in `wire.yaml` and generated from it into
`crates/substrate-wire/src/generated/host_process.rs`, which `substrate-wire` re-exports (as
`ProcessStartInput`, `ProcessRecord`, `ProcessState`, `ProcessEndedBy` and the rest). No route or wire
bundle is derived from this domain yet; the successor bundle that carries the family is cut from it.
`cargo xtask check-ess` validates it, compares the committed synthesized suite
[`../host-processes-suite.json`](../host-processes-suite.json) byte for byte, runs the synthesized
and authored scenarios against the ESS interpreted reference, and regenerates the wire types and
compares them byte for byte.

```console
ess specify validate --path spec/host-processes --strict-requires
ess verify conform synthesize --path spec/host-processes --strict-requires \
  --out spec/host-processes-suite.json
ess verify conform run --path spec/host-processes --scenarios spec/host-processes \
  --target interpreted --report-format 2 --strict-requires
```

The pin is ESS 0.74.0 (`ess-inputs.yaml`). The authored scenarios under `scenarios/` are the
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
  deadlines (8, 9), and the cursor's monotonicity (16), which the ordering scenarios check. The
  same holds for three values of ADR 0036 itself: the effective deadline `deadline_at`, the earlier
  of `timeout_ms` and `expires_at` (S11), which the supervisor names when it enforces it; the
  environment digest `env_digest` (S1); and the wire bounds `MIN_LEASE_TTL_MS`,
  `MAX_LEASE_TTL_MS` and `MAX_PROCESS_TIMEOUT_MS`, which ESS 0.60.0 cannot declare as constants.
- **Deferred to the S8/S9 wave:** the profile of container and adopted records (3), adopted
  supervision (10), the container label keys (11) and a container timeout (12). The `StartContainer`
  and `AdoptProcess` declarations stay drafted and are marked deferred; nothing in this wave builds
  them.

## What this model does not cover

The kernel half of these claims belongs to a native suite. That covers killing the whole cgroup on
every stop, the process tree being gone before a record ends, the refusal of the `host` profile
under a hosted or static-bearer TCP posture, the per-subject partition of records, and the byte
bound on the log file. Here a record reaches `Ended` only through the supervisor's empty-cgroup
report (`RecordExit`) or its reconciliation finding the cgroup gone (`RecordVanished`, which ends
the record `vanished`). Lease expiry and the hard deadline are supervisor commands, because ESS has
no clock-driven transition.

## Wire types (`wire.yaml`)

`wire.yaml` declares what the routes of S1-S7 and S11 carry inside the existing mutation and success
envelopes: the start input, the record and its lease, deadline, exit and observations, the listing
page, the stop-by-owner input and result, and the log tail. `cargo xtask check-ess` generates them
with ESS's Rust target (`--names short`) and compares the file byte for byte. Three choices follow
from what ESS 0.60.0 can state:

- **State spelling.** ESS requires a lifecycle state name to start with an upper-case letter, and the
  wire spells every resource state in lower case, so `ProcessState` (`starting`, `running`,
  `stopping`, `ended`) is its own enum beside the `HostProcess` lifecycle.
- **Lease request.** ESS realizes a union with an explicit tag member, which ADR 0036's
  `{ "ttl_ms": n }` or `{ "keep": true }` does not have, so `ProcessLeaseRequest` is one struct with
  optional members, and the daemon admits exactly one of a TTL and a keep.
- **Reachable refusals.** Every start member with a named refusal is optional, and `profile` and
  `expires_at` are text, so an omitted or malformed member reaches its named refusal
  (`process.profile-required`, `process.profile-unknown`, `process.expires-at-invalid`, …)
  instead of the generic schema refusal.

Hand-written beside them in `crates/substrate-wire/src/lib.rs`, because ESS 0.60.0 cannot express
them: the bounds (`MAX_PROCESS_*` and the retention and grace constants; ESS declares no constants),
the refusal codes with their statuses (`PROCESS_REFUSALS`; an ESS error carries no wire code or
status), the `process.host` capability fact (a member of the existing hand-written capability
facts), and the two query-string types `ProcessListQuery` and `ProcessLogsQuery` (ESS realizes an
`Integer` as a JSON number, which a query-string decoder cannot read).
