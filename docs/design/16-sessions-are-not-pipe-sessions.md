# Design 16: a session is not a pipe session

**Status:** proposed · **Date:** 2026-08-30

This document fixes the name of one route family. It decides whether the seven operation ids move,
how a second path is declared so it cannot drift from the first, what a client sees at each, what
the request hash normalizes against, and what would have to become true before the old path can be
withdrawn. It claims **no ADR number**: `adr/` admits `accepted` and `superseded` only
(`xtask/src/adrs.rs:12`), so the number is assigned by the operator at acceptance, exactly as
[design 12](12-aperture-byte-ceiling.md) waited for one. No planning story names it yet.

## Context

Every route the daemon serves is a plain noun — `/v1/machine`, `/v1/workspaces`, `/v1/execs`,
`/v1/events`, `/v1/reconciliation-snapshots`, `/v1/ops`
(`crates/substrate-daemon/src/app/routes.rs:35-102`). One is not:
`/v1/pipe-sessions` (`:67-86`) names **how the resource is wired**, and it is the only path in the
API that names a mechanism rather than a thing.

The accepted decision already says this is wrong.
[ADR 0007](../../adr/0007-protocol-processes-use-raw-pipe-sessions.md) reads *"Phase 4 supports two
explicit session **modes**: `pty` for human terminal interaction and `pipes` for machine protocols.
**Both create a leased session resource** before attachment"* (`:22`), and
[ADR 0008](../../adr/0008-pipe-sessions-have-distinct-durable-identity.md) makes the resource the
unit of authority — *"The operation resource is the session"* (`:21-23`). One resource, two modes.
The code agrees with the ADRs everywhere except the path:

| axis | value | where |
|---|---|---|
| resource tag | `SessionKind::Session` → `"kind": "session"` | `crates/substrate-wire/src/lib.rs:1220-1223` |
| its siblings | `WorkspaceKind::Workspace`, `ExecKind::Exec` — the same one-variant shape | `:276-279`, `:1196-1199` |
| channel axis | `SessionMode::Pipes`, a separate enum | `:1227-1229` |
| durable id | `ses_…` | `crates/substrate-daemon/src/app/service.rs:127-129` |
| operation ids | `session.capabilities`, `session.start`, `session.get`, `session.attach`, `session.signal`, `session.retire`, `session.lease.renew` | `contracts/substrate-wire/0.8.0/operations.json:379-546` |

All seven ids are mode-neutral already. There is no `/v1/sessions` and no `/v1/pipes`. "Pipe
session" is not a type; it is a session in pipes mode, the way an exec with an aperture is not an
"aperture-exec". [Design 13](13-pty-sessions.md) would serve a terminal from this family
(`:43-51`) — a PTY at a URL that says pipe — which is what makes the name urgent rather than
cosmetic.

## Decision

**Add `/v1/sessions`. Keep `/v1/pipe-sessions` answering. Rename nothing.** The constraint is not
taste. `xtask/src/bundle.rs:263-268` refuses any operation the predecessor served and the successor
does not — *"an additive successor never drops one"* — and that loop has **no branch on
`compatibility.kind`**. So the schema const `"kind": {"const": "additive-v1"}`
(`contracts/substrate-wire/0.8.0/schemas/bundle.json:21-22`) is not really what forbids a breaking
change; it is authored per version (`xtask/bundle-source/0.8.0/documents/schemas/bundle.json:21-22`)
and a successor could write a different string there. What forbids it is the checker,
unconditionally. A rename that withdrew seven routes is not expressible in this format at all.

**The seven operation ids do not change.** They are already the mode-neutral names the resource
deserves; only their `path` field moves to `/v1/sessions*`. Renaming them would be churn in the one
place that was already right, and `operation_kind` — the closed 13-value enum on every durable
ledger row (`contracts/substrate-wire/0.8.0/schemas/operation.json:40`) — is derived from those ids
(`xtask/src/render.rs:450-456`), so a rename would rewrite the ledger's vocabulary to fix a URL.

**The seven *new* ids are the aliases, and that is the only way the gate can see this change at
all.** `check_compatibility` counts operation **ids**, not paths: `route_ids` collects `id` into a
`BTreeSet` (`xtask/src/bundle.rs:525-538`) and the counts are that set's intersection and difference
with the predecessor's (`:241-242`). A bare path move therefore passes `cargo xtask check-bundle`
with `preserves_routes: 26`, `adds_routes: 0` and no drop — **the compatibility checker cannot tell
a rename from a no-op.** Giving the legacy entries their own ids is what makes the change countable.
So `operations.json` grows from 26 entries to 33: the seven canonical ones at `/v1/sessions*`, plus
`pipe-session.capabilities`, `pipe-session.start`, `pipe-session.get`, `pipe-session.attach`,
`pipe-session.signal`, `pipe-session.retire`, `pipe-session.lease.renew` at the old paths — each id
matching the registry's own pattern (`schemas/operation-registry.json:119-122`). Predecessor
`0.8.0`, `preserves_routes: 26`, `adds_routes: 7`, `kind: "additive-v1"`, nothing dropped. The
registry's `maxItems` follows the count automatically
(`xtask/bundle-source/0.8.0/documents/schemas/operation-registry.json:191-193`).

**An alias is declared, not inferred.** Each legacy entry carries one new field, `alias_of`, naming
the canonical id — `{"id": "pipe-session.start", "path": "/v1/pipe-sessions", "alias_of":
"session.start", …}`. The registry item is `additionalProperties: false` (`:14`), so the successor's
own schema admits the field and every earlier bundle stays closed against it. `check-bundle` gains a
`0.9.0` arm beside the four that exist (`xtask/src/bundle.rs:383-395`) asserting the anti-drift rule:
**an entry with `alias_of` is byte-identical to its target in every field except `id`, `path` and
`alias_of`** — same method, scope, risk, idempotency, effects, exposure, and the same address, input
and result schemas. Two independently authored families could drift in a capability predicate and
nothing would notice; a declared alias cannot, because a difference is a gate failure.

**An alias contributes a route and nothing else.** The renderer's derived markers filter on
`alias_of`: `keyed_route_ids` (`xtask/src/render.rs:444-456`) skips them, so `operation_kind` keeps
its 13 values and no ledger row ever records which path a client used; `response_branches`
(`:509-533`) and the keyed result and request branches skip them, so the response envelope is
unchanged. Only `registry()` (`:431-442`) emits them, which is exactly the surface `route_ids`
counts. Coverage requirements are authored rather than derived (`:550-560`), so the seven aliases
add no `route.*` rows beside the 26 that exist
(`contracts/substrate-wire/0.8.0/coverage.json:552-617`); they add one, `route.session.alias`, whose
evidence is a vector calling both paths with one `op` and observing one session.

**The request hash normalizes against the canonical template, and this is the part that bites.**
The address is a hashed field: `canonical_request_hash_v2(method, address, raw_input, raw_query)`
(`crates/substrate-daemon/src/app/operations.rs:105`, tuple at
`crates/substrate-wire/src/lib.rs:2110-2116`), and `hashing.json` says the address is the registry
template with its parameters substituted (`contracts/substrate-wire/0.8.0/hashing.json:6`, `:65`).
Left alone, the same `op` ULID sent to `/v1/pipe-sessions` and then to `/v1/sessions` hashes twice
and the second call is `operation.request-conflict`
(`crates/substrate-daemon/src/app/operations.rs:684-690`) — a client that times out on the old path
and retries on the new one is told its own request is somebody else's. So `hashing.json` gains one
key stating that an aliased address normalizes against the **aliased** operation's template, and the
daemon passes the canonical literal on both registrations: the address strings the handlers already
hold (`crates/substrate-daemon/src/app/sessions.rs:204`, `:456`, `:514`, `:633`) become
`/v1/sessions…` and do not vary with the path that was called. Two `Router::route` registrations,
one handler (`crates/substrate-daemon/src/app/routes.rs:67-86`): drift inside the daemon is
impossible by construction, because there is no second implementation to drift.

**Identical responses at both paths, and no marker on the old one.** *What a `Deprecation` header
would have cost.* A conformance vector's `expected.response` is `additionalProperties: false` over
`status` and `body` (`contracts/substrate-wire/0.8.0/schemas/vector.json:1674-1704`), so **no vector
can assert a response header** — the signal would be unprovable in the gate, and a deprecation
nothing verifies is a comment, not a contract. *What a body field would have cost.* The two paths
share one result schema, which is the whole point of `alias_of`; marking only the old one means
splitting that schema, which reintroduces the drift the alias exists to prevent, and
`response_policy` is `closed-route-selected-envelopes`
(`contracts/substrate-wire/0.8.0/compatibility.json:10`). Supersession is therefore stated once, in
the registry, where a consumer already reads which operation lives at which path — machine-readable,
inside the bundle, and covered by the fixed-point check.

**Removal is a later decision and needs a bundle format that does not exist.** Four things would
have to change, and none of them is this design's to make. `xtask/src/bundle.rs:263-268` would need
a declared-removal branch instead of an unconditional refusal. `bundle.json.compatibility` would
need a third count, `removes_routes`, enumerating exactly which ids went and what replaced each —
invariant 6 currently fixes the block at predecessor plus `adds_routes`/`preserves_routes` and says
the checker pins them (`AGENTS.md:43-53`), so a third count is an amendment to an invariant, not a
field. `compatibility.json` would need a removal record, the shape `errata_from` already sets for
`0.2.0` (`xtask/src/json.rs:1367-1394`). And `kind` would need a second admissible value. Withdrawing
`/v1/pipe-sessions` is that change; it is not proposed here and it cannot be cut until the consumer
has moved.

**No atlas ADR.** The wire-visible-identifier fence (`AGENTS.md:92-98`) enumerates five things: the
`urn:b10x:substrate-wire:*` schema `$id`s, the `x-b10x-contract*` headers, the
`https://b10x.invalid/` URI namespace, the `b10x.execution-capsule.v1` domain separator and the
`origin: b10x` marker. Every one carries the former brand name, which is why they are frozen — atlas
ADR 0001 renamed the brand and these bytes could not follow. A route path carries no brand and is on
no such list. The broader org rule that *renaming anything another repo verifies is a coordinated
migration with an ADR* (`AGENTS.md:3-6`) does reach a path — and it is precisely the reason this
design renames nothing. `/v1/pipe-sessions` keeps answering byte for byte, so no consumer's pinned
path moves and there is nothing to coordinate. That ADR falls due at removal.
[Design 13](13-pty-sessions.md) reads the fence the other way (`:57-62`); its `AGENTS.md:92-98`
citation does not support a path, its `:3-6` citation does, and both were arguments against a
**rename**, which this is not.

**`harness` does nothing, until it does one thing.** It is the consumer substrate confines
(`README.md:21`), and I have not read its source: I do not know which paths it pins or whether it
reads `operations.json` at all. The additive shape is what makes that safe not to know — no working
client breaks on the day `0.9.0` lands, because every path it can be using still answers. What
harness owes is a single migration, moving its seven paths to `/v1/sessions*` at its own pace, and
the removal bundle cannot be cut until it reports that done. The daemon still advertises
`substrate-wire/0.4.0` in `x-b10x-contract` (`AGENTS.md:49-51`), so the header is not how harness
would learn the new path either; the bundle is.

**Successor bundle `0.9.0`, provisionally**: predecessor `0.8.0`, `preserves_routes: 26`,
`adds_routes: 7`, authored under `xtask/bundle-source/0.9.0/`, with `cargo xtask check-bundle 0.9.0`
in `scripts/gate.sh` beside the four that are there (`:27-30`) — a bundle whose check is not in the
gate is unverified from the next commit onward. [Design 13](13-pty-sessions.md) (`:166-167`),
[design 14](14-network-session-authority.md) (`:175-181`) and
[design 15](15-docker-driver-entry-gate.md) (`:236-247`) all name `0.9.0` provisionally too; the
number belongs to whichever is accepted first and the rest move to its successor. If this one lands
with design 13, the counts add rather than replace, and the two say so in one bundle. Earlier
directories keep their bytes (invariant 6): `0.4.0` through `0.8.0` each serve all seven session ids
at `/v1/pipe-sessions` and always will, so the alias is not a transition period for the bundles — it
is a transition period for the daemon.

## Consequences

The API stops having one route named after a mechanism, and a terminal served under design 13
arrives at a path that describes what it is. Nothing that works today stops working: 33 operations
where there were 26, seven of them the old paths kept alive under declared aliases that a gate check
holds identical to their targets.

The costs are stated rather than hidden. The registry carries two entries for every session
operation until removal, and any future session route has to be added twice for as long as that
lasts. The seven alias ids are themselves routes the additive checker now protects, so withdrawing
them later is the same non-additive change as withdrawing the paths — the alias buys a migration
window, not an exemption. And a client reading `operations.json` sees two paths for one operation
with no ordering between them beyond `alias_of`; a consumer that ignores that field learns nothing
about which one is canonical.

Everything here is provable in CI, unusually for a design in this repository: the counts, the
alias-identity rule, the shared response schema and the cross-path replay all fall out of
`cargo xtask check-bundle 0.9.0` and one HTTP vector. There is no host-confinement half and nothing
to report **absent rather than passed**.
