---
format: aep.planning-md/3
id: approval-record:pod-orphan-policy-sweep-d124
kind: approval-record
status: draft
title: Open sweeps orphaned labelled NetworkPolicies (decision D124)
relations:
- decides: story:pod-unknown-cleanup-outlives-adapter
revision: 1
---
# Decision: an adapter's open sweeps orphaned NetworkPolicies

Taken 2026-10-28 by the orchestrating session under the operator's standing grant (its decision
D124), answering `decision-blocker:pod-orphan-policy-sweep`.

Open may sweep. Scope:

- only NetworkPolicies in the adapter's configured namespace;
- only policies carrying substrate's own owner label;
- only when the Pod named by the policy's run label no longer exists;
- never an unlabelled policy, never one whose Pod exists in any phase.

Rests on the story's own acceptance ("Open sweeps the namespace for labelled NetworkPolicies whose
Pod no longer exists") and on finding A3 (`crates/substrate-host/src/confinement/pod.rs`): without
the sweep a policy outlives the process that held the adapter.

Implementation condition added by the coordinating session: a live adapter creates a run's policy
before its Pod, so the sweep also skips a policy younger than the adapter's API timeout, so that it
never removes the default-deny policy of a run another adapter is still starting.
`docs/design/28-pod-confinement-adapter.md` § *Not decided here* is amended in the same change.
