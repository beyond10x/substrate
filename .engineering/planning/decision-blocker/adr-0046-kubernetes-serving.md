---
format: aep.planning-md/3
id: decision-blocker:adr-0046-kubernetes-serving
kind: decision-blocker
status: cleared
title: 'Operator decides ADR 0046: node-bound Kubernetes serving profile'
summary: 'Decider: operator. Nine decisions (K1-K9) in adr/0046 on branch design/ops-listener-and-k8s-serving.'
relations:
- blocks: story:node-bound-kubernetes-serving-profile
withholds: test_result
revision: 2
transitions:
- {from: "open", to: "cleared", at: "2026-10-28T08:44:29Z", actor: "human:timo", revision: 2}
---
# Operator decides ADR 0046: node-bound Kubernetes serving profile
