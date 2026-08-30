---
format: aep.planning-md/1
id: story:ledger-rows-carry-the-declared-grant
kind: story
status: implemented
title: An operation's ledger row carries the declared grant it ran under
summary: 'Atlas O1 exit evidence: every effectful call attributable to a declared grant, verified across the connectors/identity seam from substrate''s own record; design 06 section 2 fixes the shape.'
owner: substrate
tags:
- ledger
- o1
- trust
revision: 7
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

## Design draft — 2026-08-30

`docs/design/09-delegated-context-and-grant-attribution.md` (proposed) carries the shape and the
ADR text ready to accept. Findings that move this story's default: identity issues **opaque**
tokens and holds no signing key, `kid` or JWKS (`identity/src/lib.rs:1936-1939`, `:2172`), and
`urn:b10x:substrate` is documented but not issuable (`identity/README.md:144`,
`identity/src/lib.rs:1920-1922`); connectors **already** issues Ed25519 compact JWS carrying
`dl_grant`, `sub`, `act` (`connectors/crates/service/src/authority.rs:52-71`) and names the field
`grant_ref` (`connectors/crates/integration-catalog/src/lib.rs:99-106`). The draft keeps the
story's default (identity-signed) with that cost stated; the cheaper path is a connectors-signed
context. Decides: the ADR, with both owners.

## Progress — 2026-08-30, steps 2-4 done, story complete

Merged as `16b2b3b` (PR #20). Every command below was re-run by the orchestrator in the agent's
worktree, not taken from a report.

**Step 2 — the successor bundle.** `contracts/substrate-wire/0.7.0/`, 224 files, additive successor
to `0.6.0`, carrying the optional `delegated_context` request member, the `grant_ref` and
`platform_principal` ledger/event fields and the named refusals.

- `cargo xtask check-bundle 0.7.0` → `contract bundle 0.7.0 verified: 224 files, fixed point of
  xtask/bundle-source/0.7.0`; `0.5.0` (206) and `0.6.0` (213) still fixed points.
- `git status --short contracts/` showed only the new directory (invariant 6).
- `cargo xtask check-json` → `1316 documents in 7 bundles`. This matters: `check-json` landed the
  same night (`5f332b8`) and classifies every document under every bundle, where the retired Python
  only ever ran on `0.1.0`–`0.4.0`. `0.7.0` is classified, not skipped.

**Step 3 — the four named tests**, on the wire against the shipped binary over its socket.
`PORTABLE_CASES` 31 → 33, `DELEGATED_CASES` 48 → 50
(`crates/substrate-daemon/tests/runtime_vectors.rs:2768-2769`).
`bash scripts/delegated-lane.sh` → `runtime clean-room: 50 HTTP cases, startup refusal, and
dual-daemon refusal passed (delegated lane)`, plus the four named tests, 5 passed, exit 0.

**Failing-first is partly missing, and this records that rather than papering over it.** The
implementing agent hit a session limit before reporting, so its own failing-first output is lost.
One perturbation was reproduced to prove the tests are not vacuous: replacing the subject-binding
check at `crates/substrate-daemon/src/delegation.rs:174` with `if false` makes
`delegated_context_bound_to_another_subject_is_refused` fail `left: 201, right: 422` — a context
bound to another subject is accepted where it must be refused. Reverted; the file is byte-identical
to its backup. **The other three tests were not proven to fail first.** If that matters later, the
perturbations are cheap to repeat.

**Step 4 — key material.** The test signer derives its seed from a sentence
(`crates/substrate-daemon/tests/runtime_vectors.rs:2405-2409`) rather than carrying a key blob, so
the public repository has nothing to leak or rotate, and the cases mint documents bound to *this*
machine's subject and *this* instant — which a committed fixture never can.
`bash scripts/check-secrets.sh` → `no leaks found`.

**What it does not do**, each a deliberate boundary: it never evaluates the grant (connectors
decides); it never signs — `--delegated-context-key <kid>=<issuer>=<base64url>` declares a
*verifying* key, so which service signs is configuration and changes no substrate code; and the
grant-set revision, tenant, actor chain and `jti` are verified as part of the closed claim set and
then dropped, keeping the query surface to the two named columns.

Eight named refusals: `delegated-context.absent`, `.malformed`, `.unknown-key`,
`.signature-invalid`, `.audience-mismatch`, `.subject-mismatch`, `.expired`, `.grant-conflict`
(`crates/substrate-daemon/src/delegation.rs:209-256`).

`delegated_context` is a sibling of `op` and `input`, never a member of `input`
(`crates/substrate-wire/src/lib.rs:100-113`), so it sits outside the canonical request hash:
replaying the same `op` with a fresh context is the same operation and returns the original
outcome, and a request without one serializes exactly as a `0.6.0` client's did — which is what
keeps every frozen bundle's vectors true.

**Still open on the seam, not on substrate:** step 4's conformance vector *pair* needs the
connectors-side copy. Substrate's side is byte-reproducible; the atlas O1 row cannot cite both
sides until connectors holds its copy.
