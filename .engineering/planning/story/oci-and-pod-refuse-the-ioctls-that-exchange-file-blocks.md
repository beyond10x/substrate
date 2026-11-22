---
format: aep.planning-md/3
id: story:oci-and-pod-refuse-the-ioctls-that-exchange-file-blocks
kind: story
status: draft
title: The oci and pod adapters refuse the ioctl requests that exchange file blocks
summary: ADR 0050's refusal is in Substrate's own seccomp program; the container runtimes' default profiles pass the request (read, not run)
tags:
- security
revision: 1
---
# Story: the `oci` and `pod` adapters refuse the ioctl requests that exchange file blocks

## Outcome

A run on the `oci` or the `pod` adapter that calls `EXT4_IOC_MOVE_EXT` or `XFS_IOC_SWAPEXT` is
refused, as a run on the `linux` adapter is by ADR 0050, or the adapter states by a named fact
or refusal that it cannot hold that floor.

## Where the requirement comes from

ADR 0050 (the floor refuses the ioctl requests that exchange file blocks) states the gap and
does not decide it. The operator session ordered on 2026-11-22 that it is filed as a draft.
Stakeholder: the operator.

## Why

- On `linux` the refusal is in Substrate's own seccomp program. On `oci` and `pod` the container
  runtime applies its profile.
- Read, no container started: Docker's and containerd's default seccomp profiles allow `ioctl`
  with no argument rule. A run in a container with a writable volume on `ext4` can then exchange
  blocks as a run on `linux` could before ADR 0050.
- The executor of the namespace driver runs inside the pod; whether it applies Substrate's
  program to the run it starts was not read for this draft.

## Not known

- Whether the call succeeds in a container on this host. Not run.
- Which profile the `pod` adapter's pods run under on a cluster with a restricted Pod Security
  admission (`RuntimeDefault`), and whether a custom profile can be required without node access.
  The chart loads a Localhost profile for the host driver's own pod (ADR 0046); whether that
  profile filters ioctl was not read.

## What changes

1. Measure first: the lane case of ADR 0050's story, run on `oci` and in the pod lane, shows
   whether the call succeeds.
2. Decide, in a design note: Substrate ships a seccomp profile for the runs it starts on `oci`
   (a profile file passed to the runtime) and requires one on `pod` (a Localhost profile, or the
   executor applies the program itself before it starts the run), or the adapters publish that
   the floor's refusal is absent there and a profile that requires it is refused.
3. The case that was red on those adapters is green, or the named absence is held by a case.

## Acceptance

On `oci` and on `pod`, a run that calls either request gets `EACCES` with its files unchanged,
or the adapter's facts say the refusal is absent and a start that requires it is refused by
name; one of the two is held by a lane case on each adapter.
