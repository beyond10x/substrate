---
title: What is Substrate?
description: A standalone execution data plane for confined workspaces, bounded processes, durable operations, and observed state.
---

# Run things. Report what happened.

Substrate turns one Linux machine into a governed execution service. A caller asks for a bounded
operation; Substrate admits or refuses it, dispatches it through a verified driver, then reports what
the machine actually observed.

The distinction matters. A process runner can say “started” while its child escaped, its output was
lost, or its final state is unknown. Substrate keeps requested state separate from observed state and
names uncertainty instead of filling it with optimism.

For example, create a guarded workspace, write an input file, then ask Substrate to run
`sha256sum` under explicit bounds. The workspace, the start operation and the running exec have
different identities and lifetimes. [Follow that request](./concepts/operations.md) or
[try guarded file I/O first](./getting-started.md#try-a-workspace-without-process-execution).

## The boundary in one table

| Layer | Owns | Does not own |
|---|---|---|
| caller | intent, product policy, placement choice | machine confinement |
| Substrate | resource lifecycle, admission, durable operations, bounded execution, observations | agent loops, scheduling, vendor policy |
| driver | verified enforcement on the selected machine | a different public contract |

Every deployment serves the same contract. Features appear only when the running daemon has probed
the capability needed to enforce them.

## What it gives a caller

- **Confined workspaces.** File access remains below a daemon-owned root; path and symlink escape
  attempts are refused.
- **Bounded processes.** Commands use an argument vector, a cleared and shaped environment, output
  caps, process, memory, CPU and time bounds, hard opt-in writable-storage quotas, and whole-tree
  termination.
- **Durable mutations.** An operation is recorded before driver dispatch, so a broken connection
  does not force the caller to guess whether an effect began.
- **Observed answers.** Terminal state, exit status, truncation, expiry, and uncertainty are data.
- **Exact resource facts.** Opted-in runs expose kernel CPU, memory, process, OOM and block-I/O
  observations during and after execution; Substrate does not invent a mean from samples.
- **Verified capability facts.** A deployment advertises only what it has probed and can enforce.
- **Leased resources and events.** Abandoned work becomes a typed transition, and consumers can
  replay a bounded event history.

## What it deliberately leaves out

Substrate is not a scheduler, agent loop, policy engine, identity provider, credential broker, or
public ingress product. The current implementation governs one Linux host. Higher layers decide
why work should happen and where it should be placed.

## Choose a path

- [Run a local daemon](./getting-started.md) and inspect its verified machine facts.
- [See what you can build](./use-cases.md), from test workers to file processors and agent tools.
- [Run an ordinary binary](./guides/run-a-command.md) with terminal-visible bounds and metrics.
- [Understand the system boundary](./concepts/boundary.md) and [model coverage](./concepts/model.md).
- [See how confinement becomes refusal](./concepts/confinement.md).
- [Follow a mutation from operation to observation](./concepts/operations.md).
- [Check the current status and limitations](./status.md).
