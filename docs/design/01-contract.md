# Design 01: the substrate API contract

**Status:** draft for review · **Date:** 2026-08-13

This document is the product: the wire contract a substrate daemon serves, family by family,
with the error taxonomy and the constraints that keep every operation declarable in the
daemonloom/connectors catalog. Drivers (`host`, `docker`, `k8s`) implement this contract;
consumers (Flux, autodev, the platform) see only it.

## 1. Shape of the wire

- **Transport.** HTTP/1.1+JSON under a `/v1` path prefix. Streams (logs, events, exec output,
  duplex sessions and tunnels) are WebSocket channels with **closed, declared frame sets**.
  Nothing else: no gRPC, no signing, no cookies, no multi-leg handshakes.
- **Identity of resources.** Server-minted prefixed ids: `ws_…` (workspace), `ex_…` (exec),
  `ses_…` (session), `wl_…` (workload), `vol_…` (volume), `ep_…` (endpoint); images go by
  digest. Ids are opaque; labels (`key=value`, caller-owned) are the query surface.
- **Operation ids.** Every mutation takes a **client-minted** `op` id (recommended: ULID).
  Replaying the same `op` with the same body is a no-op returning the original outcome;
  replaying it with a different body is `conflict`. This is the reconciliation handle for the
  unanswered failure modes (§6.2) and what makes platform-side retry safe.
- **Observed answers.** A mutation's `2xx` body is the resource **re-read from the driver**
  after the change, stamped `observed_at`. The contract has no response that merely echoes a
  request. Fields the driver cannot know are `null` and mean *unknown*, never zero.
- **Pagination.** Every list takes `?cursor=` and `?limit=`, answers `{items, next_cursor}`,
  filters by `?label=`. Cursors are opaque.
- **Versioning.** Additive evolution within `/v1`. Optional features are gated by capability
  facts (§8), never by version sniffing.
- **Authentication.** `Authorization: Bearer sbt_…`. Tokens are daemon configuration, each
  carrying a stable local subject, an operational label (the immediate event `actor`), and coarse
  scopes: `observe`, `workspaces`, `exec`, `workloads`, `images`, `admin`. Resources and operation
  ids are namespaced to that authenticated subject; another subject receives the same not-found
  answer as an unknown id. The daemon binds loopback by default and **refuses to start on a
  reachable address without authentication and TLS/mTLS or a configured trusted tunnel**. Scopes
  are blast-radius limiters; policy lives in the platform's grants. One daemon is one trust domain
  and, for v1, one tenant.

## 2. Operation metadata: the substrate vocabulary is native

Every operation in this spec declares, in the spec itself:

- `direction` — `outbound` for all request/response operations; events are `inbound`.
- `risk` — substrate-local working values: `read` | `write` | `destructive`.
- `idempotency` — `idempotent` | `keyed` (safe under the same `op` id) | `none`.
- `effects` — a **closed v1 set**: `process`, `filesystem:workspace`, `filesystem:volume`,
  `network:egress`, `network:expose`, `image`, `workload`.
- `expose` — `callable` | `callable-direct` (§7.8; duplex surfaces the platform brokers but
  never proxies) | `projected` (worth showing to a model).

These fields are not asserted to be byte-compatible with connectors. Substrate publishes a
canonical machine-readable wire/metadata bundle. Connectors owns a small projection manifest for
its distinct direction, risk, idempotency, semantic-effect, exposure, auth, credential, and request
facts. A deterministic translation of the pinned substrate bundle plus that manifest produces the
first-party provider document and is tested byte-for-byte against the compiled catalog artifact.
No grant example is normative until that translation and its conformance fixture exist.

## 3. The six families

### 3.1 Workspace — a confined tree with identity

The unit of "somewhere to work": a directory tree the daemon owns, materialized from git or empty.
Substrate owns the confinement implementation: lexical `..` escape and symlink escape are both
refused, and paths are walked component-by-component without depending on a product-owned crate.

| Operation | Method & path | risk | idempotency | effects |
|---|---|---|---|---|
| Create (empty or from source) | `POST /v1/workspaces` | write | keyed | filesystem:workspace, network:egress (git source) |
| List / get | `GET /v1/workspaces[/{id}]` | read | idempotent | — |
| Checkout a pinned ref | `POST /v1/workspaces/{id}/checkout` | write | keyed | filesystem:workspace, network:egress |
| Read file / list dir | `GET /v1/workspaces/{id}/files/{path}` | read | idempotent | — |
| Write file (atomic) | `PUT /v1/workspaces/{id}/files/{path}` | write | keyed | filesystem:workspace |
| Delete file | `DELETE /v1/workspaces/{id}/files/{path}` | destructive | keyed | filesystem:workspace |
| Snapshot out (bundle/tar, or push) | `POST /v1/workspaces/{id}/snapshot` | write | keyed | network:egress (push) |
| Destroy | `DELETE /v1/workspaces/{id}` | destructive | keyed | filesystem:workspace |

Semantics:

- **Sources are git-native.** `{git: {source: <named>, ref, depth}}`, an unauthenticated URL that
  passes a deployment destination aperture, `{bundle: <uploaded artifact>}`, or empty. Checkout
  resolves a mutable ref to an immutable commit and records that commit. A named source binds
  scheme, authority, port, path, credential, and destination policy in daemon configuration.
  Callers never pair an arbitrary URL with a stored credential. Bulk content leaves only via
  `snapshot` — a bundle/tar download or a push `{remote: <named>, refspec}` to a configured,
  credential-bound remote. Redirects, proxies, submodules, Git LFS, credential helpers, and hooks
  are disabled unless their bounded behavior is explicitly part of the named source/remote.
- **File IO is bounded.** Reads are ranged and byte-capped; writes are atomic replacements.
  This surface is for inspection and small edits, not bulk transfer (§10).
- Workspaces are disposable and may carry a lease (§5).

### 3.2 Exec — bounded runs, background spawns, interactive sessions

| Operation | Method & path | risk | idempotency | effects |
|---|---|---|---|---|
| Start an exec | `POST /v1/execs` | write | keyed | process, filesystem:workspace (+ network:egress unless sandbox denies) |
| Get observed state | `GET /v1/execs/{id}` | read | idempotent | — |
| Signal | `POST /v1/execs/{id}/signal` | destructive | keyed | process |
| Captured output | `GET /v1/execs/{id}/output` | read | idempotent | — |
| Live output stream | `WS /v1/execs/{id}/stream` | read | — | — |
| Open interactive session | `POST /v1/sessions` + `WS /v1/sessions/{id}/channel` | write | keyed | process, filesystem:workspace |

The exec spec: `{workspace, argv, env, image?, sandbox, timeout, stdin?, wait?}`.

- **Argv-only.** `argv[0]` is the program; there is no shell-string field anywhere in this
  contract. cwd is pinned to the workspace root.
- **Environment is default-deny.** The child's env is cleared to the non-secret allowlist,
  then shaped by `env: {allow: […], set: {…}}`. No credential ambient in the daemon's own
  environment can reach a child.
- **Sandbox profiles:** ordinary callers use `workspace` (fs: workspace rw + system ro;
  `network: none|aperture`) plus `require: true` — if the driver cannot enforce the requested
  profile the exec is **refused**, never run weaker. `aperture` names deployment-owned egress
  policy; request data cannot widen it. An operator-only unconfined profile is a distinct authority,
  disabled by default and unavailable in hosted or satellite postures. Which confinement and
  destination policy snapshot were actually applied is recorded on the exec.
- **`image` is driver-dependent.** On a container-backed driver an exec may name the image it
  runs in (default from daemon config); the host driver refuses the field as `unserved`.
- **Exit is an observation, not an error.** `exited{code: 1}` is a successful answer. Captured
  output is byte-capped per stream with an appended truncation notice; the reader drains past
  the cap so a full pipe cannot deadlock the child.
- **Sessions are the only duplex surface** (PTY; frames: `stdin`, `stdout`, `resize`, `exit`).
  `?wait=true` on short execs returns the terminal observation directly; everything else polls
  or streams.
- Background execs live and die with their workspace and have **no restart policy** — that is
  what workloads are for.

### 3.3 Workload — a long-lived app from an image

The line between the families: **an exec runs in a workspace; a workload runs from an image.**
"Run a flux agent in a container/pod" is a workload; "run this build step" is an exec.

| Operation | Method & path | risk | idempotency | effects |
|---|---|---|---|---|
| Deploy | `POST /v1/workloads` | write | keyed | workload, network:expose (if endpoints) |
| List / get observed status | `GET /v1/workloads[/{id}]` | read | idempotent | — |
| Replace (recreate/rolling per driver) | `PUT /v1/workloads/{id}` | write | keyed | workload |
| Stop / start / restart | `POST /v1/workloads/{id}/(stop\|start\|restart)` | destructive / write / destructive | keyed | workload |
| Delete | `DELETE /v1/workloads/{id}` | destructive | keyed | workload |
| Logs | `GET /v1/workloads/{id}/logs` + `WS …/logs/stream` | read | idempotent | — |

The workload spec: `{name, image, command?, args?, env, secret_slots?, mounts?, endpoints?,
restart: never|on-failure|always, resources?, lease_ttl?, labels}`.

- **Images are recorded by observed digest** even when given as a tag.
- **Secret slots are names, never values** (§7.7): they resolve from daemon-config named
  secrets, injected only into the workload that declared them.
- **Status is observed**: `{desired, observed: {state, probed_at}, restarts, last_exit}`.
  A driver that cannot currently answer reports `observed: null` — unknown, not "running".
- Mounts reference volumes or workspaces `{target, path, ro}`.

### 3.4 Image — build, pull, push

| Operation | Method & path | risk | idempotency | effects |
|---|---|---|---|---|
| Build from a workspace | `POST /v1/images/builds` (+ `WS …/builds/{id}/stream`) | write | keyed | image, process, network:egress |
| List / get | `GET /v1/images[/{digest}]` | read | idempotent | — |
| Pull | `POST /v1/images/pulls` | write | keyed | image, network:egress |
| Push | `POST /v1/images/{digest}/push` | write | keyed | network:egress |

- Build spec: `{workspace, context_path, dockerfile, tags, build_args, target?}`. Build args
  are non-secret by contract. A failed build is an **observed terminal state** on the build
  resource, not a wire error.
- **Registries are named daemon configuration**; pull/push reference them by name. Registry
  credentials never appear in request JSON.
- **A build is arbitrary code execution.** Each driver declares the fact
  `image.build.confined: true|false` (docker-daemon builds are root-equivalent and say so).
  Consumers that need confined builds match on the fact rather than trusting a hope.

### 3.5 Volume & Endpoint — storage and network primitives

| Operation | Method & path | risk | idempotency | effects |
|---|---|---|---|---|
| Volume create / list / delete | `POST\|GET\|DELETE /v1/volumes[/{id}]` | write / read / destructive | keyed | filesystem:volume |
| Endpoint expose | `POST /v1/endpoints` | write | keyed | network:expose |
| Endpoint delete | `DELETE /v1/endpoints/{id}` | destructive | keyed | network:expose |
| Tunnel (port-forward) | `WS /v1/endpoints/{id}/tunnel` | write | — | network:expose |

- Volumes are named, labeled storage; deletion is refused (`conflict`) while mounted.
- Endpoints target a workload or exec port with explicit exposure: `loopback` | `lan`.
  LAN exposure is **refused unless daemon configuration allows it** — fail closed. The answer
  carries the **observed** address, which may differ from the requested one.
- The tunnel is a duplex byte channel (`callable-direct`, §7.8).

### 3.6 Observe — machine facts, events, the operation ledger

| Operation | Method & path | risk | idempotency |
|---|---|---|---|
| Machine facts & capabilities | `GET /v1/machine` | read | idempotent |
| Event page / stream | `GET /v1/events?cursor=` + `WS /v1/events/stream` | read | idempotent |
| Operation ledger | `GET /v1/ops/{op}` | read | idempotent |
| Usage metrics | `GET /v1/metrics` | read | idempotent |

- **Events are the observability spine.** Every state transition of every resource emits one
  typed event `{seq, resource, transition, observed_at, actor, op}`. The event set is closed
  and declared (what connectors' inbound grants require); replay is cursor-based within a
  stated retention window, and the window itself is a machine fact.
- **`/v1/machine` reports a versioned snapshot of probed facts**: drivers present, sandbox backends verified usable,
  capability matrix (§8), resource totals, versions, retention windows, configured exposure
  policy. A capability listed here is a promise; one absent here answers `unserved`.
- **`/v1/ops/{op}`** answers only inside the authenticated subject and deployment namespace, for
  an operation id that subject minted: never seen | accepted & in flight | terminal outcome. This
  is the recovery surface for the unanswered modes (§6.2), not a cross-principal ledger oracle.
- Metrics are per-resource and machine-level usage, JSON. (Prometheus exposition on a second
  listener is an open question, §10.)

## 4. Sandbox & environment model

Owned by this contract and implemented behind substrate driver ports:

- Path confinement refuses both escape shapes (lexical and symlink, including dangling links),
  on every file operation and every mount resolution.
- Process spawn is argv-only through one builder per driver; env is cleared to a non-secret
  allowlist then explicitly shaped; captured streams are byte-capped with truncation notices.
- Sandbox availability is probed into a versioned capability snapshot (`bubblewrap`/`seatbelt` on
  host; container isolation on docker/k8s) and reported as fact. Admission binds the operation to
  the selected driver's snapshot; backend/configuration change invalidates it, and
  security-critical predicates are rechecked at operation start. `require: true` + unavailable
  backend = `refused`, with the missing backend named.

## 5. Leases

Liveness is asserted, never assumed. A workspace, exec, or workload created with `lease_ttl`
must be renewed (`POST /v1/{kind}/{id}/lease/renew`) before expiry; expiry is a **typed
transition** — exec killed, workload stopped, workspace frozen then collected after the
retention window — each emitting an event with reason `lease-expired`. A consumer that dies
leaves facts, not mysteries; autodev treats expiry exactly like an orphaned turn.

## 6. Error taxonomy

### 6.1 Answered outcomes

Every non-2xx body is `{class, code, message, address?, retriable, op?}`. `class` is the
structural discriminator — classification is never textual. `code` is a stable machine name
(`workspace.path-escape`, `exec.sandbox-unavailable`, `lease.expired`, …). `address` names
**what** was refused (a path, a capability, a limit) and never carries a value.

| class | HTTP | What happened | What the operator does |
|---|---|---|---|
| `refused` | 403/404/422 | Answered: a guard, validator, or precondition said no. Includes unknown ids. | Fix the request or widen authority. **Do not** retry unchanged. |
| `conflict` | 409 | Answered: state moved — op-id replayed with a different body, name taken, volume mounted, revision stale. | Re-read, re-decide, retry with fresh facts. |
| `unserved` | 501 | This daemon/driver does not implement the operation; it is absent from `/v1/machine`. | Pick another daemon, or stop asking. Retry never helps. |
| `exhausted` | 429/507 | Valid request, no capacity: quota, disk, concurrency. | Free resources or wait; retry the **same** `op` later. |
| `failed` | 500 | Accepted and attempted; the machinery itself failed terminally (driver error). | Inspect the substrate; retry the same `op` after repair. |

Two rules that keep the taxonomy honest:

- **Exit ≠ error.** A nonzero exit code, a failed build, a crashed workload are observed
  states on resources, answered `2xx`. The taxonomy above is about the *operation*, never the
  *outcome of the thing run*.
- **An answered failure never claims the link is broken.** Anything the daemon answered
  classifies as one of the five; "unreachable" keeps meaning what it says.

### 6.2 Unanswered outcomes, and how the contract serves them

Two more failure modes exist that a wire cannot carry, because they *are* the absence of the wire.
Substrate owns their wire meaning; clients may project them into their own internal taxonomies:

| Mode | What happened | What the client does |
|---|---|---|
| `unreachable` | No answer arrived; acceptance is not known. | Query `GET /v1/ops/{op}`. **Never mint a new `op` for an automatic mutation retry.** |
| `unknown` | Accepted, but the terminal outcome is unproven (connection died mid-flight). | Reconcile via the ledger and event replay. Never auto-retry a mutation under a fresh id. |

The contract's affordances for them are deliberate features, not conveniences: client-minted
`op` ids on every mutation, the operation ledger, cursor-replayable events, and leases that
turn a vanished client into a typed, observable expiry.

A Flux adapter can project this taxonomy into its delegate seam: `unserved` → `Unserved`; every
other answered class → `Refused` (Flux's own safe defaulting); transport silence → `Unreachable`;
accepted-unproven → `Unknown`. That adapter belongs to Flux and does not define this contract.

## 7. Catalog declarability constraints

The constraints that keep substrate a first-class connectors provider. Each is testable; a
change that breaks one is a breaking change to this contract, whatever the diff size.

1. **One operation = one method + path + JSON body**, expressible as a reviewed request
   template in catalog text. No signing, no cookies, no multi-leg handshakes.
2. **Substrate metadata lives in its released specification** (§2). The connector document is a
   deterministic, versioned translation of that bundle plus a connectors-owned projection manifest;
   the result is tested byte-for-byte against the compiled catalog artifact.
3. **Effects come from the closed v1 set** (§2). New effects are additive spec changes,
   declared before any operation carries them — never derived at runtime.
4. **Connectors owns the Connection shape.** Substrate exposes an authenticated endpoint and
   deployment identity; connectors binds those to its configured Connection without substrate
   redefining that noun. In the SaaS posture a LAN-bound
   daemon is unreachable by construction and the destination aperture must refuse it; the
   answer is connectors' **satellite posture** (design 03: a platform deployment near the
   endpoint, dialing up, later federated) — substrate grows no reverse-tunnel surface of its
   own.
5. **Every mutation is `keyed`** on a client `op` id, so platform-side retry policy needs no
   knowledge of substrate internals.
6. **Events are a closed, declared set with bounded cursor replay.** Durable connector delivery is
   a separate composition guarantee; its cursor, deduplication, gap recovery, and reconciliation
   are proposed in the umbrella RFC 0003.
7. **No secret values in ordinary JSON, ever.** Registries and secret slots are named daemon
   configuration; audit records stay value-free by type.
8. **Duplex is brokered, never proxied.** Sessions and tunnels are declared channel bindings
   with `expose: callable-direct` — and per connectors' byte-plane split (design 03, "beyond
   HTTP") that is doctrine, not a v1 compromise: continuous bidirectional bytes never ride
   the invoke path, because the credential broker must not become a byte proxy. The
   platform's role is **session establishment**: authorize under grants, return a daemon
   endpoint reference plus a short-lived, operation-scoped channel authority; the bytes then
   flow client ↔ daemon directly. Connectors Design 03 fixes the ownership and plane split;
   this contract still owes the substrate endpoint and channel-authority wire shape (§10).
9. **Lists are cursor-paginated and label-filterable**, uniformly.
10. **Capabilities gate features** (§8); the catalog declaration marks capability-dependent
    operations so the platform can render "not served here" as a fact, not an error.

## 8. Drivers and the capability matrix

One contract, three drivers. Cells are v1 intent; ✗ answers `unserved` and is absent from
`/v1/machine`. The k8s column is a design target, not a v1 commitment.

| Family / operation | host | docker | k8s (later) |
|---|---|---|---|
| Workspace (git materialize, guarded files, snapshot) | ✓ (substrate-owned confined tree) | ✓ (host tree, bind-mounted) | PVC + init container |
| Exec: run / background | ✓ processes, bwrap profiles | ✓ container per exec | pod / `exec` |
| Exec: interactive session | ✓ PTY | ✓ `exec -it` equivalent | `kubectl exec` equivalent |
| Workload | ✗ (no image runtime) | ✓ `--restart` policies | Deployment |
| Image: build / pull / push | ✗ | ✓ (`build.confined: false`) | kaniko/buildkit (confined) |
| Volume | ✓ directories | ✓ docker volumes | PVC |
| Endpoint: loopback / lan / tunnel | ✓ / config-gated / ✓ | ✓ / config-gated / ✓ | Service / config-gated / port-forward |
| Observe | ✓ | ✓ | ✓ |

Replace semantics differ (`recreate` on docker, `rolling` on k8s) and are declared as the
capability fact `workload.replace: recreate|rolling` rather than papered over.

## 9. Security posture, stated plainly

- The daemon is **data plane only**. Its authentication and subject-scoped resource ownership exist to bound blast radius; the
  governor is the platform's grant system and the operator's choice of which machines to
  enroll.
- **A docker-driver daemon is root-equivalent on its machine.** A caller holding the separately
  admitted Docker workload authority there can own the host. This is a fact of the substrate, not a
  solvable defect; the deployment is one trust domain, uses dedicated machines for untrusted work,
  and never shares one daemon across mutually untrusted hosted tenants.
- The host driver should run as a dedicated user; children get cleared environments and named
  secret slots only where declared; nothing a child observes includes daemon credentials.
- Confinement claims are always recorded per exec/workload — what was *asked* and what was
  *applied* — so an audit never has to infer.

## 10. Open questions

1. **The brokered-establishment handshake.** Direction is settled by architecture ADR 0010 (the
   platform brokers establishment; bytes flow direct), while authority and endpoint semantics are
   proposed cross-repository work in architecture RFC 0002. Sessions remain deferred until it is
   accepted.
2. **Bulk workspace transfer.** Bundles over HTTP are fine to ~100 MB; beyond that, a shared
   git remote is the honest answer. Does the contract need a third transfer mode, or is
   "use a remote" a documented limit? (Inherit autodev's honesty: no protocol that is elegant
   in a demo and miserable at 300 MB.)
3. **k8s driver scope.** Namespace-per-daemon via kubeconfig context is the working assumption;
   does anything in the workload family force CRDs? (Hope: no.)
4. **Confined builds.** Is a rootless-buildkit path worth carrying in v1 so `image.build.
   confined: true` exists anywhere, or does that wait for the k8s driver?
5. **Metrics exposition.** JSON only, or a second listener with Prometheus exposition
   (autodev's two-listener precedent makes the second listener cheap to justify)?
6. **Volume quotas.** Are size limits a v1 requirement or a capability fact added later?
