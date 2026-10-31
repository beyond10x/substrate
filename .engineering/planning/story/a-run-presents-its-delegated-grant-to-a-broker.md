---
format: aep.planning-md/3
id: story:a-run-presents-its-delegated-grant-to-a-broker
kind: story
status: draft
title: A confined run presents its delegated grant to a broker
summary: 'Glovebox wave 5: an ADR decides how a run-scoped grant token reaches the child so it can call a Connectors broker through an aperture without holding a credential.'
owner: substrate
tags:
- delegated-context
- glovebox
revision: 1
---
# Story: a confined run presents its delegated grant to a broker

Part of Glovebox (atlas `initiative:glovebox`, `docs/design/glovebox.md`), wave 5.

## Why

- Glovebox requirement 3: a process in the box can "list the operator's GitLab merge requests
  **without ever seeing the credential**, which lives beside the daemon"
  (`atlas/docs/design/glovebox.md:23-25`). Rule SM-D201: "API calls such as GitLab merge requests
  go through a Connectors broker" (`atlas/docs/design/glovebox.md:58-59`). Wave 5 names "a broker
  the box calls with a per-run grant (Substrate ADR 0011 delegated context)"
  (`atlas/docs/design/glovebox.md:94`).
- Checked first: Substrate cannot do this today. ADR 0011's delegated-context document is
  presented **to Substrate** by the client, "as a `delegated_context` member beside `op` and
  `input`" (`adr/0011-delegated-context-and-grant-attribution.md:22-30`), with audience exactly
  `urn:b10x:substrate` (`docs/design/09-delegated-context-and-grant-attribution.md:83-87`).
  Substrate verifies it and stores the grant reference on the ledger row; it "does not evaluate the
  grant, resolve it, or call any issuer" (`adr/0011-delegated-context-and-grant-attribution.md:32`).
  Nothing hands the document or any derivative to the child: `delegated_context` occurs in the
  daemon, wire, SDK and CLI crates and nowhere in `crates/substrate-host/` (`grep -rl
  delegated_context crates`, 2026-10-29). Its audience would also make a broker refuse it, if the
  broker checks audience (inferred).
- A secret slot does not fill the gap: a slot is "daemon configuration, never request data"
  (`docs/design/11-sealed-secret-slots.md:42-46`), so it cannot carry a per-run value chosen at
  start time.
- An egress aperture already reaches one declared destination by name
  (`adr/0013-egress-apertures-are-declared-by-the-operator.md:22-24`), so the broker can be
  reached; what is missing is a run-scoped credential the child may present to it.

## What changes

1. **An ADR first** (invariant 8). It decides at least:
   - what the child holds: a run-scoped grant token minted for the broker's audience, bound to this
     exec (operation id, subject, deployment) and expiring with it; who mints it (Connectors, as
     ADR 0011 already allows it to sign, `adr/0011-delegated-context-and-grant-attribution.md:41-43`,
     or Identity), and whether Substrate relays a caller-supplied token or derives one;
   - how it reaches the child: a per-start sealed descriptor alongside the existing secret slots,
     or a file in the read-only `/runtime`, and that it never appears in the environment, argv, the
     ledger or an observation;
   - that it is useless outside the run: bound to the aperture it is presented through, refused
     after the exec ends;
   - the capability fact, the start field, the applied record and the named refusals.
2. After the ADR is accepted, a successor contract bundle (invariant 6) and the host
   implementation.

## Acceptance

- The ADR is accepted before code, and names the Connectors story that verifies the token.
- Portable lane: the fact is absent and a start that asks for a run grant is refused by name.
- Delegated lane: a child reads its run grant from the agreed place, presents it to a test broker
  reached through a declared aperture, and the broker sees the exec's binding in it; the same
  token is refused after the exec ends; the token is absent from the ledger row, the events and
  every observation.

## Blocked on / Depends on

- Blocked on the ADR above (invariant 8).
- Pairs with Connectors' `story:broker-answers-a-confined-run-without-handing-over-the-credential`,
  which verifies the token and attaches the credential beside the daemon. Atlas wave 5 waits on
  Connector credentials in Secrets (`atlas/docs/design/glovebox.md:94`).
