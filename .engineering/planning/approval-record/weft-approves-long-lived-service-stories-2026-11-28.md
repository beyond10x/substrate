---
format: aep.planning-md/3
id: approval-record:weft-approves-long-lived-service-stories-2026-11-28
kind: approval-record
status: draft
title: The stakeholder approves the stories of the long-lived service epic
summary: The weft maintainer approves the six story texts and the design direction of gaps 3 and 6; D1-D4 stay with the operator.
owner: substrate
relations:
- decides: epic:long-lived-service-on-the-host-driver
revision: 1
---
# Approval record: the stakeholder approves the stories of the long-lived service epic

## Who decided

The session that maintains the `weft` repository, the stakeholder of
`epic:long-lived-service-on-the-host-driver` (its gap list, `cloudspec` `docs/platform-ports.md`
§ 10, commit `2ed72c2`), on 2026-11-28, under the rule that the stakeholder of a request decides
for the operator on the work it requested (`approval-record:stakeholder-decides-for-the-operator-2026-11-21`).

## What was approved

Quoted: "weft approves the story text of PR #311 as stakeholder. Each acceptance covers weft's
check for gaps 1-6. I also approve the design direction for gap 3 (a listener in the run's netns,
relayed to the agent's per-run socket) and gap 6 (design 01 §3.5)."

- The story text of `story:host-driver-supervised-workload`,
  `story:inbound-endpoint-for-a-confined-exec`, `story:local-endpoint-attributed-to-its-run`,
  `story:a-start-delivers-a-per-run-secret-value`,
  `story:root-filesystem-from-a-digest-named-archive` and
  `story:read-write-volume-kept-across-runs`.
- The design direction of gap 3 and gap 6.

A note, not a change: the acceptance of gap 2 proves `loopback` exposure, which the first consumer
needs; `node` exposure may follow later.

## What it does not decide

The architecture decisions D1 to D4 (`docs/design/38-long-lived-service-on-the-host-driver.md`
§ 15) stay with the operator. No story moves out of draft by this record.
