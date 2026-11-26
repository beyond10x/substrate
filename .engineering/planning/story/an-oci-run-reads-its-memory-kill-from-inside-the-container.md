---
format: aep.planning-md/3
id: story:an-oci-run-reads-its-memory-kill-from-inside-the-container
kind: story
status: draft
title: An oci run reads its memory kill from inside the container
summary: 'Draft, not ordered: an exact source for a memory-bound kill on oci is a helper inside the container; needs its own design document and the repository owner''s word'
relations:
- decomposes: epic:runs-are-governed-and-limit-hits-are-events
- informed_by: story:an-oci-run-killed-at-its-memory-bound-names-the-limit-on-a-hosted-runner
revision: 1
---
# Story: an oci run reads its memory kill from inside the container

Not ordered. Filed as a draft on the requester's answer to decision D101
(`approval-record:requester-answers-d101-2026-11-25`). Starting it is a question for the
repository owner: it adds a binary that runs inside every `oci` run, which is a change of this
repository's architecture.

## Outcome

On the `oci` adapter, a run the kernel killed at its memory bound ends with
`limit: Some(Memory)` in every run, where the adapter is configured with a helper that reads the
kill inside the container. Without the helper the adapter answers as design 36 D27 says:
`limit: None` and `limit_unread: UnattributedKill`.

## Where the requirement comes from

`story:an-oci-run-killed-at-its-memory-bound-names-the-limit-on-a-hosted-runner` measured the
sources of a memory-bound kill on a hosted runner (run 45659093178, 2026-11-25, 610 runs):

- the runtime's `OOMKilled` flag was missing in 128 runs, and its `oom` event in the same 128;
- a reader on the host of the container's `memory.events` read the kill in 66 of 100 runs;
- a command inside the container that outlives the kill of its child read `oom_kill` 1 in its
  own cgroup in 100 of 100 runs.

That story states the unread kill and attributes none. Stakeholder: the Runbook consumer. Its
session named no story and no consumer gap that needs exact attribution on `oci` on 2026-11-25.

## What it would change

1. A design document first (invariant 8). It answers at least: how the helper reaches the
   container (a mount of a file the adapter verified, as the exec-gate helper of design 35, or a
   layer of the image); that it is statically linked; how it starts the command (as its child,
   so that it outlives the command's kill, beside or in place of the runtime's `--init`); how it
   reports the count to the adapter without a channel the run can write (the run's own processes
   share the container); what a run can do to the helper and to its report; which exit status
   and signals the helper passes on; and what the adapter answers where the helper itself is
   killed, as when the memory bound is reached with the helper as the last process.
2. The helper as a workspace member with no dependency but `libc`, built for the musl target, as
   `crates/substrate-exec-gate` is.
3. `OciAdapter` configuration that names the helper; unset, nothing changes.
4. Lane cases on the `oci` lane, and the measurement of the first story repeated with the helper.

## Acceptance

Not written. It follows the design document.

## Not known

- Whether a report from inside the container can be trusted by the adapter at all: every
  process of the run can read and write what the helper can. The measurement used a counter the
  kernel writes (`memory.events` of the container's own cgroup, read inside); a run cannot
  raise it without a kill, and whether it can hide it from the helper was not examined.
- Whether the same question exists on `pod`. It was not measured.
