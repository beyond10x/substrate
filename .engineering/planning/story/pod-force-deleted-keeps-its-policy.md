---
format: aep.planning-md/3
id: story:pod-force-deleted-keeps-its-policy
kind: story
status: draft
title: A force-deleted Pod does not release its run's NetworkPolicy
relations:
- decomposes: epic:confinement-contract-family
revision: 1
---
# Story: a force-deleted Pod does not release its run's NetworkPolicy

## Observation

The security review of the Pod Unknown cleanup fix
(`review-result:pod-unknown-cleanup-security-pass-1`, finding J2, pre-existing) found that the pod
adapter treats any 404 on a Pod read as "the Pod is gone" and then deletes the run's NetworkPolicy
by name, with no uid precondition. A Pod removed by a forced delete (PodGC after a node is lost, or
an operator's forced delete) reads 404 while its container may still run on a partitioned node.
The policy that confines that container's egress is then removed.

The adapter never force-deletes, and run names are unique, so no path inside the adapter reaches
this. The monitor, the open sweep and the deferred sweep all apply the same rule.

## Acceptance

A run's NetworkPolicy outlives a Pod that was removed while its node was unreachable, until the
adapter can tell the container is gone (for example: the node is Ready again, or the node object
is gone), or the decision to accept this is recorded in design 28 with its reason. A fake-API case
holds whichever rule is chosen, and runs once in seconds.

## Not known

Whether the API server exposes enough (Pod deletion grace, node condition) for the adapter to tell
a forced removal from a normal one without new RBAC.
