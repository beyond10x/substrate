---
format: aep.planning-md/3
id: story:namespace-lane-role-cases-do-not-fail-on-one-slow-answer
kind: story
status: draft
title: The namespace lane's Role cases do not fail on one slow answer of the cluster
summary: One hosted run failed a Role case after 120 s with kubernetes.api-failed; the daemon log was not kept
tags:
- defect
- flaky
revision: 1
---
# Story: the namespace lane's Role cases do not fail on one slow answer of the cluster

## Outcome

`cargo xtask namespace-lane observe` does not fail a case of decision 13's Role table because the
cluster answered once with an error other than the refusal the case waits for.

## What was seen

Pull request 258, first head `45f5f29e3`, hosted `Namespace lane on kind`: the case
`S6 Role without persistentvolumeclaims delete` failed with "nothing refused within 120 s: no
WARN line after the first probe carries `cannot delete resource "persistentvolumeclaims"`", codes
`{"kubernetes.api-failed"}`. The change under test moved versions and text only. The next head,
which differed in `Cargo.lock`, passed the same lane, and so did the run on `main` for the
release. Seen once. The cause is not established: the daemon's log of that run was kept on the
runner and is gone.

## Where the requirement comes from

The collection's rule "The local gate equals the forge": a forge run that goes red gets the change
that makes it not recur. A release needs this lane green on `main` for the tagged commit.
Stakeholder: the operator.

## What changes

1. The lane uploads the kept daemon log as an artifact when a case fails, so the next occurrence
   can be read.
2. With one log in hand: decide whether the case waits for the wrong line, whether the driver
   reports an API failure where it should retry, or whether 120 s is short on a loaded runner.
   No timeout is raised before that.

## Acceptance

A failed case of the lane leaves its daemon log as a workflow artifact; the story is closed with
the cause when it is seen again, or after ten release runs without it.
