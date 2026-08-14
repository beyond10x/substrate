# Repository status

**Observed:** 2026-08-14

## Current state

Phase 3 **lifecycle and recovery** is complete under the
[archived closure disposition](docs/reviews/archived/2026-08-14-phase-3-closure-review-disposition.md).
The implementation has nineteen development routes; all 39 review findings have deterministic or
independently observed evidence. This completes implementation conformance, not stable publication:
OCI packaging, signing, and digest pinning remain separate release work. See
[Plan 03](docs/plan/03-lifecycle-and-recovery.md).

| Area | State | Next proof |
|---|---|---|
| Repository | private `daemonloom/substrate`; bot-authored `main` is synchronized | keep visibility, authorship, and portable-document invariants enforced |
| Boundary | accepted: standalone, generic execution data plane, Flux-free | enforce ADRs 0001–0006 in dependency and conformance tests |
| Wire contract | immutable 0.1.0 remains byte-clean; deterministic 0.2.0 has 19 closed operations, 19 runtime-executed vectors, 57 design vectors, and a reproducible fixed point | package, sign, and digest-pin a release without changing the development authority implicitly |
| Drivers | Linux host driver implemented; absent delegation keeps exec facts absent; real delegated lane passes | retain the delegated lane and add no optimistic facts |
| Security | `openat2` beneath/no-link/no-mount I/O, atomic replacement, cleared/shaped environment, namespace no-egress, pids/memory+swap/CPU cgroup bounds, output draining, timeout, and whole-tree kill are enforced | expand adversarial coverage without weakening admission |
| Stack integration | trust, session, event, federation, and contract-release seams accepted in umbrella ADRs 0015–0019 | keep later features behind their named phases |
| Implementation | phase 3 complete: durable provisional dispatch, first-terminal-wins, exact post-commit effects, scoped streams, bounded snapshots, fair maintenance, and crash recovery are independently reviewed | begin phase 4 only under a new explicit goal |

## Repository facts

- The Rust workspace has five crates: `substrate-wire`, `substrate-store`, `substrate-host`,
  `substrate-daemon`, and the offline `substrate-contract-check`; the reproducible 0.2.0 development
  bundle is authoritative until an explicitly packaged and signed release succeeds it.
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
- Both bundle trees classify every JSON document and meta-validate declared Draft 2020-12 schemas
  with the pinned standards validator. Semantic relations and fixed authorities are checked
  offline; seven negative gate tests prove unclassified JSON, invalid payloads, invalid schemas,
  and invalid authority targets fail closed.
- Runtime SQLite and guarded filesystem calls use separately bounded 16-slot blocking lanes;
  saturation tests prove unrelated async work remains schedulable. Snapshot GC bounds metadata,
  cascade-owned items, and expiry markers while preserving expired-versus-never-found behavior.
- Workspace cleanup advances in descriptor-relative 4,096-item batches without a total depth/item
  ceiling. Durable `destroying` blocks exec start and is automatically resumed after restart under
  fixed, subject-scoped lock stripes until the original destroy operation terminalizes.
- Git, sessions/stdin/PTY, workloads, images, volumes, endpoints, Docker, and Kubernetes are absent
  rather than stubbed.

## External dependencies

Substrate has no source dependency on another Daemonloom repository. Cross-repository compatibility
will use stable wire contracts and conformance fixtures, never sibling path dependencies.
