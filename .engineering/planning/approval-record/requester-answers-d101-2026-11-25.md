---
format: aep.planning-md/3
id: approval-record:requester-answers-d101-2026-11-25
kind: approval-record
status: active
title: The requester's answer on the rule for an oci memory-bound kill without the flag (2026-11-25)
summary: 'D101 = A: the observation states a kill it cannot attribute; the gate''s case asserts Memory or that statement; an exact source is a draft, not ordered'
relations:
- decides: story:an-oci-run-killed-at-its-memory-bound-names-the-limit-on-a-hosted-runner
revision: 2
transitions:
- {from: "draft", to: "active", at: "2026-11-25T19:06:42Z", actor: "agent:substrate", revision: 2}
---
# The requester's answer on the rule for an oci memory-bound kill without the flag (2026-11-25)

## Who decided

The requester of `epic:runs-are-governed-and-limit-hits-are-events`, the stakeholder
`story:an-oci-run-killed-at-its-memory-bound-names-the-limit-on-a-hosted-runner` names, under the
rule that the stakeholder of a request decides for the work it requested (`AGENTS.md`
§ *Planning artifacts*).

## The question

Asked on 2026-11-25 as decision D101, after the measurement the story's step 1 orders, with
option C as the default: which rule replaces design 36 D7's `oci` rule for a memory-bound kill?

- A: the stated reason only. A run that ended by `SIGKILL` which neither the monitor nor the
  embedder sent, with no flag, keeps `limit: None`, and the observation gains a member that says
  the kill's cause was not read from the runtime. The gate's case then asserts "Memory, or that
  statement". About one memory hit in four under load stays unattributed on `oci`.
- B: an exact source. The adapter runs the command under a small static helper inside the
  container, named by configuration, which reads its cgroup's `oom_kill` after the command was
  killed and reports it to the adapter. Exact where the helper is configured; without it the
  adapter answers as in A. It needs its own design document, a new helper binary and a build
  step in the gate.
- C: A now in this story, and B as its own story with its design document, ordered by the
  requester.

## The answer

> D101 = A now (the stated-cause member; the gate case asserts "Memory, or that statement"). B
> filed as a draft only, not ordered: no Runbook story or consumer gap needs exact memory
> attribution on `oci` today, and a helper binary inside every `oci` run is architecture for
> [the repository owner]. […]

The words in brackets stand for a name. The sentence left out says how a later request for B
travels: the requester asks, and the question goes to the repository owner.

## What was read before the question

One job on a standard hosted runner, run 45659093178 of 2026-11-25, started from the branch
`measure/oci-oom-flag`, which is not merged: 610 runs of the shape of the gate's case
`an_oci_run_over_its_memory_bound_names_the_memory_limit` with the runtime's own client. The
story's *Measured* section and design 36 § 12 hold the figures. In short:

- the runtime's `OOMKilled` flag was at the first read of the state in 482 runs, at a later read
  within 5 s in none, and in no read in 128;
- the runtime's `oom` event was sent for the same 482 containers;
- a reader on the host of the container's `memory.events` read the kill in 66 of 100 runs;
- a command inside the container read `oom_kill` 1 in its own cgroup in 100 of 100 runs.

## What follows

- Design 36 is amended before code: D27 (the member `limit_unread`), D28 (the monitor reads the
  runtime's state once), the `oci` note of D7, and the measurement in § 12.
- The story's step 3 changes with the answer: the gate's case stays in the gate and gets no
  retry, and its assertion becomes "`Memory`, or the stated member".
- `story:an-oci-run-reads-its-memory-kill-from-inside-the-container` is filed as a draft and is
  not ordered. Starting it is a question for the repository owner, because it adds a binary that
  runs inside every `oci` run.
