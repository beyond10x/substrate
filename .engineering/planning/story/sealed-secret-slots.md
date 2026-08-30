---
format: aep.planning-md/1
id: story:sealed-secret-slots
kind: story
status: active
title: Sealed secret slots reach a child only as a memfd descriptor
summary: Design 06 decision 3 fixes the mechanism; a live vendor harness stays refused without it.
owner: substrate
tags:
- daemon
- host
- wire
relations:
- decomposes: epic:byte-plane-completion
revision: 6
---
# Story: Sealed secret slots reach a child only as a memfd descriptor

## Outcome

A run can use a daemon-configured secret without the value ever appearing in argv, environment,
logs, events, the ledger request hash or an error body — and design 05's reason for refusing a
live vendor harness is removed on the secret side.

## Context

`docs/design/06-authentication-secrets-and-trust.md` § *V1 decisions* 3 fixes the mechanism: a
sealed Linux `memfd` at a declared child descriptor; only the slot→descriptor mapping in the shaped
environment; acquisition after every admission and dispatch-time check; the daemon closes its copy
immediately after spawn; a driver that cannot prove sealing, isolation and cleanup reports the
capability absent. Design 04 § 7 (line 79): errors may name a slot, never the value.

## Acceptance

The delegated lane proves the child reads the value from the declared descriptor and the value
bytes appear nowhere in captured argv, environment, stdout, stderr, event, ledger or diagnostic
bytes; the portable lane proves the typed refusal.

Evidence that satisfies it, in order:

1. `adr/0012-secret-slots-are-sealed-memfds.md`: slot naming, descriptor declaration, the seal set
   (`F_SEAL_WRITE|SHRINK|GROW|SEAL`), refusal classes.
2. A successor bundle adds `secret_slots` on exec and session start, the capability fact
   `secrets.slots`, and the refusal class; earlier bundle bytes unchanged.
3. Failing-first tests: `secret_slot_value_absent_from_argv_env_events_and_ledger`,
   `secret_slot_memfd_is_sealed` (child `fcntl(F_GET_SEALS)` equals the declared set),
   `secret_slot_refused_when_sealing_unavailable`, `daemon_closes_its_copy_after_spawn`
   (`/proc/<daemon>/fd` holds no memfd after spawn).
4. The ledger request hash covers slot **names** only: two requests differing only in slot value
   hash identically.

## Out of Scope

Brokered artifact secrets (design 06 decision 4) and destination-bound egress.

## Design draft — 2026-08-30

`docs/design/11-sealed-secret-slots.md` (proposed), with `adr/0012` text ready to accept. Verified
at runtime, not inferred: bubblewrap 0.11.2 passes an inherited descriptor to the child at the
same number; a sealed memfd read back inside the sandbox reports `F_GET_SEALS == 0xf` and
`pwrite → EPERM`. `libc` already exports `memfd_create` and the seal constants — no Cargo change.
Today's `close_range` (`process.rs:348-377`) would close a slot fd; the draft generalises it to one
range per gap. Mapping variable `SUBSTRATE_SECRET_SLOTS=name=fd,…`; `MFD_CLOEXEC` mandatory.

## Progress — 2026-08-30, steps 2-4 done

Gate green (`bash scripts/gate.sh`, exit 0), nothing under `contracts/` disturbed:
`git status --short contracts/` shows only `?? contracts/substrate-wire/0.5.0/`, and
`cargo xtask render-bundle 0.4.0` still reproduces the frozen tree byte for byte (`diff -r` empty,
manifest `sha256:002337bd…`).

**Step 2 — the successor bundle.** `contracts/substrate-wire/0.5.0/`, 206 files, adding
`secret_slots` on exec and session start, the capability fact `secrets.slots` and the refusal class.
Verified as a fixed point of its own authored source: `cargo xtask check-bundle 0.5.0` →
`contract bundle 0.5.0 verified: 206 files, fixed point of xtask/bundle-source/0.5.0`.

**Step 3 — the four named tests, failing first.** Recorded before implementation:
seals `left: -1 right: 15`; child digest `e3b0c44…` (empty read) against `fffca559…`;
`/proc/self/fd still holds a slot memfd after spawn`; retained descriptors `[0,1,2,5]` against
`[0,1,2,5,7]`. All five `secrets::tests::*` pass now, plus six `secret_slot_tests::*` in the wire
crate.

**Step 4 — the request hash covers names only.**
`secrets::tests::ledger_request_hash_covers_slot_names_only` passes: two requests differing only in
a slot's value hash identically.

**A real bug the failing-first probe caught, not a test artefact.** `write_all` left the memfd
offset at EOF, so a child's `read(fd)` returned 0 bytes — and `/proc/self/fd/N` masked it, because
opening through `/proc` starts at offset 0. Fixed with `lseek(…, 0, SEEK_SET)` before sealing, with
a second child assertion guarding it. A daemon that shipped without this would have handed every
child an empty secret and reported success.

**ADR 0012 is contradicted by the code in one place, and the code is right.** Design § 5 wants
`secrets.slots` *absent* when orphan reconciliation fails. That state is unreachable: `probe()` runs
before `ProcessRuntime::new` (`crates/substrate-host/src/lib.rs:446`), and a failed reconciliation
aborts `HostDriver::open`, so the daemon never starts to report anything. Recorded in `probe.rs`.
Absent-is-weaker, so nothing about this is optimistic.

**Defaults taken.**

- ADR § 9 named `scripts/check-contract-bundle-0.5.0.py`. The org language rule won:
  `cargo xtask check-bundle <version>` instead, which re-renders from the authored source and
  compares bytes — strictly stronger than a hand-written checker, because a released tree that has
  stopped being the fixed point of its own source fails whatever else still looks well-formed.
  Noted in `docs/design/11-sealed-secret-slots.md` § 9.
- Invariant 4's `driver_port.rs` guard refused a re-export of `substrate_host::SecretSlot`, so the
  daemon declares its own type and converts at the composition root.
- `--secret-slot` validates the file at startup rather than at first use.
- **`CONTRACT_BUNDLE` stays `substrate-wire/0.4.0`** (`crates/substrate-daemon/src/app.rs:3`). A
  bundle exists to be pinned by a consumer before the server claims it; moving the
  `x-b10x-contract` header is its own change with its own clients to notify. Same posture
  `read_only_roots` already has — in the wire crate, in no bundle.

Two crate edges added, both already in the workspace: `zeroize` for `substrate-host`, `jsonschema`
for `xtask`. No new external dependency.

**Not done.** No end-to-end daemon run with a live slot — there is no delegated cgroup on this host,
so the four named tests are portable-lane, driving the real acquire/place/close code without a
sandboxed child. `read_only_roots` is still absent from every bundle including `0.5.0`.
