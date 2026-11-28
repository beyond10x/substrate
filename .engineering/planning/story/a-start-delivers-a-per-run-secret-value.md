---
format: aep.planning-md/3
id: story:a-start-delivers-a-per-run-secret-value
kind: story
status: draft
title: A start delivers a secret value for that run only
summary: 'Consumer gap 4: a per-run secret value in the start, delivered as a sealed memfd, never stored or echoed.'
owner: substrate
relations:
- decomposes: epic:long-lived-service-on-the-host-driver
revision: 2
---
# Story: a start delivers a secret value for that run only

## Where the requirement comes from

Consumer gap 4 of the `weft` repository (`cloudspec` `docs/platform-ports.md` § 10, commit
`2ed72c2`): "A secret value the consumer gives one run at its start (C6 delivery through N3):
substrate's secret slots resolve from daemon-config named secrets; weft delivers per-instance
values at start." Two runs on one node need different values. The stakeholder is the session that
maintains `weft`.

## Why

- `docs/design/01-contract.md` § 3.3 and § 7.7: "Secret slots are names, never values"; they
  resolve from daemon-configuration secrets (`story:sealed-secret-slots`, implemented, which
  delivers a value as a sealed memfd).

## What changes

1. An ADR first (invariant 8), after the operator's answer to decision D4 (value in the start
   request, or a write-only store). It changes § 7.7 for per-run values and fixes how a retried
   start and the request hash treat a value.
2. A successor bundle: an optional per-run secret list on the exec, session and workload starts,
   delivered to the child by the sealed-memfd mechanism of `story:sealed-secret-slots`.
3. The value is held in memory only; it appears in no ledger row, request hash, record,
   observation, event, log line or error.

## Acceptance

A weft test (`weft` story `ports-cloud-secret-store-local`) can check:

- A start carries one value; the child reads exactly that value once from its descriptor.
- Two runs started with different values each read their own.
- The value's bytes appear in no response, record, event, log stream, ledger file or daemon log
  (checked by searching every byte the daemon wrote and answered during the run).
- A start naming a per-run secret over TCP development transport, or with the mechanism absent, is
  refused by name before dispatch.

## Blocked on / Depends on

- Decision D4 (operator), then the ADR. After gaps 1, 2, 3 and 5 by the consumer's order.
