---
status: accepted
date: 2026-11-28
---

# ADR 0058: a start carries a per-run secret value held in memory

Decider: the operator, 2026-11-28 11:08 CET, relayed by the operator's session (D4, option A);
`approval-record:operator-answers-design-38-questions-2026-11-28`. Carries
[design 38](../docs/design/38-long-lived-service-on-the-host-driver.md) § 8 and decision W20. It
**amends [design 01](../docs/design/01-contract.md) § 7.7** for per-run values and for nothing else.
The design comes before code (invariant 8): this record adds a start member, a capability fact,
refusal codes and a successor bundle, and no code yet.

## Context

Design 01 § 7.7 reads: "No secret values in ordinary JSON, ever. Registries and secret slots are
named daemon configuration; audit records stay value-free by type." A secret slot is operator-declared
daemon configuration with a name and a bounded owner-private file. A start names slots and the
descriptors they must arrive at and never carries a value, a path or a length
([ADR 0012](0012-secret-slots-are-sealed-memfds.md)). The request carries `{slot, fd}`
(`SecretSlotRequest`, `crates/substrate-wire/src/lib.rs`); the child receives a sealed memfd at its
descriptor, and the mapping, names and numbers only, in `SUBSTRATE_SECRET_SLOTS`
(`crates/substrate-host/src/memfd.rs`, `secrets.rs`; `SECRET_SLOTS_ENV` in
`crates/substrate-wire/src/lib.rs`). At most `MAX_SECRET_SLOTS` (8) per run, and a value of at most
`MAX_SECRET_SLOT_BYTES` (65,536) bytes.

The `weft` repository's secret store gives a value per run, which a slot resolved from daemon
configuration cannot carry. What the tree does with a request, which decides where a value may land:

- The request hash is an unkeyed SHA-256 over the method, the address, the raw input JSON and the
  query (`canonical_request_hash_v2` in `crates/substrate-wire/src/lib.rs`). The `operations` table
  stores that hash, the state, the actor, the resource and the outcome JSON, and has no input column;
  an exec's record is `execs.resource_json` (`crates/substrate-store/src/schema.rs`). A replay
  compares the hash and a different one is `operation.request-conflict` (409) (`decode_mutation`,
  `reservation_response` in `crates/substrate-daemon/src/app/operations.rs`).
- ADR 0012 records that the ledger hashes the whole request body, which is why a slot never carries
  a value.
- Errors and events never carry secret material; a slot name is allowed in an error
  ([design 04](../docs/design/04-security-and-isolation.md) § 6).
- The lock file has `sha2`, `zeroize` and `getrandom` and no `hmac` (`Cargo.lock`).
- The daemon serves a Unix socket, where it derives the subject from kernel peer credentials, and a
  static-bearer TCP transport that `AGENTS.md` calls explicitly development-only.

## Decision

1. **The amended rule.** Design 01 § 7.7 now reads: *No secret values in ordinary JSON, except the
   one member `secret_values` of an exec start, a session start or a workload spec, accepted only
   from a daemon serving a Unix socket, held in memory only, and never written to the ledger, a
   record, an event, a log or an error. Registries and secret slots remain named daemon
   configuration; audit records stay value-free by type.* Design 01 is edited with the slice.
   Nothing else in § 7.7 or in ADR 0012 changes: a slot is still a name that resolves from
   configuration, and a start that carries no `secret_values` is held to the old rule as before.

2. **The member.** `secret_values: [{slot, fd, value}]` on the exec start, the session start and the
   workload spec, in the shape of `secret_slots` plus a `value` string of 1 to
   `MAX_SECRET_SLOT_BYTES` (65,536) bytes. Both lists share the run's bound of
   `MAX_SECRET_SLOTS` (8) and one namespace of slot names; a name in both is
   `exec.secret-value-name-taken` (422).

3. **Where a value lands and where it does not.** Each row is a requirement of the slice and a case.

   | place | outcome |
   |---|---|
   | `operations.request_hash` | **excluded.** Every `secret_values[*].value` is removed from the JSON before `canonical_request_hash_v2` is called; the slot and fd stay. An unkeyed hash of a low-entropy secret is an offline oracle. A start without the member hashes as on `0.29.0` |
   | `operations.outcome_json`, `execs.resource_json` | none: the applied record echoes `{slot, fd}`, as `secret_slots` does, never a value or a length |
   | the ledger's input | the `operations` table has no input column; nothing to redact |
   | events, the log ring, errors | none: an error may name a slot and never a value |
   | daemon memory | the value is in the request body buffer, the parsed JSON value and a dedicated buffer. Only the last is zeroed on drop (`zeroize`); the first two are ordinary heap. **A core dump or swap of the daemon holds the value** |
   | the child | a sealed memfd at its descriptor, as a slot's, named by `SUBSTRATE_SECRET_SLOTS` (names and numbers only) |
   | `workloads` table | none; the value is not stored |

4. **A changed value on a retry is detected without storing it.** At admission the daemon computes
   for each slot `HMAC-SHA256(k, op || slot || value)`, where `k` is 32 bytes from the OS random
   source taken at daemon start and held in memory only. It keeps the result in memory beside the
   run's accounting, by scope and operation, until the run is terminal or the workload deleted. A
   retry whose operation already has a ledger row compares its commitments in constant time before
   the replay is answered; a difference is `operation.request-conflict` (409), the code the ledger
   already gives for different input. The commitment is never persisted, logged or answered. A
   per-daemon key means a leaked commitment is no oracle after the process. HMAC is a short
   construction over `sha2` or one new crate.
   - **After a daemon restart the map is empty.** A retry then replays the recorded answer without
     comparison. That is accepted: the run it describes is gone, and nothing is delivered by a
     replay.

5. **A retry resends the value.** Nothing at rest holds it. A retry that must dispatch and carries
   none is `exec.secret-values-required` (422). A retry that finds the operation recorded replays
   and does not read the value.

6. **A workload's value lives in memory for its life**, in sealed memfds, so a policy restart gets
   the same value. After a **daemon** restart it is gone: the relaunch of
   [ADR 0056](0056-a-workload-is-a-confined-run-the-daemon-supervises-on-the-host-driver.md)
   cannot happen, the workload is `failed` with `cause: start-failed` and code
   `workload.secret-values-lost`, desired stays `running`, and the caller deploys again with the
   values. A lease expiry closes the memfds.

7. **Only over a Unix socket.** A request carrying `secret_values` over the static-bearer TCP
   development transport is `exec.secret-values-transport` (422) before dispatch. The fact is
   published only on a daemon serving a Unix socket that proved sealed delivery on a throwaway run;
   absent otherwise (invariant 3), and a start naming one is `exec.secret-values-unserved` (501).

8. **The wire of the slice**, in a successor bundle that rewrites no released byte (invariant 6):
   fact `exec.secret-values`, `const true`; field `secret_values`; applied `secret_values: [{slot,
   fd}]`. Refusals, each a wire constant with a vector: `exec.secret-values-unserved` (501),
   `exec.secret-values-transport` (422), `exec.secret-value-name-taken` (422),
   `exec.secret-values-required` (422), `workload.secret-values-lost` (409 on `start`, and the
   event's code); `operation.request-conflict` (409) already exists.

## Alternatives

- *A write-only route that stores the value on the node (D4 B).* Also puts a value in JSON against
  § 7.7, and puts a secret at rest on the node.
- *Daemon-configuration secrets only (D4 C).* Keeps § 7.7 and is today's state. It does not serve the
  consumer, whose store gives a value per run.

## Consequences

- **The rule of design 01 § 7.7 is weaker for one member.** A catalogue reading of "no value in
  JSON" no longer holds without the exception. The cost is accepted, and bounded by decisions 1, 3
  and 7: one member, a Unix socket, memory only.
- **A residual stays: the daemon's memory.** The request body buffer and the parsed JSON value are
  ordinary heap and only the dedicated buffer is zeroed, so a core dump or swap of the daemon holds
  the value. This record does not remove it.
- A workload that holds a value cannot be relaunched after a daemon restart. The caller deploys it
  again, and the service is `failed` until then.
- A retry after a daemon restart replays without comparing the value; a changed value is detected
  only while the daemon that admitted the operation lives.
- The unkeyed request hash stays as it is for every other member. Only the value is removed before
  hashing, so a start with the member and a start with a different value hash the same, and the
  commitment is what tells them apart.
- **What stays true of the isolation floor.** A value reaches the run only as a sealed memfd at a
  declared descriptor, as a slot's does; it is in no argument, no environment variable, no file
  the run can see by path and no log. The user namespace, the seccomp program, the bounds and the
  no-egress namespace are unchanged.
- The proof is a byte search over every byte the daemon wrote and answered during a run: no
  response, record, event, log stream, ledger file or daemon log holds the value.
- `AGENTS.md` invariant 6, the gate section and the lane counts of the runtime vectors move with the
  successor bundle, as with every successor.

## Not decided here

- Zeroing or locking the request buffers that hold a value, and whether the daemon should `mlock` or
  disable core dumps.
- Whether a workload survives a daemon restart with its values; surviving is
  [ADR 0056](0056-a-workload-is-a-confined-run-the-daemon-supervises-on-the-host-driver.md)'s open
  question, and values would need a store, which this record rules out.
- The hosted Identity admission profile (bundle `0.13.0`): this record states the Unix socket and the
  static-bearer TCP development transport only, and does not decide what a hosted transport may do
  with `secret_values`.
- Whether to add the `hmac` crate or write HMAC over `sha2` (design 38 § 16, I2).
- The number of the successor bundle: slices are ordered by the consumer's gaps, so it is taken when
  the bundle is cut.
