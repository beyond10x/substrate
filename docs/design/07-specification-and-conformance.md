# Design 07: specification bundle and minimum wire

**Status:** accepted v1 design · **Date:** 2026-08-13

This closes the machine-readable authority, minimum endpoint set, envelope, projection, and
conformance choices required before implementation. Actual schemas and fixtures are the first
implementation deliverable; this document fixes what they must contain.

## 1. Contract bundle

Substrate owns `substrate-wire` under
[architecture ADR 0019 — Foundation contracts ship as signed reproducible bundles](https://github.com/daemonloom/daemonloom/blob/main/architecture/adr/0019-contract-release-and-conformance.md).
The first development bundle is `0.1.0` and contains:

```text
bundle.json
origins.json
compatibility.json
packaging.json
schemas/request.json
schemas/response.json
schemas/error.json
schemas/resource.json
schemas/capability.json
schemas/operation.json
schemas/event.json
schemas/vector.json
vectors/http/*.json
vectors/driver/*.json
```

Every schema and vocabulary in the bundle is explicitly `origin: daemonloom`; HTTP, JSON Schema,
JSON canonicalization, OAuth/JWT, WebSocket, and later OCI inputs retain official specification URI
and version in `origins.json`. The bundle follows deterministic OCI packaging, signing, digest
pinning, and clean-room conformance from ADR 0019. Markdown is explanation, not wire authority.
`bundle.json` lists the media type, byte length, and digest of every bundle path except itself; the
outer OCI manifest digest pins `bundle.json`. This avoids a recursive self-hash while leaving every
distributed byte covered.

## 2. Minimum host endpoint set

Only these operations are served in phase 2:

| Method and path | Purpose |
|---|---|
| `GET /v1/machine` | selected host driver, config/capability snapshot, limits |
| `POST /v1/workspaces` | create one empty confined workspace |
| `GET /v1/workspaces/{id}` | observe workspace |
| `GET /v1/workspaces/{id}/files/{path}` | ranged file read or paginated directory listing |
| `PUT /v1/workspaces/{id}/files/{path}` | bounded atomic file replacement |
| `DELETE /v1/workspaces/{id}/files/{path}` | bounded file deletion |
| `DELETE /v1/workspaces/{id}` | destroy workspace after owned execs are terminal |
| `POST /v1/execs` | start one bounded argv-only exec in a workspace |
| `GET /v1/execs/{id}` | observe process state and applied confinement |
| `GET /v1/execs/{id}/output` | ranged bounded stdout/stderr capture |
| `POST /v1/execs/{id}/signal` | cancel/signal and reconcile the process cgroup |
| `GET /v1/ops/{op}` | reconcile one subject-scoped mutation |

Workspace Git/bundle sources, snapshots, leases, live streams, events, sessions, workloads, images,
volumes, endpoints, Docker, Kubernetes, and hosted identity are absent capabilities, not stubbed
successes. A call to a known but absent family returns `unserved` only if that path is present in the
released version; otherwise ordinary not-found applies.

### Phase-3 additive endpoint set

`substrate-wire` 0.2.0 preserves all twelve routes above and adds exactly seven:

| Method and path | Purpose |
|---|---|
| `GET /v1/events` | bounded pull from the retained generation/sequence journal |
| `GET /v1/events/stream` | bounded WebSocket delivery of the same event values and cursors |
| `POST /v1/reconciliation-snapshots` | materialize one subject-scoped state view at an event barrier |
| `GET /v1/reconciliation-snapshots/{snapshot_id}` | page the immutable materialization |
| `POST /v1/workspaces/{workspace_id}/lease/renew` | keyed renewal of an existing explicit workspace lease |
| `POST /v1/execs/{exec_id}/lease/renew` | keyed renewal of an existing explicit exec lease |
| `DELETE /v1/execs/{exec_id}` | keyed retirement of terminal durable exec state and bounded output |

Workspace creation and exec start gain an optional explicit `lease_ttl_ms`; omission preserves the
0.1.0 request bytes and means no lease. Phase 3 does not add Git, sessions/stdin/PTY, workloads,
images, volumes, endpoints, Docker, Kubernetes, or hosted identity.

## 3. Canonical envelopes

Every request receives or is assigned a `request_id`. Every keyed resource-mutation JSON body
contains `op` and its operation-specific `input`. Reconciliation snapshot creation is the one
bounded non-keyed control mutation in 0.2: its body is exactly `{}`, it creates no operation-ledger
row, and its committed control event supplies the barrier. Success is:

```text
{ api_version, request_id, operation?, result }
```

Answered failure is:

```text
{ api_version, request_id,
  error: { class, code, message, address?, retriable, operation? } }
```

The closed classes and HTTP mappings remain those in Design 01. The route-specific schemas must
make secret values, authority tokens, raw environment, shell strings, and untrusted provider
response bodies structurally impossible. The common envelope alone does not provide that proof.

The operation record is:

```text
{ operation, request_hash, state, accepted_at?, terminal_at?,
  capability_snapshot, actor, principal?, resource?, outcome? }
```

`state` is `refused | accepted | terminal | unknown`. `outcome` contains either an observed result
or the typed answered error. Another subject sees not-found, not the record.

The immutable 0.1 hash is SHA-256 over its pinned length-delimited tuple of API major, HTTP method,
normalized route/resource address, and RFC 8785-canonical typed operation input. The 0.2 hash has a
new, explicitly versioned tuple: hash version, method, normalized address, canonical raw JSON
`input`, and canonical query. Strictly decoded query pairs are sorted with duplicates preserved;
malformed percent/UTF-8 queries retain exact raw bytes in a separate domain. Transport headers,
bearer material, and `request_id` are excluded. Subject/deployment scope is part of the ledger key.
Reusing the same scoped operation id with a different version-appropriate hash is `conflict`.

## 4. Capability and driver predicates

The machine document and request predicates use the closed shape fixed in Design 02. The minimum
host bundle declares facts for guarded file I/O, atomic replacement, argv exec, namespace sandbox,
no egress, cgroup limits/kill, output caps, and supported signals. A fact appears only after the
running backend probes it. Unknown required facts make a request `unserved`; malformed or unknown
predicate syntax is `refused`.

## 5. Connectors-owned projection manifest

The projection is a deterministic translation, never schema identity:

| Substrate fact | Connectors target rule |
|---|---|
| operation identity/path/input/output | copied through explicit field templates from the pinned wire bundle |
| `risk=read|write|destructive` | connectors-owned per-operation value with floors `low|high|destructive`; it may escalate, never lower |
| `idempotent|keyed|none` | `idempotent|conditional|non_idempotent`; keyed emits the same-operation-id condition |
| local `effects` | retained as substrate capability requirements; never relabeled semantic effects |
| semantic effects | mandatory connectors-owned explicit list, even when empty |
| `projected|callable|callable-direct` | connector `expose=true|false|false`; direct channels use the session binding, not unary invoke |
| auth/credential/request facts | explicit connectors-owned manifest entries; never synthesized from substrate absence |
| substrate event direction | channel declaration, never an outbound operation direction |

The manifest is published by connectors beside its connector schema, not inside the substrate-owned
bundle. It must name every source operation exactly once, reject an unknown source enum, reject an
unmapped target field, and record both source bundle and connector schema digests. The generated
provider document is compared byte-for-byte with the catalog artifact. This proof belongs to
substrate phase 6/connectors S-023 and S-031 and is not a source dependency of the host daemon.

## 6. Conformance inventory

The completed wire suite covers canonical hashing/replay conflict, strict request fields, additive response
handling, error classes, subject-scoped not-found, crash-before/after dispatch, observed response,
and capability invalidation. The driver suite covers every threat vector in Design 04 plus output
draining, non-zero exit as observation, whole-cgroup cancellation, and post-action re-observation.

Development vector cases may begin as prose-backed design inputs, but they are not conformance
evidence until they contain exact fixture setup, wire bytes, expected instances or hashes, and
machine-checkable postconditions. The bundle checker proves inventory integrity, not runtime
conformance.

Every JSON authority is schema-classified. Payloads use their declared bundle-relative `$schema`;
the five bootstrap authorities (`bundle`, `compatibility` including errata, `origins`, `packaging`,
and `hashing`) use fixed closed schemas in the offline gate. Adding an unclassified JSON
file fails CI. Every file below `schemas/` must declare Draft 2020-12 and pass the offline
pinned `jsonschema` crate's standards-conforming meta-schema validator before it can classify
another document. The same validator compiles each classified schema and validates its JSON
instance. Historical rootless-URN `$id` values cannot resolve relative references under standard
URI rules, so the gate deterministically inlines the exact bundle-relative `$ref` targets before
standards instance validation; it never changes the authoritative bytes or their assertions.
Negative tests inject an unclassified file, an invalid fixed authority, an invalid declared
payload, and an invalid schema to prove each production failure boundary. The immutable 0.1 bytes
are not edited; the same gate supplies their external bootstrap classification.

The `0.1.0` development bundle now satisfies that contract-execution-readiness gate. Its operation
registry selects closed address, input, and result schemas for all twelve phase-2 routes;
`hashing.json` fixes the length encoding and exact digest fixtures; `coverage.json` makes the route,
error, recovery, and Design 04 threat inventories exact; and `runner.json` defines the independent
producer/runner boundary. This opens runtime scaffolding but does not count the unexecuted vectors
as host-driver conformance evidence.

Phase 2 exits only when a black-box client built from the bundle and the host driver pass these
vectors without repository source access or a sibling checkout.

The additive `0.2.0` bundle is being regenerated from its authoritative renderer and is not yet a
publishable release. Its registry has 19 closed operations: the twelve 0.1 operations and exactly
seven additive operations. Its schemas additionally close source-scoped streams, snapshot-first
bootstrap, finite ledger capacity, durable refusals, transport bounds, maintenance scheduling, and
lifecycle crash windows. The manifest distinguishes nineteen 0.2 vectors executed by the daemon
runtime tests from design vectors that receive schema and semantic checking but are not claimed as
runtime executions. Portable and delegated runtime inventories must be reported from
fresh gate output; this design does not pin unmeasured case counts. No prose count or passing static
schema gate substitutes for the regenerated manifest and executable runtime evidence.

The 0.1.0 directory is immutable. Runtime compatibility is nevertheless executable rather than
assumed: `contract_vectors.rs` selects disputed vectors through each bundle manifest and compares
exact JSON results. Path-depth, cross-subject operation hiding, and TERM observation remain exact
0.1 behavior. Version 0.2 explicitly corrects four predecessor expectations: restart changes an unanswered
accepted operation to `unknown`, and a terminally persisted machinery failure sets
`retriable=false`; its input-limit vector also uses the 2 MiB envelope needed to represent the
advertised exactly 1 MiB decoded file, and a durably terminal write-limit refusal no longer
promises retry under the same operation id. These are safety clarifications, not silent
reinterpretations of 0.1 bytes.
