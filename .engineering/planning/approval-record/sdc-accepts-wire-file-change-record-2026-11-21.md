---
format: aep.planning-md/3
id: approval-record:sdc-accepts-wire-file-change-record-2026-11-21
kind: approval-record
status: active
title: Session sdc accepts the wire story of the file-change record and answers design 35 (2026-11-21)
tags:
- sdc
relations:
- decides: story:exec-file-change-record-on-the-wire
- decides: story:linux-executable-allow-list
- decides: story:executable-deny-list
revision: 2
transitions:
- {from: "draft", to: "active", at: "2026-11-21T03:16:04Z", actor: "human:timo", revision: 2}
---
# Session sdc accepts the wire story of the file-change record, and answers design 35

## Decision

Session `sdc` wrote to the substrate session on 2026-11-21, with the authority
`approval-record:sdc-decides-plan-and-design-2026-11-21` records:

> Wire story: story:exec-file-change-record-on-the-wire is accepted under the operator's standing
> rule (accepted on creation; record actor agent:sdc, executor agent:substrate, correlation
> operator-standing-acceptance-2026-11-21).

`story:exec-file-change-record-on-the-wire` moved to active with that executor and correlation.
The store wrote `actor: human:timo` on both transitions; `aep plan artifact move` has no flag for
the actor. The decision was session `sdc`'s, not typed by the operator.

## Its answers to design document 35, same message

- Commit the document as the bot and open its own pull request for review.
- Q1: the helper is a new binary of this repository, `substrate-exec-gate`, named in `HostConfig`.
  The embedder notes state the install requirement. A profile that carries a list with the helper
  missing or not executable fails closed with a named error.
- Q2, Q3 and Q4 are security defaults and the operator's. Until he answers, the document carries
  each recommendation as "provisional, pending the operator", and stories 1 and 2 build on it:
  the linux and macos difference for an empty list stays and is stated; no `memfd_create` refusal
  in stories 1 and 2; the hard-link limit is stated. None of the three is published as a wire
  property or in release notes before he has answered.
- Q5: the lists are for an honest tool chain. Containment of hostile code stays with the
  namespaces, the seccomp program and the read-only view. The document says so in its first
  section and lists the probed limits. It also names the options that could make a denied
  execution visible after the run, with their cost; nothing is implemented.
- The story corrections from the probes are accepted as listed.

## Not covered

No release, tag, merge to `main` or version number.
