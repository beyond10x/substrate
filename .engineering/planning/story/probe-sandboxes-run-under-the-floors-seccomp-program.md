---
format: aep.planning-md/3
id: story:probe-sandboxes-run-under-the-floors-seccomp-program
kind: story
status: draft
title: The driver's probe sandboxes run under the floor's seccomp program
summary: Eight probe sandboxes start bubblewrap without the program; no caller reaches them
tags:
- security
revision: 1
---
# Story: the driver's probe sandboxes run under the floor's seccomp program

## Outcome

Every sandbox the host driver starts runs under the one seccomp program of the floor, the probes
it starts for itself included.

## Where the requirement comes from

Found while ADR 0050 was implemented and confirmed by its adversary pass and its security review:
the probes in `pty.rs`, `read_only_files.rs`, `writable_roots.rs`, `workdir.rs`, `egress.rs` and
three in `probe.rs` start bubblewrap without `--seccomp`. Both reviews read that each runs a fixed
argv written by the driver over directories the driver made, and that no caller reaches them. So
this is not a defect a caller can use; it is a second kind of sandbox the floor's text does not
describe. Stakeholder: the operator.

## What changes

1. The probes use the command builder's program, or `AGENTS.md`'s safety envelope and ADR 0050 say
   in one sentence that probes are outside the floor and why that is sound.
2. A case that fails when a new probe is added without the program, or without being named in
   that sentence.

## Acceptance

`grep` for the bubblewrap argv builders finds none without the program apart from the ones the
sentence names, and the case of step 2 holds it.
