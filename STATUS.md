# Repository status

**Observed:** 2026-08-17

## Current state

Phase 3 **lifecycle and recovery** is complete under the
[archived closure disposition](docs/reviews/archived/2026-08-14-phase-3-closure-review-disposition.md).
The phase-3 implementation has nineteen bundled development routes; the 0.4.0 successor development
bundle adds seven phase-4 pipe-session routes plus the bounded execution-capsule contract. All 39 phase-3 review findings have
deterministic or independently observed evidence. This completes phase-3 implementation
conformance, not stable publication:
OCI packaging, signing, and digest pinning remain separate release work. See
[Plan 03](docs/plan/03-lifecycle-and-recovery.md).

| Area | State | Next proof |
|---|---|---|
| Source | `foundation/substrate` in the predecessor monorepo; predecessor history is preserved | keep visibility, authorship, and portable-document invariants enforced |
| Boundary | accepted: standalone, generic execution data plane, Flux-free | enforce ADRs 0001–0006 in dependency and conformance tests |
| Wire contract | immutable 0.1.0 remains byte-clean; deterministic 0.2.0 and 0.3.0 remain reproducible; deterministic 0.4.0 has 26 closed operations, 21 executable vectors, 71 design vectors, 112 requirements, 11 hash fixtures, and a reproducible fixed point | package, sign, and digest-pin a complete runtime closure and stable release without changing development authority implicitly |
| Drivers | Linux host driver implemented; absent delegation keeps exec facts absent; real delegated lane passes | retain the delegated lane and add no optimistic facts |
| Security | `openat2` beneath/no-link/no-mount I/O, atomic replacement, cleared/shaped environment, namespace no-egress, pids/memory+swap plus cumulatively observed CPU cgroup bounds, backend-identity-bound capability snapshots, output draining, timeout, whole-tree kill, exact capsule-byte verification, read-only `/runtime`, separate writable `/workspace`, owner-private durable state, and bounded normal/restart capsule cleanup are enforced; static-bearer TCP is explicitly development-only | implement the accepted short-lived scoped hosted trust-envelope profile and retain the inline capsule proof while defining a signed complete runtime closure separately |
| Stack integration | trust, session, event, federation, and contract-release seams accepted in umbrella ADRs 0015–0019 | keep later features behind their named phases |
| Implementation | phase 3 complete; the phase-4 raw-pipe slice has distinct durable session identity, session-native lifecycle operations, one scoped Unix-WebSocket attachment, atomic terminal/restart projection, verified execution capsules, and a green delegated model-free Agent lane with correlated hook evidence | retain the raw-pipe/capsule evidence while adding only separately gated PTY, authority, and release work |

## Repository facts

- The Rust workspace has five crates: `substrate-wire`, `substrate-store`, `substrate-host`,
  `substrate-daemon`, and the offline `substrate-contract-check`; the reproducible 0.4.0 successor
  bundle is the current development contract while all earlier bundle directories remain immutable,
  with the single recorded 2026-08-24 exception of the brand rename, which re-rendered every bundle
  in place (AGENTS.md invariant 6).
  No development bundle becomes a stable owner release without packaging and signing.
- No Flux package, type, protocol, or checkout is required.
- The clean-room runner reports its current portable and delegated case inventories in fresh gate
  output. Portable execution proves typed `unserved` without confinement; delegated execution adds
  actual bounded exec, capacity pressure, trapped TERM, output durability, and idle-time
  whole-cgroup lease expiry.
- Current Rust tests prove provisional dispatch before host mutation, full first-terminal-wins
  behavior across signal/expiry, exact post-commit event effects, subject-local wake hints,
  restart-to-unknown without redispatch, observed-effect/store-failure recovery, real WebSocket
  limits, capped fair maintenance across reopen, lease clocks, symlink escape refusal, and strict
  minimum host limits.
- All four bundle trees classify every JSON document and meta-validate declared Draft 2020-12 schemas
  with the pinned standards validator. Semantic relations and fixed authorities are checked
  offline; seven negative gate tests prove unclassified JSON, invalid payloads, invalid schemas,
  and invalid authority targets fail closed.
- Runtime SQLite and guarded filesystem calls use separately bounded 16-slot blocking lanes;
  saturation tests prove unrelated async work remains schedulable. Snapshot GC bounds metadata,
  cascade-owned items, and expiry markers while preserving expired-versus-never-found behavior.
- Workspace cleanup advances in descriptor-relative 4,096-item batches without a total depth/item
  ceiling. Durable `destroying` blocks exec start and is automatically resumed after restart under
  fixed, subject-scoped lock stripes until the original destroy operation terminalizes.
- Git, PTY, reconnect, workloads, images, volumes, endpoints, Docker, and Kubernetes are absent
  rather than stubbed. The development pipe session is the sole phase-4 byte-plane slice.

Phase 4 is now explicitly active under
[ADR 0007](adr/0007-protocol-processes-use-raw-pipe-sessions.md) and
[Plan 04](docs/plan/04-direct-byte-plane.md). The daemon durably reserves the leased underlying exec
before dispatch, scopes attachment by authenticated subject, admits one attachment, and terminates
the tree on invalid ordering or attachment loss. Semantic route tests and Agent's independent
copied-contract fixture pass. The deterministic 0.4.0 bundle and real delegated Agent compatibility
lane now also pass, including exact capsule/config/hook byte binding, read-only runtime material,
correlated hook lifecycle, bounded queue pressure, attachment/protocol loss, lease expiry,
restart-to-unknown, capsule reconciliation, and whole-tree cleanup. This is development cross-component confinement
evidence; it is not a signed stable release or public product conformance claim.

## External dependencies

Substrate has no source dependency on another b10x component. Cross-component compatibility
uses stable wire contracts and conformance fixtures, never sibling implementation-path dependencies.
