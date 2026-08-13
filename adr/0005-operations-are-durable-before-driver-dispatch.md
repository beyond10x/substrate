---
status: accepted
date: 2026-08-13
---

# ADR 0005: operations are durable before driver dispatch

## Context

A client can lose an answer at every point around a host mutation. Retrying under a new id or
claiming exactly-once driver execution would duplicate work or fabricate state.

## Decision

Substrate durably commits the subject-scoped operation id, canonical request hash, admitted
capability/config snapshot, and accepted state before driver dispatch. Terminal resource
observation, operation outcome, and event are committed together before a terminal answer.

After restart, accepted/nonterminal work is reconciled from driver observation. It becomes terminal
only with proof; otherwise it remains `unknown` and is never automatically repeated.

## Consequences

- Same id/different request is conflict; same id/same request returns the original logical outcome.
- Event visibility cannot precede readable state.
- Driver calls are at-least-once in the failure model only when an explicit operation defines safe
  recovery; the generic daemon never pretends host side effects are exactly once.
