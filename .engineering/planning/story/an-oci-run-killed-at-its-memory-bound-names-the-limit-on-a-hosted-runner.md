---
format: aep.planning-md/3
id: story:an-oci-run-killed-at-its-memory-bound-names-the-limit-on-a-hosted-runner
kind: story
status: active
title: An oci run killed at its memory bound names the memory limit on a hosted runner
summary: 'Measured on a hosted runner: the runtime''s OOM flag is missing in 128 of 610 memory-bound kills and never late; design 36 D27 states the kill the adapter cannot attribute'
tags:
- defect
- flaky
relations:
- decomposes: epic:runs-are-governed-and-limit-hits-are-events
- informed_by: story:run-observation-names-the-limit-that-ended-it
revision: 7
transitions:
- {from: "draft", to: "proposed", at: "2026-11-25T16:29:13Z", actor: "agent:substrate", revision: 4}
- {from: "proposed", to: "active", at: "2026-11-25T16:29:14Z", actor: "agent:substrate", revision: 5}
---
# Story: an oci run killed at its memory bound names the memory limit on a hosted runner

## Outcome

On the `oci` adapter, a run the kernel killed at its memory bound ends with
`limit: Some(Memory)`, or the observation says that the limit could not be read and why. The
gate's case `an_oci_run_over_its_memory_bound_names_the_memory_limit` passes on a hosted runner
in every run, without a retry.

## Where the requirement comes from

The gate job `Gate: tests and Clippy` of pull request 302 failed once on 2026-11-25 in
`an_oci_run_over_its_memory_bound_names_the_memory_limit`
(`crates/substrate-host/tests/ess_confinement_oci.rs:1297`), run 45594091184, attempt 1, job
139144273381. The pull request changes no code of the host crate. The same tree passed that job
on pull request 301. Stakeholder: the Runbook consumer, whose requirement is that every limit hit
is visible as an event.

## Observed

```
ESS confinement oci limit memory: the run ended after 1.61908603s
assertion `left != right` failed: ESS confinement oci limit memory: the runtime's state held no
OOMKilled within a second of the exit: RunObservation { …, adapter: Oci, …, state: Ended,
exit: Some(RunExit { code: None, signal: Some(9) }), limit: None, memory_oom_kills: None,
process_limit_hits: None }
```

This is the second sighting. The first, on a hosted runner on 2026-11-24, is recorded in
[design 36](../../../docs/design/36-limit-hits.md); the monitor then made one read. From that
day the monitor read the runtime's state again for one second when the first read held no
`OOMKilled`. The run above ended with signal 9 and the state held no flag within that second.
3,774 of the run's 3,775 cases passed.

The third sighting is of the same day: the gate of the push of `main` at `224e1e5ffd`, run
45644944907, attempt 1, job 139325324759. The case printed `the run ended after 1.466541576s`
and failed in the same assertion with the same observation.

The case's profile bounds the run at 2,000 ms of wall time, and both failed runs ended before
it (1.62 s and 1.47 s), so neither kill was the monitor's own.

## Why it matters

- A consumer reads `limit` to tell a memory-bound kill from another kill. Here the kill is
  reported as signal 9 with no limit, so the limit hit is not visible.
- D7 forbids deriving a limit from an exit or a signal alone, so the adapter cannot fill the gap
  from the signal.
- The case fails the gate of a pull request that does not touch it. A re-run of the job is the
  only way past it today.

## Measured

Step 1, done on 2026-11-25: one job on a standard hosted runner, run 45659093178, started by a
push of the branch `measure/oci-oom-flag` at `afbad6851a`. The branch is not merged and has no
pull request; it holds `scripts/measure-oci-oom-flag.sh` and the workflow that runs it on a push
of that branch alone.

The runner: image ubuntu-26.04, kernel 7.0.0-1020-azure, 4 CPUs, Docker 29.4.2 with the systemd
cgroup driver on cgroup v2, runc 1.5.3.

The script repeats the shape of the gate's case with the runtime's own client: the arguments
the `oci` adapter passes to `create`, then `start --attach`, then reads of the container's state
for 5 s. 610 runs in five phases:

| phase | runs | flag at the first read | flag at a later read | no flag in 5 s |
|---|---|---|---|---|
| A: one at a time, no load | 60 | 56 | 0 | 4 |
| B: one at a time, beside 8 busy loops | 150 | 113 | 0 | 37 |
| C: four at a time, beside 8 busy loops | 200 | 146 | 0 | 54 |
| D: as B, with a reader on the host of the container's `memory.events` | 100 | 81 | 0 | 19 |
| E: as B, the growing process a child of the container's command | 100 | 86 | 0 | 14 |
| all | 610 | 482 | 0 | 128 |

What else the job read:

- The kernel log holds 610 OOM kill lines. It names 569 of the 610 cgroups; the kernel limits
  the rate of that report.
- The runtime's `oom` event was sent for 482 containers, the same 482 that held the flag. No
  run without the flag had an event.
- Phase D's reader on the host read `oom_kill` above zero in 66 of 100 runs: flag and reader 57,
  flag only 24, reader only 9, neither 10.
- Phase E's command read `oom_kill` 1 in its own cgroup in 100 of 100 runs, after the kill of
  its child.
- Run times, least, median and most, in ms: A 174, 187, 226; B 339, 406, 592; C 335, 471, 753;
  D 369, 433, 558; E 365, 433, 554.

What it answers of *Not known* as the story was filed:

- The flag was never late. It was missing: in 4 of 60 runs with no load and in 14 % to 27 % of
  the runs of the four phases under load. A longer read names nothing more.
- Of the sources the adapter can read from outside the container, none is exact: the runtime's
  event stream equals the flag, and a reader on the host of the cgroup's counter missed 34 of
  100 kills.
- The one exact source in the job runs inside the container.

Not covered: another runner image, another runtime, a rootless daemon, and more than one job.
On a workstation (Docker 29.8.2, the cgroupfs driver on cgroup v2) every probed run held the
flag at the first read: 330 of 330 on 2026-11-24 and 19 of 19 on 2026-11-25.

## Decided

`approval-record:requester-answers-d101-2026-11-25`: the stated reason, and no exact source in
this story. Design 36 holds the rule as D27 and D28, amended on 2026-11-25 before code.

## What changes

1. Done: the measurement above (`approval-record:requester-answers-d100-2026-11-25`).
2. Done: design 36 is amended. D27: `RunObservation` gains `limit_unread: Option<LimitUnread>`,
   and the `run_ended` record the same member. `oci` sets `UnattributedKill` for a run whose main
   process ended by `SIGKILL` that neither the monitor nor the embedder sent and whose state
   holds no `OOMKilled`. `limit` stays `None`. D28: the monitor reads the runtime's state once;
   the re-read of one second is removed.
3. The code of D27 and D28 in `crates/substrate-host/src/confinement/`, with the cases design 36
   § 8 lists, the regenerated public-items list, the `UNMAPPED` block of the specification with
   its regenerated projections, and a `CHANGELOG.md` entry.
4. The gate's case stays in the gate and gets no retry. Its assertion becomes: `limit` is
   `Memory` and `limit_unread` is `None`, or `limit` is `None` and `limit_unread` is
   `UnattributedKill`. It prints which of the two it read. Any other observation fails it.

## Acceptance

A case that forces the state the hosted runner showed (signal 9, no flag at the read) is red
before the change and green after it, and the measurement of step 1, repeated after the change,
records no run of the case that ends without a named limit or a stated reason. The repeat is
one job on a standard hosted runner, from a branch that is not merged, which runs the memory
case's shape through the changed adapter and counts the runs with a named limit, with the stated
member and with neither.

## Boundaries

- No exact attribution of a memory-bound kill on `oci`:
  `story:an-oci-run-reads-its-memory-kill-from-inside-the-container`, a draft that is not
  ordered.
- `pod` is not changed and was not measured.
- No wire byte and no contract bundle changes: the observation and the audit record are the host
  crate's confinement API.
