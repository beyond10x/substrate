---
format: aep.planning-md/1
id: story:sealed-secret-slots
kind: story
status: proposed
title: Sealed secret slots reach a child only as a memfd descriptor
summary: Design 06 decision 3 fixes the mechanism; a live vendor harness stays refused without it.
owner: substrate
tags:
- daemon
- host
- wire
relations:
- decomposes: epic:byte-plane-completion
revision: 4
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
