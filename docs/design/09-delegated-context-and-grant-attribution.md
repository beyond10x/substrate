# Design 09: delegated context and grant attribution

**Status:** accepted as [ADR 0011](../../adr/0011-delegated-context-and-grant-attribution.md) · **Date:** 2026-08-29

Substrate can say which local subject ran a process, not under whose authority. This document fixes
the one artifact that closes that gap without making substrate an authorization engine.

## 1. Problem

Atlas objective O1 closes when "every effectful call in a run's record is attributable to a declared
grant; a call outside it is a named refusal, not a missing row". Substrate holds the execution record,
so a reader of `GET /v1/ops/{op}` must answer *which grant authorised this, on behalf of which platform
principal* from substrate's own durable row.

Today it cannot. The daemon derives `subject` and `actor` from kernel peer credentials, and the one
column that sounds like an answer — `principal` in the `operations` table — holds `pid:<pid>`, the
calling *process*; the hosted TCP path leaves it `None` (`crates/substrate-daemon/src/runtime.rs:404-408`,
`:492`; `crates/substrate-store/src/schema.rs:55`). Nothing names a grant.

Design 06 § 2 already fixed the shape of the answer: the initiating platform principal is retained
separately from the immediate service actor "through tamper-safe delegated context; caller-written
identity strings are not trusted". This document says what that document is, what substrate checks and
what it refuses, and widens substrate's authority by nothing — design 06 § 2's "a higher-layer permit
cannot override" substrate's local checks stands unchanged.

## 2. What exists

**Identity issues opaque tokens and signs nothing.** An access token is a random string
`dl_access_v1_…`; the claims live server-side, a relying party resolves them at
`GET /v1/access-authority`, and storage is a SHA-256 verifier, never the value
(`identity/src/lib.rs:1936-1939`, `:1781`, `:2008-2025`; `identity/AGENTS.md` invariant 2). The closed
set is `iss, sub, aud, iat, nbf, exp, jti, act{sub}, scope, dl_principal_kind, dl_tenant, email,
groups`, lifetime five minutes, with `act` carrying the same subject as `sub` — no delegation chain yet
(`identity/src/lib.rs:1656-1671`, `:70`, `:1957-1959`). There is **no issuer key, no `kid`, no JWKS
route and no JWS issuance path** (`identity/Cargo.toml:26`, `identity/src/lib.rs:2172`).

**The `substrate` audience is documented but not issuable.** `identity/README.md:144` lists
`urn:b10x:substrate`; issuance refuses every audience but `urn:b10x:connectors`, and the string occurs
in identity's source only as an audience-mismatch test's negative case
(`identity/src/lib.rs:1920-1922`, `:3291`). The vocabulary is frozen until an M2 audience registry
lands, so minting it is a coordinated change there (`identity/AGENTS.md` § *Safety envelope*).

**Connectors owns both halves of a grant reference.** A Grant is tenant-scoped, per-connector and bound
to one Connection, never to a credential; `Grant.grant` is "a stable reference bound into every decision
this grant admits" and `GrantSet.revision` "travels into every decision, so an audit row can say which
published policy admitted an operation" (`connectors/crates/domain/src/grant.rs:131-136`, `:178-183`;
values like `grant:observability-read` at `:255`). `grant_ref` is already connectors' own field name for
this audit purpose (`connectors/crates/integration-catalog/src/lib.rs:99-106`).

**Connectors can sign; its decision object cannot travel.** `GrantDecision` binds the whole decided
context behind private fields, with no public constructor and no `Serialize`
(`connectors/crates/domain/src/evaluator.rs:133-163`). What *does* leave the process is Session
Authority v1: a compact JWS, `alg` EdDSA over Ed25519, a `kid`, ≤ 60 s lifetime with 5 s skew, a DPoP
`cnf.jkt`, one-time redemption through a `ReplayStore` and a `RevocationView`, and claims already
carrying `dl_grant`, `sub`, `act`, `dl_org`, `dl_deployment` and `dl_connection`
(`connectors/crates/service/src/authority.rs:13-20`, `:52-71`, `:226-291`, `:352-360`). Its verifier
holds one configured `trusted_key` with no publication route (`:455`), so key distribution is out of
band. One roadmap premise is stale: atlas' arrows row records "0 audience strings … in
`connectors/crates`", but the constant is `connectors/crates/server/src/hosted.rs:55`, presented on
every call (`connectors/crates/identity-http/src/adapter.rs:143`).

## 3. The delegated-context document

One bounded, self-verifying, signed document: a compact JWS, `alg: EdDSA` over Ed25519, header
`typ: substrate-delegated-context+jwt`, a `kid` naming a configured trusted key, at most 4 KiB before
decode. Substrate never calls an issuer during a request: an availability dependency inside a
confinement decision turns an identity outage into an exec outage, which rules out identity's present
opaque-token-plus-callback model. The type string carries no brand token, so it needs no atlas
exemption; connectors' `dl-session+jwt` is frozen where it is. The claim set is closed.

| claim | meaning |
|---|---|
| `iss` | issuer origin, matched exactly against the configured issuer for that `kid` |
| `aud` | exactly `urn:b10x:substrate` — one string, no list, no prefix match |
| `sub` | the initiating platform principal, opaque to substrate |
| `act.sub` | the immediate service actor, in the RFC 8693 shape both siblings already emit |
| `iat`, `nbf`, `exp` | `iat ≤ nbf ≤ exp`, total lifetime ≤ 300 s, ± 30 s skew |
| `jti` | one-time id |
| `bound_subject`, `bound_deployment` | the exact substrate subject (e.g. `local:1000`) and deployment it may be presented under |
| `tenant` | the tenant whose grants decided |
| `grant_ref`, `grant_revision` | the admitting grant's stable reference, and the grant-set revision current when it decided |

**Substrate verifies**, before dispatch and before any driver authority exists: the JWS parses within
the byte bound; `alg`, `typ` and `kid` are as above; the signature verifies; `iss` matches that key's
configured issuer; `aud` is exactly `urn:b10x:substrate`; the time window holds; `bound_subject` and
`bound_deployment` equal this request's; the claim set is closed with every member in bound.

**Substrate does not** evaluate, fetch, resolve or read authority out of the grant — a document naming
a grant that admits everything still meets substrate's own scope, ownership, capability, sandbox, limit
and lease checks unchanged. Nor does it derive subject, tenant or uid from the document: the binding
runs the other way, a mismatch refuses rather than re-subjects, a verified document can only annotate or
refuse, and caller-written identity strings elsewhere in the body stay untrusted and unread.

## 4. Where it lands

**Request.** An optional `delegated_context` string, sibling to `op` and `input`, not inside `input`.
Every route arm today is `{op, input}` with `additionalProperties: false`
(`contracts/substrate-wire/0.4.0/schemas/request.json`), so each arm gains the member and keeps the
closed policy. Outside `input` it stays out of the canonical request hash (design 07 § 3), with a
consequence stated rather than discovered later: replaying the same `op` with a *fresh* context is the
same operation and returns the original outcome, not a `conflict`; first write wins on the recorded
grant, and a replay under a different `grant_ref` is a `conflict` (§ 5).

**Ledger and events.** Two nullable columns beside the existing `actor` and `principal`
(`crates/substrate-store/src/schema.rs:54-55`): `grant_ref` and `platform_principal`. The existing
`principal` keeps its `pid:` meaning — reusing it would collapse the local process and the platform
principal into one field, the exact confusion design 06 § 2 forbids. The same two members, nullable,
join `operation.accepted`, `operation.refused`, `operation.terminal`, `operation.unknown` and
`operation.failed` beside `actor` and `principal`, and the event set stays closed (design 01 § 3.6;
`crates/substrate-store/src/operations.rs:712`, `:763`, `:808`;
`crates/substrate-store/src/events.rs:220-222`).

**Capability facts and postures.** `trust.delegated-context: true` appears only after the daemon
resolves a configured trusted issuer key — configured intent is not a fact (design 02 § 3) — and
`trust.delegated-context-required: true|false` tells a client, before it calls, whether omission
refuses. Effectful means a mutation declaring any effect from the closed v1 set (design 01 § 2); reads
never require a context. Across `architecture/deployment-postures.md` a context is **required** in the
hosted and satellite-adjacent postures and **optional** in the personal and organization ones — a direct
client under architecture ADR 0013 has no grant, so absence there is normal, not a gap.

## 5. Refusals

Each is answered before dispatch, durable under the operation id (design 03, V1 decision 1), with a
stable dotted `code`. None degrades to "ran, unattributed" — a missing guarantee is a named refusal,
never silent (invariant 3).

| code | class | when |
|---|---|---|
| `delegated-context.absent` | `refused` | required by this deployment, not presented |
| `delegated-context.malformed` | `refused` | not a compact JWS, over the byte bound, unknown or out-of-bound claim, unsupported `alg` or `typ` |
| `delegated-context.unknown-key` | `refused` | `kid` names no configured trusted key |
| `delegated-context.signature-invalid` | `refused` | signature fails against that key |
| `delegated-context.audience-mismatch` | `refused` | `aud` is not exactly `urn:b10x:substrate` |
| `delegated-context.subject-mismatch` | `refused` | `bound_subject` or `bound_deployment` disagrees with the authenticated request |
| `delegated-context.expired` | `refused` | outside the `nbf`/`exp` window after skew |
| `delegated-context.grant-conflict` | `conflict` | the same `op` replayed under a different `grant_ref` |

`address` names the claim that failed and never carries a value (design 01 § 6.1). The document's bytes
never enter an error body, an event, a log, the request hash or a resource observation (design 06 § 3).

## 6. Conformance vectors

The pair both repositories hold, byte-identical, so the atlas O1 row cites two sides rather than one
repository's word: one accepting vector and one per refusal code in § 5. Substrate's copies live under
the successor bundle's `vectors/http/`; connectors keeps its copies beside the shared artifact it
carries today, `connectors/fixtures/substrate-wire-0.1.0-axis-projection.json`. Each carries the
**public** verifying key and the pre-signed compact JWS — verify-only, so no seed is committed on either
side — and states its evaluation instant, so `nbf`/`exp` outcomes are deterministic. Divergence in
either direction fails both sides, which makes the seam *verified* rather than asserted.

## 7. Compatibility

A successor bundle, `contracts/substrate-wire/0.5.0`. Every earlier directory keeps its exact bytes
(invariant 6): an optional request member and two nullable record fields are added by cutting a new
bundle, never by editing `0.4.0`. Its compatibility block states `kind: additive-v1`, `predecessor:
0.4.0`, `adds_routes: 0`, `preserves_routes: 26` — the operation registry holds 26 entries in both
`0.3.0` and `0.4.0`, and this change adds no route. The successor's checker joins the four already in
`scripts/gate.sh`; a bundle whose checker is not in the gate is unverified from the next commit on. One
trap: `0.4.0`'s block repeats `0.3.0`'s counts and predecessor
(`contracts/substrate-wire/0.4.0/bundle.json:5-10`), so the successor states its own lineage, and its
checker proves an `adds_routes: 0` shape none of the four has seen. A client that sends nothing is
unaffected — omission is the `0.4.0` request byte-for-byte.

## 8. Open decisions

Each states the default taken if nobody answers, so silence does not block the story.

1. **Who signs — identity or connectors.** Owner: identity and connectors jointly, in the ADR.
   Verification is issuer-agnostic by construction — a configured `kid` and trusted key, the same bytes
   either way. The evidence is asymmetric: connectors already issues Ed25519 compact JWS; identity has
   no signing key, no `kid` and no issuance path (§ 2). **DEFAULT: identity-signed, audience
   `urn:b10x:substrate`**, per the story — which obliges identity to add issuer key material and a
   publication route it lacks; connectors-signed is the reversible fallback.
2. **Minting the `urn:b10x:substrate` audience, and its brand status.** Owner: identity, with atlas on
   the brand question — this repository's `AGENTS.md` § *Safety envelope* calls `urn:b10x:*` former-brand
   carriers while `identity/AGENTS.md` says the prefix "carries no banned token now". **DEFAULT:
   substrate pins the string identity already published rather than minting one**, correct either way.
3. **Whether `grant_revision` becomes a third ledger column.** Owner: substrate. **DEFAULT: no** — the
   two named columns are the query surface, and connectors retains the revision on its own decision
   (`connectors/crates/domain/src/evaluator.rs:250-254`); adding it later is another bundle.
4. **Replay and revocation.** Owner: substrate. Connectors' session authority redeems a `jti` once and
   consults a revocation view (`connectors/crates/service/src/authority.rs:352-360`). **DEFAULT: neither
   here** — a delegated context is an attribution record, not a capability; one-time redemption breaks
   the `op`-replay contract in § 4, a revocation lookup puts a network call on the dispatch path, and
   the ≤ 300 s lifetime is the bound.

## 9. Proposed ADR text

Accept by copying the block below to `adr/0011-delegated-context-and-grant-attribution.md` and adding
the matching `adr/README.md` row — number `0011`, decision "Delegated context carries grant
attribution", status `accepted`, linking that filename. The gate requires the frontmatter fence, that
`status`, a `YYYY-MM-DD` `date`, a `# ADR 0011: …` first heading and an agreeing index row, which a
*proposed* ADR cannot satisfy — so the text lives here.

```markdown
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
```
