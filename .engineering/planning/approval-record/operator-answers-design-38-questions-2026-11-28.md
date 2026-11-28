---
format: aep.planning-md/3
id: approval-record:operator-answers-design-38-questions-2026-11-28
kind: approval-record
status: draft
title: The operator answers the four questions of design 38
summary: 'D1-D4 = A: confined workload family, in-daemon archive unpacker, inbound relay, per-run secret value in memory.'
owner: substrate
relations:
- decides: epic:long-lived-service-on-the-host-driver
revision: 1
---
# Approval record: the operator answers the four questions of design 38

## Who decided

The operator, 2026-11-28 11:08 CET, relayed by the operator's session, on the four questions of
`docs/design/38-long-lived-service-on-the-host-driver.md` § 15.

## What was decided

- **D1 = A**: a confined workload family on the host driver, with restart supervised by the daemon.
- **D2 = A**: the daemon unpacks the sha256-named archive itself, in Rust.
- **D3 = A**: an inbound endpoint is a daemon relay through a Unix socket into the run's network
  namespace; the run stays without egress.
- **D4 = A**: a secret value travels in the start request, is held in memory only and reaches the
  run as a memfd; the rule of design 01 § 7.7 that secret slots are names, never values, changes for
  per-run values.

## What follows

Each answer gets an ADR before code of its slice (invariant 8): ADR 0055 (root filesystems, D2),
ADR 0056 (workloads, D1), ADR 0057 (inbound endpoints, D3), ADR 0058 (per-run secret values, D4).
The work is built in integration batches, one pull request per batch.
