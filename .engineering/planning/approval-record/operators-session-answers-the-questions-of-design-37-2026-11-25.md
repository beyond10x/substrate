---
format: aep.planning-md/3
id: approval-record:operators-session-answers-the-questions-of-design-37-2026-11-25
kind: approval-record
status: draft
title: The operator's session answers the six questions of design 37 (2026-11-25)
relations:
- decides: story:exec-start-carries-executable-lists
revision: 1
---
# The operator's session answers the six questions of design 37 (2026-11-25)

Design 37 (`docs/design/37-executable-lists-on-the-wire.md`, § 15) lists six questions that have
more than one defensible answer. The stakeholder of
`story:exec-start-carries-executable-lists` is the session `sdc`. It was not running on
2026-11-25. Under the collection's rule for a question with a default, this session put the six
to the operator's session as one selection at 01:03 CET, with A as the default.

## The selection as put

- A (default): all six as the document writes them.
  - Q1: the two members stand in `sandbox`.
  - Q2: a list the host did not prove is `confinement.fact-unserved` (501).
  - Q3: the three defaults of design 35 § 19 stand, and the published text states them.
  - Q4: the allow-list is bounded at 64 entries on the wire and in the adapter API.
  - Q5: the release image and the chart are a story of their own.
  - Q6: the capability snapshot names why a fact is absent with fact, cause and configuration
    field, without a host path.
- B: as A, but this story also ships the helper in the daemon image and a chart value.
- C: hold all six until `sdc` runs again.

## The answer, as received from session `host-operator` at 01:15 CET

> D88 = A, taken by me under Timo's delegation: all six as the document writes them. Reason: a
> design document, reversible, `sdc` is not running, and Q5 as its own story keeps the image
> and chart work out of this change set. Write in § 15 that the six answers are host-operator's
> on default and that `sdc` may reopen any of them when it runs again.

## What it decides

- Design 37 is accepted as written, and `story:exec-start-carries-executable-lists` may be
  planned on it.
- The six answers are the operator's session's, not the stakeholder's. Session `sdc` may reopen
  any of them when it runs again; a reopened answer is a revision of design 37 before the
  bundle is cut, or a successor bundle after.
- Q3 confirms design 35's three defaults (its Q2, Q3 and Q4) for the published property, which
  design 35 § 15 required before they reach a wire property.
