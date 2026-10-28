---
format: aep.planning-md/3
id: decision-blocker:pod-orphan-policy-sweep
kind: decision-blocker
status: cleared
title: 'Design 28 leaves restart cleanup undecided: may an adapter''s open sweep NetworkPolicies whose Pod is gone (finding A3)'
relations:
- blocks: story:pod-unknown-cleanup-outlives-adapter
withholds: test_result
revision: 2
transitions:
- {from: "open", to: "cleared", at: "2026-10-28T07:39:26Z", actor: "human:timo", revision: 2}
---
# Design 28 leaves restart cleanup undecided: may an adapter's open sweep NetworkPolicies whose Pod is gone (finding A3)
