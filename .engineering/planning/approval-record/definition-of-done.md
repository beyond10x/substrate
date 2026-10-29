---
format: aep.planning-md/3
id: approval-record:definition-of-done
kind: approval-record
status: draft
title: Substrate is done with remote serving, Kubernetes and one adopter
revision: 1
---
# Decision: what makes Substrate done

Taken 2026-10-29 by the operator, choosing option B of three offered in the session:

> "actually, B - we must include k8s too - firecracker/kvm is not part of done - its a follow up"

## Options offered

| Option | Done when |
|---|---|
| A | Remote serving passes clean-room conformance, and one consuming project runs on the remote SDK |
| B (chosen) | A, plus the Kubernetes deployment and driver track |
| C | Every track in `ROADMAP.md` |

## Rule

`ROADMAP.md` § *Definition of done* carries the four conditions. The Firecracker driver, the Docker
driver and hosted composition are follow-up tracks and do not hold up done.

## Open

The Kubernetes namespace driver has no accepted decision yet; it needs its RBAC and ownership
design before code (invariant 8).
