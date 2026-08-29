---
format: aep.planning-md/1
id: story:ledger-rows-carry-the-declared-grant
kind: story
status: draft
title: An operation's ledger row carries the declared grant it ran under
summary: 'Atlas O1 exit evidence: every effectful call attributable to a declared grant, verified across the connectors/identity seam from substrate''s own record; design 06 section 2 fixes the shape.'
owner: substrate
tags:
- ledger
- o1
- trust
revision: 2
---
# Story: An operation's ledger row carries the declared grant it ran under

## Outcome

From substrate's own durable record, a reader can answer atlas O1's exit question for every
effectful operation: *which declared grant authorised this, on behalf of which platform principal,
through which immediate actor* — and an operation presented without a grant reference where the
deployment requires one is a named refusal, not a missing column.

## Context

`atlas/ROADMAP.md` O1 — *governed reach* — names substrate an owner and states the exit evidence:
"every effectful call in a run's record is attributable to a declared grant; a call outside it is
a named refusal, not a missing row". Its state column says "connectors grants and identity
audiences: not yet verified across a seam". Substrate's side of that seam is already designed:
`docs/design/06-authentication-secrets-and-trust.md` § 2 — the initiating platform principal is
retained separately from the immediate service actor "through tamper-safe delegated context;
caller-written identity strings are not trusted", and every ledger key is scoped by deployment
and authenticated subject. Nothing in the backlog implemented that context on the wire or in the
ledger; this is the one item that moves the objective substrate serves (`AGENTS.md` § *Serves*).

## Acceptance

An exec or session operation started with a delegated-context document records the grant
reference and platform principal in its ledger row and in its `operation.*` events, the same
bytes a connectors-side fixture presents, and a test proves a caller-written identity string in
the request body is ignored in favour of the verified context.

Evidence that satisfies it, in order:

1. **Before code** (invariant 8): an ADR fixing the delegated-context shape — who signs or seals
   it, what substrate verifies (audience, expiry, binding to the authenticated subject), and the
   refusal class when it is required and absent or invalid. Coordinated with `connectors` and
   `identity` under `atlas/AGENTS.md` § *Cross-repo changes*; the ADR names both relying parties.
2. A successor bundle adds the optional `delegated_context` request field, the ledger/event
   fields `grant_ref` and `platform_principal`, and the refusal; earlier bundle bytes unchanged.
3. Failing-first tests: `ledger_row_records_grant_ref_and_platform_principal`,
   `caller_written_identity_is_ignored_when_context_is_verified`,
   `missing_delegated_context_is_refused_by_name_when_required`,
   `delegated_context_bound_to_another_subject_is_refused`.
4. A conformance vector pair: substrate's fixture and a connectors-owned copy, byte-identical, so
   the atlas O1 row can cite both sides.

## Out of Scope

Grant *evaluation* — connectors decides; substrate records and verifies binding only (design 06
§ 2: "a higher-layer permit cannot override" substrate's local checks, and substrate never
becomes the authorization engine). Hosted identity material (phase 7).

## Open Questions

Whether the delegated context is a signed JWT-shaped token from identity or a sealed blob from
connectors. Decides: the ADR, with both owners. Default if nobody answers: **identity-signed,
audience `substrate`**, because atlas O1 already pairs "connectors grants and identity audiences".
