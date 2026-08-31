---
title: What you can build
description: Practical systems that use Substrate as a confined execution data plane, with and without agents.
---

# Put a governed process boundary under many kinds of automation

Substrate is useful anywhere a service needs to run a command but should not inherit the host's full
filesystem, network, process tree, or resource budget. An agent can be the caller, but it does not
have to be. The API accepts an argument vector, explicit bounds, a workspace, and a capability
snapshot; it returns durable operations and observed state.

## Build and test workers

Run compilers, linters, test binaries, and package inspection in isolated workspaces. A worker can:

- copy source into a guarded workspace;
- execute a compiler directly with CPU, memory, process, output, and duration ceilings;
- retain bounded stdout and stderr after the client disconnects;
- read exact terminal CPU, peak-memory, process, I/O, and wall-clock observations;
- destroy or expire the workspace when the job is done.

Substrate does not schedule the worker fleet or choose which tests to run. It gives the scheduler a
small, machine-enforced execution primitive.

## File and document conversion

Wrap tools such as image converters, archive inspectors, renderers, and document processors without
giving each one ambient access to the machine. Input can live in `/workspace`, runtime material can
be supplied read-only at `/runtime`, and disposable output can use quota-bounded `/scratch`.

This is especially useful for formats that are valid input to a mature native binary but still
come from an untrusted source. The binary is not made safe by declaration; the surrounding process
and filesystem reach are constrained by the host driver.

## Extension and plugin execution

Give a plugin one workspace, no network by default, a cleared environment, and a bounded process
tree. If it needs one external service, an operator can expose a named, destination-bound aperture
rather than general egress. The caller chooses a declared aperture by name; it cannot submit a host
or port.

Substrate does not decide which plugin is trusted or approved. A product or policy layer makes that
decision before asking Substrate to execute it.

## Batch and data-processing steps

Run a native CLI over local input with a measurable resource envelope. Exact counters make it
possible to answer questions such as:

- Did the command finish before its wall-time and CPU budgets?
- What peak memory did the kernel observe?
- How many processes existed at peak, and did the process limit reject any forks?
- How many bytes reached block devices through the cgroup?
- How much quota-accounted scratch space remained at completion?

There is deliberately no “mean memory” field. The kernel exposes exact current and peak values;
Substrate does not synthesize an average from samples and present it as an exact fact.

## Agent tool execution

An agent runtime can map tool calls to Substrate operations and keep model reasoning outside the
execution boundary. The same properties are useful here—argv-only execution, no ambient identity,
durable effects, bounded output, explicit reach, and named refusal—but Substrate never runs the
agent loop or decides whether a tool call should be approved.

## What it is not

Substrate is not a general-purpose container orchestrator, a multi-tenant public sandbox, a billing
meter, or a fleet scheduler. One daemon governs one trust domain on one handed-over machine scope.
Machine facts tell a caller which guarantees that deployment actually proved.

Ready to try the primitive directly? [Run a bounded command](./guides/run-a-command.md), then read
[storage quotas and resource metrics](./guides/storage-and-metrics.md).
