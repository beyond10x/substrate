---
format: aep.planning-md/3
id: story:a-session-operations-acceptance-is-published-as-accepted
kind: story
status: draft
title: A session operation's acceptance is published as operation.accepted
tags:
- events
revision: 1
---
## Outcome

The events of a session operation are the operation's own, as for every other kind: one
`operation.accepted` at its acceptance, and one terminal event (`operation.terminal`,
`operation.failed` or `operation.unknown`) at its end. The projection the store documents for
a session's transitions (`published_transition` in `crates/substrate-store/src/events.rs`:
a session's transition is published as the operation-ledger projection) applies to the
transitions of the session resource only (`session.*`), and leaves a transition that starts
`operation.` as it is.

## Why

A reader of the event stream that follows one operation waits for its terminal event. The
rule above lets it treat a session operation as it treats an exec or a workspace operation,
with no table of kinds. ADR 0053 decision 8 states the events of a completion for every path;
this story states the one event before them.

## Before code

This changes the published event stream of every session operation, so invariant 8 applies:

- name the consumers that read session operation events (the Rust SDK's event reader, the
  CLI, and each repository of the collection that pins the wire crate or the SDK) and what
  each does with the first event of a session operation;
- say whether the change needs a successor bundle or is inside what the current bundle
  publishes for the closed event union (the event schema and the vectors of the advertised
  bundle decide it), and whether any executed contract vector moves;
- record the answer as an amendment of the design document that owns the session event
  projection, or as an ADR, and put the decision to this story's stakeholder.

## Acceptance

- A case in `crates/substrate-store/src/` holds the literal event list of a session start, a
  session lease renewal, a session observation and a session retire, each recorded and
  unrecorded: `operation.accepted` first, then the completion's events as ADR 0053 decision 8
  states them, no event twice.
- The case the review wrote,
  `a_session_operations_acceptance_is_published_as_operation_accepted`, is in the tree and
  green. Its text is in the review record.
- The `session.*` projections (`session.unknown`, `session.cleanup-failed`, every other
  `session.*`) publish what they publish now, held by a case each.
- The contract vectors and the runtime vectors that read a session operation's events pass,
  with the counts of both lanes stated.
- `CHANGELOG.md` names the change under *Changed*, with what a reader of the stream sees.

## Where the requirement comes from

The confirming adversary pass on ADR 0053's store change, 2026-11-24
(`review-result:result-bound-store-confirming-adversary`). Stakeholder: the session that
works this repository; a decision this story needs goes to the operator's session.

## Scope

`crates/substrate-store/src/events.rs`, the store's event cases, the daemon's session event
cases, `CHANGELOG.md`, and the design document or ADR named under *Before code*.
