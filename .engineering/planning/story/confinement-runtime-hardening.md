---
format: aep.planning-md/1
id: story:confinement-runtime-hardening
kind: story
status: active
title: Confinement and runtime paths fail closed under pressure and restart
summary: Close host-root, pipe, file, aperture, replay and delegated-cgroup defects.
relations:
- decomposes: epic:byte-plane-completion
revision: 8
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
