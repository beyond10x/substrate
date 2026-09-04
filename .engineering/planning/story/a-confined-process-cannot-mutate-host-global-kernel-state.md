---
format: aep.planning-md/1
id: story:a-confined-process-cannot-mutate-host-global-kernel-state
kind: story
status: draft
title: A confined process cannot mutate host-global kernel state
scope:
- confidence: cited
  path: crates/substrate-host/src/process.rs
- confidence: cited
  path: crates/substrate-host/src/seccomp.rs
revision: 3
---
# Story: A confined process cannot mutate host-global kernel state

## Context

Opening a socket makes the kernel `request_module` on `net-pf-<family>` and
`net-pf-<family>-proto-<protocol>`. That is a property of `socket(2)` itself, not of any one
family, so a confined process can make the **host** load kernel code of its choosing into the
single global module table. The modules persist after the sandbox exits, and `/proc/modules` is not
namespaced, so a mutually-isolated sibling sandbox observes exactly what another loaded.

Measured during the 2026-09-04 wave B
(`review-result:adversary-waveb-u3-pass-2`, and the correction round that answered it):

```
AF_INET SOCK_STREAM IPPROTO_SCTP -> fd=3
=== host modules that appeared === sctp
```

The wave denied every family a confined process could not use anyway — `AF_ALG`, `AF_RDS`,
`AF_PPPOX`, `AF_KCM`, `AF_SMC`, `AF_MCTP`, `AF_PACKET` — which took the denied set from three to
ten. Three families are recorded **allowed with the residual accepted**, because refusing them
removes shipped capability:

| family | why it stays |
|---|---|
| `AF_INET` | `egress::install` is a TCP listener in the child's netns; refusing it removes the egress aperture |
| `AF_INET6` | the same, and name resolution |
| `AF_NETLINK` | **not measured** — the row reasons from `modules.alias` and says so. It is the only unmeasured claim in the table |

## Acceptance

A confined exec cannot cause the host kernel to load a module that was not already resident, by any
route, proven by a delegated-lane case that diffs `/proc/modules` across a sandbox that tries.

## Notes

`crates/substrate-host/src/seccomp.rs` carries `FamilyPolicy.host_state`, and a case fails on any
empty one — so a family added later cannot arrive without an answer to this question. What that
field cannot do is close the residual: the three allowed families are a deliberate trade, not an
oversight.

Closing this properly is a **design decision, not a correction**, and it needs an ADR under
invariant 8. Shapes worth weighing:

- pre-load every module the runtime needs at daemon start, then deny module autoload for confined
  children — moves the decision from the workload to the operator;
- run confined execs in a mount namespace whose `/proc/modules` is masked, which closes the
  *observation* half but not the *influence* half;
- accept the residual explicitly and document it in the safety envelope, which is what the code says
  today.

**Measure `AF_NETLINK` before deciding anything.** It is the one row in the table that was reasoned
rather than measured, and the wave that wrote it said so rather than quietly rounding it up.
