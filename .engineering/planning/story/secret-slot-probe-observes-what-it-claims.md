---
format: aep.planning-md/1
id: story:secret-slot-probe-observes-what-it-claims
kind: story
status: draft
title: The secret-slot probe observes the seals and descriptor set it publishes a fact about
summary: Design 11 § 5 requires the probe child report the seals and nothing else above 2; the probe checks neither.
relations:
- decomposes: epic:byte-plane-completion
- depends_on: story:sealed-secret-slots
revision: 2
---
# Story: The secret-slot probe observes the seals and descriptor set it publishes a fact about

## Outcome

`secrets.slots` means what design 11 says it means. Today the capability fact is published on a
weaker observation than the one its design names, so a host whose bubblewrap passed a descriptor
with the wrong seals — or with extra descriptors leaked in — would still advertise the capability.

## Context

`docs/design/11-sealed-secret-slots.md:108-110` states the condition for publishing the fact:

> a bubblewrap child reporting the probe descriptor at its declared number with **the same seals and
> nothing else above 2**

The probe child does neither of those two things. Its whole command is
`crates/substrate-host/src/probe.rs:296-298`:

```
cat <&{target}; if echo x >&{target} 2>/dev/null; then printf writable; else printf sealed; fi
```

and the acceptance is `output.stdout == format!("{sentinel}sealed")`
(`crates/substrate-host/src/probe.rs:313`). So the child proves the descriptor arrived at its number
and is not writable. It never calls `fcntl(F_GET_SEALS)`, so "the same seals" is unobserved — a
descriptor sealed `F_SEAL_WRITE` alone would pass. It never enumerates `/proc/self/fd`, so "nothing
else above 2" is unobserved.

The in-process half of the condition *is* observed — `memfd_create` + `F_ADD_SEALS` with
`F_GET_SEALS` reading back `0xf` and a refused write — and the parent enforces the rest by
construction (`crates/substrate-host/src/secrets.rs`, `place_and_close`). Enforced by construction
is not the same as observed, and invariant 3 is that a guarantee substrate cannot verify is a named
refusal, not a fact published anyway.

This is not a live exposure: `crates/substrate-daemon/tests/runtime_vectors.rs` now observes both
properties at run time on the delegated lane — the child reports `fds:[0,1,2,7]`, `memfds:[7]`,
`seals:15`. The gap is that the *gating probe* does less than the test does, so the fact can be
published on a host where the test never ran.

Found while proving `story:sealed-secret-slots` on the delegated lane, 2026-08-30.

## Acceptance

The probe child reports its seal word and its descriptor set, and `secrets.slots` is withheld when
either disagrees with what design 11 § 5 requires.

Evidence that satisfies it:

1. The probe child reads `F_GET_SEALS` on the target descriptor and enumerates `/proc/self/fd`, and
   the parent compares both against the declared values — not against a substring of `cat` output.
2. A failing-first test per property: a probe whose child reports a short seal word withholds the
   fact; a probe whose child reports an extra descriptor withholds the fact. Both must fail before
   the change and pass after.
3. `secrets.slots` is still published on this host after the change (the probe did not become
   unsatisfiable), proven by `bash scripts/delegated-lane.sh` still reporting its full case count.
4. Whichever way it lands, `docs/design/11-sealed-secret-slots.md:108-110` and the code agree
   afterwards. If the decision is instead that the design overstates what a probe can cheaply
   observe, that is an amendment to an accepted ADR (0012) and needs the operator, not a quiet
   edit.

## Out of Scope

Changing the seal set, the slot naming rule, or anything about how a slot reaches a child. This
story is only about what the probe observes before it publishes the fact.
