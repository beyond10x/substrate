---
format: aep.planning-md/3
id: story:namespace-driver-attributes-a-proof-to-the-node-it-ran-on
kind: story
status: draft
title: The namespace driver attributes a proof to the node its Pod ran on
summary: Probe and exec Pods are bound to a node by name, and the assigned node is read back
tags:
- defect
- security
revision: 1
---
# Story: the namespace driver attributes a proof to the node its Pod ran on

## Outcome

A node-specific proof names the node the probe Pod was assigned to, and an exec runs on that
node. The driver reads the assigned node back from the Pod and refuses, by name, a proof or a
start whose Pod is on another node.

## Design

Owed before code: an amendment to ADR 0047 decision 5, which chose the hostname label. The
amendment states node affinity on `metadata.name` and the read-back.

## Acceptance

Cases on the crate's fake cluster: a node whose hostname label differs from its name, and two
nodes with one label value, each end in the right attribution or a named refusal. Estimated
under 3 s. No kind cluster.


## Where the requirement comes from

The operator's private security review of `main` at `1af046902`, dated 2026-11-22,
finding 7. The operator ordered its findings handled on 2026-11-22. By
`approval-record:operator-decides-how-security-review-fixes-travel-2026-11-22` this story
states the rule that must hold; the review's detail stays with the operator until the fix is
on `main`. Checked against the source at `1af046902` by reading; nothing was run. Release
0.23.0 has the same source. Stakeholder: the operator.
