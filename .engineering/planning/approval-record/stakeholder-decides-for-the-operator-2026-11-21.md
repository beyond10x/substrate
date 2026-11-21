---
format: aep.planning-md/3
id: approval-record:stakeholder-decides-for-the-operator-2026-11-21
kind: approval-record
status: active
title: 'Operator: the stakeholder of a request decides on his behalf (2026-11-21)'
tags:
- sdc
relations:
- decides: epic:runs-are-governed-and-limit-hits-are-events
revision: 2
transitions:
- {from: "draft", to: "active", at: "2026-11-21T03:38:39Z", actor: "human:timo", revision: 2}
---
# Operator: the stakeholder of a request decides on his behalf

## Decision

The operator, Timo, wrote in the substrate session on 2026-11-21, after that session had put three
security defaults of design 35 to him:

> all your decisions, they should be made by whoever submitted the request or the thing you
> actually fixed. So that could be SDC, it could also be AI agent platform. Each of them would be
> capable of deciding stuff, but the stakeholder of that ticket should actually decide on behalf
> of me.

## What it changes

- A decision the substrate session would have put to the operator goes to the stakeholder of the
  work instead: the session or person that submitted the request the artifact came from. That
  stakeholder's answer counts as the operator's.
- For `epic:runs-are-governed-and-limit-hits-are-events` and the stories around it the
  stakeholder is session `sdc`. This includes the three security defaults of
  `docs/design/35-executable-lists.md` that session `sdc` had passed back to the operator.
- It is not limited to session `sdc`: the operator names "AI agent platform" as another possible
  stakeholder.

## The substrate session's reading, not the operator's words

- The earlier limits stand, because this message does not lift them: merges, pushes to `main`,
  releases, tags and version numbers stay the operator's
  (`approval-record:sdc-decides-plan-and-design-2026-11-21`).
- A stakeholder decides for the work it requested, not for another stakeholder's.
- An artifact names its stakeholder in its body, under "Where the requirement comes from".
