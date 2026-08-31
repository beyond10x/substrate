---
format: aep.planning-md/1
id: story:confinement-runtime-hardening
kind: story
status: active
title: Confinement and runtime paths fail closed under pressure and restart
summary: Close host-root, pipe, file, aperture, replay and delegated-cgroup defects.
relations:
- decomposes: epic:byte-plane-completion
revision: 3
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
