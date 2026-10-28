---
format: aep.planning-md/3
id: decision-blocker:adr-0045-operational-listener
kind: decision-blocker
status: cleared
title: 'Operator decides ADR 0045: operational health and metrics on a separate listener'
summary: 'Decider: operator. Six decisions (D1-D6) in adr/0045 on branch design/ops-listener-and-k8s-serving.'
relations:
- blocks: story:operational-health-and-telemetry
withholds: test_result
revision: 2
transitions:
- {from: "open", to: "cleared", at: "2026-10-28T06:16:11Z", actor: "human:timo", revision: 2}
---
# Operator decides ADR 0045: operational health and metrics on a separate listener
