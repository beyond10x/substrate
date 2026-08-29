---
status: accepted
date: 2026-08-29
---

# ADR 0011: delegated context carries grant attribution

## Context

Atlas O1 closes when every effectful call in a run's record is attributable to a declared grant.
Substrate cannot answer that: its subject and actor come from kernel peer credentials, and the
`principal` column of the operation ledger holds the calling process id, not a platform principal.
Design 06 section 2 decided the shape — the initiating platform principal retained separately from the
immediate service actor through tamper-safe delegated context, caller-written identity strings not
trusted — but nothing implemented it. Identity issues opaque access tokens resolved by a callback and
holds no signing key; connectors owns the grant, carries a stable grant reference and a grant-set
revision on every decision, and already issues Ed25519 compact JWS. Neither names substrate as an
audience in served code.

## Decision

Accept an optional signed **delegated-context document**, presented as a `delegated_context` member
beside `op` and `input`, required for effectful operations in the hosted and satellite-adjacent
postures. It is a compact JWS, `alg` EdDSA over Ed25519, `typ` `substrate-delegated-context+jwt`, with
a `kid` naming a configured trusted key, a lifetime of at most 300 seconds, and a closed claim set
carrying the audience `urn:b10x:substrate`, the platform principal in `sub`, the immediate actor in
`act.sub`, the substrate subject and deployment it is bound to, the tenant, the grant reference and the
grant revision. Substrate verifies signature, issuer, exact audience, time window, subject binding and
claim closure, and stores the grant reference and platform principal on the ledger row and on every
`operation.*` event.

Substrate does not evaluate the grant, resolve it, or call any issuer during a request. A verified
document annotates or refuses; it never admits an operation that substrate's own scope, capability,
sandbox, limit or lease checks declined, because a higher-layer permit cannot override a local check.
Absence where required, a malformed document, an unknown key, an invalid signature, a wrong audience, a
subject-binding mismatch and expiry are each a distinct named refusal; replaying one operation id under
a different grant reference is a conflict. None degrades to an unattributed run.

The change ships as a successor contract bundle, leaving earlier bundle bytes untouched, with a
conformance vector pair held byte-identically by substrate and connectors and carrying only public key
material. Identity and connectors are both named relying parties: which signs is a configuration of the
trusted key and changes no substrate code, and the audience string is adopted from identity's published
vocabulary, not minted here.

## Consequences

- A reader of one ledger row can answer O1's question — which grant, on behalf of which platform
  principal, through which immediate actor — without leaving substrate.
- The ledger gains two nullable columns. The existing `principal` keeps its process-id meaning; the
  platform principal is separate, because collapsing them is the confusion this decision prevents.
- Substrate takes on signature verification and one configured trust anchor — no grant evaluation, no
  introspection call, no runtime dependency on identity or connectors being reachable.
- Identity must mint an audience it documents and refuses, or connectors must sign instead; until one
  ships, the field is optional everywhere and the hosted requirement cannot be turned on.
