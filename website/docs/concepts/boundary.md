---
title: System boundary
description: Where caller intent stops and Substrate's execution data-plane responsibility begins.
---

# One machine, one execution boundary

Substrate owns execution mechanics and observed lifecycle state. It does not own the intent that led
to a request.

The [system model](./model.md) maps the actual daemon, store, host driver, SDK and MCP adapter.
For a concrete request, the caller chooses to hash a file; Substrate owns whether the workspace
and process can be confined and what the process actually did.

This dependency direction stays one-way. Consumers integrate through the service contract; the data
plane does not import product behavior.

## What Substrate owns

| Concern | Responsibility |
|---|---|
| workspaces | identity, confined filesystem root, guarded files, lifecycle |
| execs | argv-only start, sandbox requirements, limits, signals, output, terminal state |
| sessions | leased raw-pipe and probe-gated PTY modes with one bounded attachment |
| operations | caller-minted retry identity and durable outcome |
| events | typed transitions with bounded replay |
| capabilities | facts probed from the active backend and configuration |
| leases | explicit liveness and typed expiry |

## What stays above the boundary

- deciding whether the requested action serves a product goal;
- rich authorization and grant policy;
- choosing a machine from a fleet;
- model turns, tool loops, and workflow behavior;
- organization membership, quotas, and billing;
- public routing and product ingress.

Substrate still authenticates its local caller, scopes resources to that subject, checks
preconditions, and enforces its own limits. “Policy lives elsewhere” never means “run unauthenticated
or unbounded.”

## One contract, capability-gated behavior

The shipped driver is the Linux host driver. The contract separates its resource model from
backend-specific enforcement. A caller should ask which capabilities this daemon
verified, not which driver happens to be behind it. An absent capability produces `unserved` or a
more specific refusal.

That preserves honest substitution:

- a personal Linux host may serve guarded workspaces and confined exec;
- a host without cgroup delegation may still serve workspace operations but not exec;
- future container or cluster drivers may serve more resource families without changing the caller's
  product policy.

See [the contract surface](../reference/contract.md) for the currently served families and
[status](../status.md) for capabilities that are still absent.
