# Design 11: sealed secret slots

**Status:** accepted as [ADR 0012](../../adr/0012-secret-slots-are-sealed-memfds.md) · **Date:** 2026-08-29

## 1. Problem

A confined process cannot hold a credential, so substrate cannot run the work that needs one.
[Plan 04](../plan/04-direct-byte-plane.md) states it: "sealed named secret slots, and
destination-bound egress remain separate additions. A live vendor harness is refused until the
required secret and egress capabilities exist" (`docs/plan/04-direct-byte-plane.md:87`); design 05
phase-4 decision 6 keeps slots absent "until their own slice proves them". This document proves the
secret side. Egress is a separate slice and is not weakened here: a process holding a slot still has
an unshared network namespace and no route out.

The value must not be reachable through any surface substrate produces — argv, the shaped
environment, logs, typed events, the ledger request hash, resource observations, an error body
([design 06 § 3](06-authentication-secrets-and-trust.md),
`docs/design/04-security-and-isolation.md:79`: an error may name a *slot*, never its material).
"Not printed" is not the guarantee; "never in a byte substrate can emit" is.

## 2. What exists

| Mechanism | Where | What it already gives |
|---|---|---|
| environment clearing and shape | `crates/substrate-host/src/process.rs:891`, `:908`, `:1429`, `:1453`; `crates/substrate-daemon/src/app/operations.rs:383` | `env_clear()` plus bubblewrap `--clearenv`, so the child environment is built only from `--setenv`; a caller-set name containing `secret`, `token`, `credential`, … is already refused at both layers |
| descriptor closure | `process.rs:348-377` | `pre_exec` runs two `close_range` windows — `[3, sync_fd-1]` and `[sync_fd+1, u32::MAX]` — then `PR_SET_NO_NEW_PRIVS`. Only stdio and the launch-barrier descriptor survive |
| launch barrier | `process.rs:339-347`, `:947`, `:1673-1688` | a private pipe passed as bubblewrap `--block-fd`, released after cgroup attach; the parent already drops both ends right after spawn (`:379`, `:403`) |
| output draining | `process.rs:1329-1387`, `:1054-1093` | two tasks drain stdout/stderr to an admitted cap and keep draining past it, so a child never blocks and no consumer sees more than the attested bytes |
| admission and dispatch-time recheck | `process.rs:780-879`; `operations.rs:234-318` | daemon-side and driver-side predicates before dispatch; `recheck_backend` refuses `exec.capability-stale` before anything is materialized |
| capability facts | `crates/substrate-host/src/probe.rs:42-116` | a fact is published only from a successful probe; the snapshot digest (`probe.rs:98-115`) binds driver, generation, probe time, facts and backend binding |
| request hash | `crates/substrate-wire/src/lib.rs:1666-1691`; used at `operations.rs:96` | `canonical_request_hash_v2` frames method, address, **the whole canonical request input** and query into one SHA-256 |
| startup reconciliation | `process.rs:122-159`, `:162-245` | orphan `substrate-ex_` cgroups are killed and proven empty before capsule cleanup runs; an unexpected entry fails closed |
| operator declaration | `crates/substrate-daemon/src/main.rs:10-66`; `crates/substrate-daemon/src/runtime.rs:615-641` | clap derive; the bearer file precedent already demands one bounded regular file with private ownership and reads it into a `Zeroizing` buffer |

Two consequences shape everything below. The request hash covers the entire request body, so the
only way a value stays out of the ledger is to never be in the request. And `close_range` closes
every descriptor except stdio and the barrier, so a slot descriptor must be introduced into that
closure deliberately — it cannot be inherited by accident.

## 3. Slots

**Operator declaration.** A slot is daemon configuration, never request data:
`--secret-slot <name>=<path>` (repeatable, `SUBSTRATE_SECRET_SLOT`, `value_delimiter = ','`). The
path must be one regular file with private workload ownership — the predicate `read_bearer_digest`
already applies at `runtime.rs:617-632` — bounded at 64 KiB. A name matches `[a-z][a-z0-9_]{0,63}`;
a repeated name fails startup; a daemon with no declared slot does not publish the capability.

**Request.** A start names slots and the descriptor each must arrive at —
`"secret_slots": [{"slot": "vendor_api_key", "fd": 7}]`. `fd` is in `3..=63` and distinct within the
request; `slot` must be declared. The field is absent on every existing consumer and carries no
value, path or length. It joins `ExecStartInput` (`crates/substrate-wire/src/lib.rs:752-771`) beside
`read_only_roots`, and pipe-session start through the same embedded input.

**Delivery.** At acquisition the daemon reads the declared file into a `Zeroizing` buffer, creates
`memfd_create("substrate-slot-<slot>", MFD_CLOEXEC | MFD_ALLOW_SEALING)`, writes the bytes, drops
the buffer, and applies exactly
`F_ADD_SEALS: F_SEAL_WRITE | F_SEAL_SHRINK | F_SEAL_GROW | F_SEAL_SEAL` — `0xf`, no more and no
less. `F_SEAL_SEAL` closes the set, so no later holder — the daemon included — can add or remove a
seal. The memfd name is the slot name, non-secret by construction, and is what the child sees at
`/proc/self/fd/7 -> /memfd:substrate-slot-vendor_api_key (deleted)`. `MFD_CLOEXEC` is not
decoration: `max_concurrent_execs` defaults to 16 (`crates/substrate-host/src/lib.rs:73`), so spawns
overlap and an inheritable memfd would be inherited by *another subject's* child between its fork
and its `close_range`. The descriptor becomes inheritable only inside this child's `pre_exec`, by
`dup2`, which clears `FD_CLOEXEC` on that target alone.

**Crossing bubblewrap.** Bubblewrap passes an inherited descriptor to the child at the same number;
verified with bubblewrap 0.11.2, where a sealed memfd placed at descriptor 7 was readable inside the
sandbox, reported `F_GET_SEALS == 0xf`, and refused `pwrite` with `EPERM`. No bubblewrap flag is
involved. Being backend behaviour and not a documented contract, it is probed and never assumed.

**Discovery.** The shaped environment carries the mapping and nothing else, as one `--setenv` beside
the existing ones at `process.rs:949-954`:
`SUBSTRATE_SECRET_SLOTS=registry_token=9,vendor_api_key=7`. Pairs are `name=fd`, comma-separated,
sorted by name, ASCII, no spaces, no trailing comma; absent when the request declares no slot. A
caller cannot collide with it: `secretish_name` already refuses any caller-set name containing
`secret` (`operations.rs:383-395`, `process.rs:1453-1466`).

## 4. Ordering

Acquisition is the last thing before spawn and the first thing released after it.

| # | Step | Crash here leaves |
|---|---|---|
| 1 | daemon admission — scope, shape, capability predicate, `secrets.slots` present, slot declared, descriptors legal (`operations.rs:234-318`) | nothing acquired |
| 2 | durable `accepted` reservation before dispatch (ADR 0005; `crates/substrate-daemon/src/app/execs.rs:151`) | an `accepted` row with no value read |
| 3 | driver admission and `recheck_backend` (`process.rs:780-879`) | nothing acquired |
| 4 | **acquire**: read file, memfd, write, seal, verify `F_GET_SEALS == 0xf` | anonymous memory held by the daemon process only; freed by the kernel when it exits |
| 5 | `pre_exec`: place each slot with `dup2`, then `close_range` over the complement of the retained set | the child never execs; both copies die with the fork |
| 6 | `spawn` | as step 5 |
| 7 | daemon closes its copies, beside the existing `drop(sync_read)` at `process.rs:379` | the child holds the only copy |
| 8 | child exits; cgroup killed and proven empty (`process.rs:1638`) | no holder; the kernel frees the memfd |

Step 5 generalises the current two `close_range` windows. The retained set is
`{0,1,2} ∪ {declared fds} ∪ {sync_fd}`; after every `dup2` the driver issues one `close_range` per
gap between consecutive retained descriptors, ascending. With no slots those gaps are exactly the
two windows at `process.rs:355-372`, so today's behaviour is the special case. The daemon's source
descriptors are staged above 63 with `F_DUPFD_CLOEXEC` before the fork, so a declared descriptor can
never collide with one.

**Startup cleanup is a proof, not an action.** A memfd has no name in any filesystem, so unlike a
capsule directory (`process.rs:162-245`) there is no residue to sweep. The only possible holder is a
surviving child, and `reconcile_orphans` (`process.rs:122-159`) already kills every `substrate-ex_`
cgroup and proves it empty *before* any exec is admitted after a restart. A driver that cannot
complete that reconciliation reports `secrets.slots` absent.

## 5. Refusals

The probe publishes `secrets.slots` only when all of it holds: at least one declared slot; a
`memfd_create` + `F_ADD_SEALS` round trip whose `F_GET_SEALS` reads back exactly `0xf` and whose
write attempt fails; a bubblewrap child reporting the probe descriptor at its declared number with
the same seals and nothing else above 2; and successful orphan reconciliation. The fact is the
sorted list of declared **names** — never a path, a length or a digest of a value. Adding or
removing a slot changes the fact and so the snapshot digest (`probe.rs:98-115`); **rotating a value
changes nothing observable**, which keeps § 7 true.

| Situation | Class | Code | Field |
|---|---|---|---|
| slot name not declared by the operator | `refused` | `exec.secret-slot-unknown` | `secret_slots` |
| descriptor outside `3..=63`, or repeated in the request | `refused` | `exec.secret-slot-descriptor-invalid` | `secret_slots` |
| descriptor collides with stdio or the barrier/pipe descriptors | `refused` | `exec.secret-slot-descriptor-invalid` | `secret_slots` |
| capability `secrets.slots` absent and a slot is requested | `unserved` | `exec.secret-slots-unserved` | `secret_slots` |
| sealing, descriptor pass-through or cleanup unprovable at probe | capability **absent** | — | — |
| declared file unreadable, oversized or not owner-private at acquisition | `failed` | `exec.secret-slot-unreadable` | `secret_slots` |
| seal verification fails after `F_ADD_SEALS` | `failed` | `exec.secret-slot-unsealed` | `secret_slots` |

Every message names the slot and nothing else. There is no fallback: a slot never degrades to an
environment variable, a workspace file, or an argument.

## 6. Non-leakage proofs

Each test observes the process from outside, never from a claim the code makes about itself, and
seeds a distinct high-entropy sentinel as the slot value to search for.

| Test | Observed from outside |
|---|---|
| `secret_slot_value_absent_from_argv_env_events_and_ledger` | the sentinel is absent from `/proc/<child>/cmdline` and `/proc/<child>/environ`, from captured stdout/stderr, from every emitted event body, from the ledger row including `request_hash`, and from every error body in the run |
| `secret_slot_memfd_is_sealed` | the child's own `fcntl(fd, F_GET_SEALS)` equals `F_SEAL_WRITE`, `F_SEAL_SHRINK`, `F_SEAL_GROW` and `F_SEAL_SEAL` together, and a write to the descriptor fails `EPERM` |
| `secret_slot_refused_when_sealing_unavailable` | with sealing forced unavailable, `/v1/machine` omits `secrets.slots` and a start naming a slot answers `unserved`, never a weaker delivery |
| `daemon_closes_its_copy_after_spawn` | `/proc/<daemon>/fd` contains no `memfd:substrate-slot-*` link once the start has answered, while the child still reads the value |

Two more the mechanism needs: the child's descriptor set is exactly `{0,1,2} ∪ {declared}` plus
bubblewrap's own; and a concurrent start for a second subject sees none of the first's slots.

## 7. Ledger and events

The value is never in the request, so it is never in the hash. `canonical_request_hash_v2`
(`crates/substrate-wire/src/lib.rs:1666-1691`) frames the whole canonical input, which holds
`{"slot": "vendor_api_key", "fd": 7}` — a name and a number. Two starts differing only in the
material behind the slot are byte-identical requests and hash identically, so an operator can rotate
a credential without invalidating an admitted operation and without the rotation appearing anywhere
a client can read.

Typed events and the applied confinement record carry slot names and descriptors, so an auditor sees
that a run held `vendor_api_key` and where it arrived. `AppliedConfinement` (`process.rs:401-419`)
records what was applied rather than what was asked, and slots follow that rule.

## 8. Conformance vectors

New driver vectors shaped like `vectors/driver/credential-inheritance.json` in the 0.4.0 bundle — a
`daemon-authority` sentinel setup plus `/probes/*` postconditions asserted `false`. Each requirement
id gains a `coverage.json` row sourced here, in the existing `requirements[].evidence[]` form.

| Vector id | Covers |
|---|---|
| `secret-slot-reaches-declared-descriptor` | `secrets.slot-delivery` |
| `secret-slot-memfd-is-sealed` | `secrets.slot-sealed` |
| `secret-slot-value-never-observable` | `secrets.slot-non-leakage` |
| `secret-slot-unknown-name-refused` | `secrets.slot-unknown` |
| `secret-slot-unserved-without-capability` | `secrets.slot-unserved` |
| `secret-slot-daemon-copy-closed` | `secrets.slot-cleanup` |

## 9. Compatibility

`0.4.0` is frozen with the other released bundles (invariant 6). The change ships as successor
`contracts/substrate-wire/0.5.0`, `kind: additive-v1`, `predecessor: "0.4.0"`, `adds_routes: 0`,
`preserves_routes: 26` — the count `0.4.0` already carries (19 preserved + 7 added). No byte of an
earlier bundle changes, and the successor's checker joins the four in `scripts/gate.sh`; a bundle
whose checker is not in the gate is unverified from the next commit.

**Implementation note, 2026-08-30.** That checker is `cargo xtask check-bundle 0.5.0`, not
`scripts/check-contract-bundle-0.5.0.py` as this section first said. Between this design and its
implementation, `story:tooling-moves-to-cargo-xtask` landed the Rust renderer and moved the gate's
own checks to `cargo xtask` verbs; the four Python pairs stay only as the reproducibility proof of
the bundles they froze (`AGENTS.md` § *The gate*). Writing a fifth Python script would have added a
runnable Python file to a repository whose language rule forbids one. The Rust verb checks strictly
more than its predecessors could: it re-renders the bundle and compares byte for byte, so a
hand-edit anywhere in `contracts/substrate-wire/0.5.0` fails the gate.

Request policy is closed, so failure is honest both ways: a `0.5.0` client sending `secret_slots` to
a `0.4.0` daemon is refused as schema-invalid, and a `0.4.0` client is unaffected.

**No dependency is missing.** `libc` is already a direct dependency of `substrate-host`
(`crates/substrate-host/Cargo.toml:15`) and on `target_os = "linux"` exports all of it:
`memfd_create`, `MFD_CLOEXEC`/`MFD_ALLOW_SEALING`, `F_ADD_SEALS`/`F_GET_SEALS`, the four `F_SEAL_*`
constants. The workspace `nix` features `["fs", "user"]` (`Cargo.toml:35`) already gate in
`nix::sys::memfd` and `SealFlag`, so that route needs no feature change either — but `substrate-host`
does not depend on `nix`, and `libc` matches the `pre_exec` code beside it.

## 10. Open decisions

| # | Decision | Owner | DEFAULT if unanswered |
|---|---|---|---|
| 1 | `MFD_NOEXEC_SEAL` on the memfd | host driver | **Do not set it.** It adds `F_SEAL_EXEC` to `F_GET_SEALS`, breaking the exact-equality test the story names, and needs Linux 6.3; the child can already write and exec a file in its own workspace, so it buys no authority |
| 2 | Slot value read at acquisition versus cached at startup | daemon | **Read at acquisition** into a `Zeroizing` buffer. Rotation needs no restart and the daemon holds no long-lived plaintext |
| 3 | Descriptor ceiling | wire | **63.** Above the stdio and barrier range, small enough to keep the `close_range` gap list bounded |
| 4 | Slots on pipe-session start in the same slice | wire | **Yes.** Sessions embed `ExecStartInput`; excluding them would make the capability depend on which start route was used |
| 5 | ADR number | operator | **0012**, per the story. `adr/0011-*` is unassigned at this commit; if another slice takes 0012 first, renumber before accepting |

## 11. Proposed ADR text

Ready to accept as `adr/0012-secret-slots-are-sealed-memfds.md`, plus the `adr/README.md` index row
for 0012 — title *Secret slots are sealed memfds*, status `accepted` — in the existing row form.

```markdown
---
status: accepted
date: 2026-08-29
---

# ADR 0012: secret slots are sealed memfds

## Context

Substrate can confine a process but cannot give it a credential, so the work that needs one — a
vendor harness under confinement — is refused (`docs/plan/04-direct-byte-plane.md:87`). Design 06
§ *V1 decisions* 3 fixed the mechanism and left it unimplemented. Every ordinary channel leaks: argv
shows in `/proc/<pid>/cmdline`, the environment in `/proc/<pid>/environ`, a workspace file survives
the run, and the ledger hashes the whole request body (`crates/substrate-wire/src/lib.rs:1666`).

## Decision

A secret slot is operator-declared daemon configuration with a name and a bounded owner-private
file. A start names slots and the descriptors they must arrive at; it never carries a value, a path
or a length.

At dispatch, after every admission check and the backend recheck, the driver copies the declared
bytes into an anonymous `memfd`, applies exactly
`F_SEAL_WRITE|F_SEAL_SHRINK|F_SEAL_GROW|F_SEAL_SEAL`, verifies the read-back, places it at the
declared descriptor with `dup2` in `pre_exec`, closes every other descriptor above stdio, spawns, and
closes its own copy immediately. The child finds the mapping — names and descriptors only — in
`SUBSTRATE_SECRET_SLOTS`.

The capability fact `secrets.slots` is the sorted list of declared names, published only when a
probe proves sealing, descriptor isolation across bubblewrap, and orphan reconciliation. An unknown
slot or illegal descriptor is `refused`; a slot requested while the capability is absent is
`unserved`. There is no weaker delivery.

## Consequences

An operator rotates a credential without restarting the daemon, without invalidating an admitted
operation, and without any client observing that it happened: the value is outside the capability
snapshot and outside the request, so two otherwise identical starts hash the same.

Errors and events may name a slot and never its material. A driver that cannot prove the guarantees
reports the capability absent rather than degrading, per invariant 3. `F_SEAL_SEAL` closes the seal
set, so not even substrate can reopen the memory it handed over. Descriptor pass-through through
bubblewrap is probed on every capability snapshot rather than assumed.
```
