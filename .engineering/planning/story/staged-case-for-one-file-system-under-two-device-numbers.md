---
format: aep.planning-md/3
id: story:staged-case-for-one-file-system-under-two-device-numbers
kind: story
status: draft
title: A staged case holds one file system shown under two device numbers
summary: Design 35 D30's stated limit gets a lane case once a runner grants FUSE in a user namespace
tags:
- sdc
revision: 1
---
# Story: a staged case holds one file system shown under two device numbers

## Outcome

A lane case stages one directory shown twice under two device numbers, once as a read-only root
and once as a writable location, and holds that a start with an executable list follows nothing
on it.

## Where the requirement comes from

Design 35 D30 states the limit: where one file system shows under two device numbers (a network
file system mounted twice, two FUSE mounts of one directory), the comparison of mounts cannot see
that a read-only root and a writable location hold the same files. The guard is that nothing on a
file system outside `ext4`, `overlay` and `tmpfs` is followed. Session `sdc` asked on 2026-11-21
for a backlog item that stages it once a runner with FUSE exists. Stakeholder: session `sdc`.

## Why

- The limit is stated in the public documentation and in the changelog entry of the mount fix
  (pull request 244). No case stages it.
- The guard that covers it today is the closed file-system list of D26. A later story that admits
  another type would remove the guard without a red case.

## What changes

1. One case in the delegated lane: two FUSE mounts of one directory in a private user and mount
   namespace; one is a read-only root holding a planted link, the other lies in a writable
   location; a start listing the link is refused `exec.sandbox-place-unserved` before dispatch.
2. Where the host grants no FUSE mount in a user namespace the case prints `ABSENT` with that
   reason, and the lane requires the line (`require_staged` in `scripts/delegated-lane.sh`).

## Depends on

A runner that grants a FUSE mount inside a user namespace. A GitHub-hosted runner grants a user
namespace no mount at all (observed on pull request 244), so the case runs on a workstation until
one exists.

## Acceptance

`bash scripts/delegated-lane.sh` on a host with FUSE in a user namespace prints the case's
`executed` line and the refusal; on a host without it prints the case's `ABSENT` line with its
reason, and fails when neither line is there.
