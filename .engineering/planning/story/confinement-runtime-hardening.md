---
format: aep.planning-md/1
id: story:confinement-runtime-hardening
kind: story
status: implemented
title: Confinement and runtime paths fail closed under pressure and restart
summary: Close host-root, pipe, file, aperture, replay and delegated-cgroup defects.
relations:
- decomposes: epic:byte-plane-completion
revision: 10
---
# Story: Confinement and runtime hardening

## Outcome

Host roots cannot reach host IPC or shadow owned mounts; pipe, aperture, file and replay paths terminate and report facts without deadlock, unbounded allocation or optimistic degradation.

## Acceptance

- AF_UNIX host IPC is blocked and probe-verified.
- Owned and overlapping mounts are refused component-wise.
- Pipe queue saturation is a durable named terminal failure.
- File reads remain bounded under concurrent growth.
- Aperture cleanup, counters, source eviction and delegated tests are race-free.
- Required delegated context is verified before replay.

## Current work — 2026-09-01

Superseded by the merged delivery record below. The former worktree is no longer unmerged.

## Current delivery — 2026-09-01

The exact-workspace-execution slice shipped through PR 57 and is merged on main at annotated tag 0.4.0 (commit 31340a6). It contains accepted ADR 0023, substrate-wire/0.12.0, probe-gated read-only/scoped workspace writes and SDK coverage for the current confinement options. The roadmap stories for contract promotion and SDK parity consume this result and must not reimplement it. This delivery replaces the earlier worktree note; closing the broader story still requires its complete acceptance evidence, not merely the release tag.

## Completion evidence — 2026-09-01

The 0.4.0 release at commit 31340a6 was re-audited against every acceptance item before implementation. Existing coverage already proved the AF_UNIX sentinel probe, bounded concurrent-growth reads, stale aperture cleanup, post-quiescence counters, idle-only source eviction, and the delegated cgroup lane.

The audit found two residual enforcement gaps: later-owned mounts (`/scratch`, the aperture hosts file, and the generated CA bundle) were not in the component-wise root collision set; and reusable AF_UNIX datagram socketpairs plus x32-tagged socket syscalls could evade the socket refusal. Failing-first regressions reproduced both gaps. Commit 218d655 closes them while preserving AF_UNIX stream socketpairs for process-local IPC and retaining the existing `exec.read-only-root-invalid` refusal.

Commit 7b94844 adds boundary-level evidence that pipe queue saturation persists `session.output-backpressure` as the durable terminal observation and that required delegated context is rejected when absent or malformed before a successful request can replay. The original attribution row remains immutable.

Verification on this branch:

- `cargo test -p b10x-substrate-host --lib --locked`: 67 passed.
- `cargo test -p b10x-substrate-daemon --test contract_vectors --locked`: 28 passed.
- `bash scripts/delegated-lane.sh`: exit 0 under a delegated cgroup v2 root; 67 host unit tests, 11 host integration cases, and 8 shipped-daemon runtime-vector tests passed, including the 20-attempt aperture ceiling race.
- Focused formatting and clippy checks for every changed crate passed with warnings denied.

No wire schema, refusal code, released contract bundle, or accepted design decision changed.
