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
