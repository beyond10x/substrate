---
format: aep.planning-md/3
id: approval-record:sdc-decides-design-35-defaults-2026-11-21
kind: approval-record
status: active
title: Stakeholder (agent:sdc) decides the three defaults of design 35 (2026-11-21)
tags:
- sdc
relations:
- decides: story:linux-executable-allow-list
- decides: story:executable-deny-list
revision: 2
transitions:
- {from: "draft", to: "active", at: "2026-11-21T03:45:10Z", actor: "human:timo", revision: 2}
---
# Stakeholder decides the three defaults of design 35

## Who decided

Session `sdc` (agent:sdc), not a person, on 2026-11-21, as the stakeholder of
`epic:runs-are-governed-and-limit-hits-are-events`, under
`approval-record:stakeholder-decides-for-the-operator-2026-11-21`. Session `sdc` did not hear the
operator's ruling itself; it decided on the substrate session's relay of his words, and it is
telling the operator what it decided and that he can overturn it.

## The decisions, in its words

> Q2: A for stories 1 and 2 — an empty allow-list on linux keeps today's behaviour; the
> linux/macos difference is stated in the document and in the profile's reference text. C (absent
> keeps today's behaviour, explicitly empty means the program only) is drafted as a follow-up
> story with its cost (ConfinementProfile.exec_subpaths must tell absent from empty; spec change),
> status draft, not in this slice; I decide it when the wire story is planned. B is rejected: it
> changes every existing linux run.
> Q3: A — no memfd_create refusal. The document states that refusing it would not close the route
> (a copy into a writable listed directory still runs).
> Q4: A — state the hard-link limit; no refusal by link count.
> All three leave existing behaviour unchanged. Remove "provisional, pending the operator" and
> write "decided by the stakeholder (agent:sdc), 2026-11-21, revisable before the wire story
> publishes the property". Releases, tags, merges to main and version numbers stay the
> operator's, as you read it.

## What follows

- `docs/design/35-executable-lists.md` records the three as decided by the stakeholder and
  revisable before `story:exec-start-carries-executable-lists` publishes the property.
- `story:explicitly-empty-exec-subpaths-means-the-program-only` is drafted for option C and stays
  draft.
- Session `sdc`'s review of the document, the same day, accepts the design once its points 1 to 5
  are in the document. That acceptance is recorded when the revision is on pull request 240.
