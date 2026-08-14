# Repository status

**Observed:** 2026-08-14

## Current state

Phase 3 **lifecycle and recovery** is reopened under the
[phase-3 closure review](docs/reviews/2026-08-14-phase-3-closure-review.md). The implementation has
nineteen development routes and its durable-dispatch, first-terminal-wins, and subject-scoped event
commit boundaries are green. It is **not** ready to publish: workspace lifecycle admission,
finite-ledger refusal, bounded snapshots and maintenance, transport capacity, semantic schemas,
and the final adversarial lanes remain open. See [Plan 03](docs/plan/03-lifecycle-and-recovery.md).

| Area | State | Next proof |
|---|---|---|
| Repository | private `daemonloom/substrate`; bot-authored `main` is synchronized | keep visibility, authorship, and portable-document invariants enforced |
| Boundary | accepted: standalone, generic execution data plane, Flux-free | enforce ADRs 0001–0006 in dependency and conformance tests |
| Wire contract | immutable 0.1.0 remains byte-clean; the 0.2.0 tree is an uncommitted pre-curation development bundle and does not yet match the reopened runtime/design invariants | regenerate only 0.2.0 from exact schemas and executable vectors, then prove a reproducible fixed point |
| Drivers | Linux host driver implemented; absent delegation keeps exec facts absent; real delegated lane passes | retain the delegated lane and add no optimistic facts |
| Security | `openat2` beneath/no-link/no-mount I/O, atomic replacement, cleared/shaped environment, namespace no-egress, pids/memory+swap/CPU cgroup bounds, output draining, timeout, and whole-tree kill are enforced | expand adversarial coverage without weakening admission |
| Stack integration | trust, session, event, federation, and contract-release seams accepted in umbrella ADRs 0015–0019 | keep later features behind their named phases |
| Implementation | closure in progress: provisional dispatch precedes host mutation; first full terminal exec wins transactionally; all event transactions report exact post-commit effects into subject-scoped coalesced wake hints | close review findings 5–39 before beginning phase 4 |

## Repository facts

- The Rust workspace has five crates: `substrate-wire`, `substrate-store`, `substrate-host`,
  `substrate-daemon`, and the offline `substrate-contract-check`; the released contract bundle will
  remain authoritative once 0.2.0 is regenerated and independently proven.
- No Flux package, type, protocol, or checkout is required.
- The clean-room runner reports its current portable and delegated case inventories in fresh gate
  output. Portable execution proves typed `unserved` without confinement; delegated execution adds
  actual bounded exec, capacity pressure, trapped TERM, output durability, and idle-time
  whole-cgroup lease expiry.
- Current Rust tests prove provisional dispatch before host mutation, full first-terminal-wins
  behavior across signal/expiry, exact post-commit event effects, subject-local wake hints,
  restart-to-unknown without redispatch, lease clocks, symlink escape refusal, and strict minimum
  host limits. The open review records the deterministic race/capacity/restart evidence still due.
- Both bundle trees currently classify every JSON document and meta-validate declared Draft
  2020-12 schemas with the pinned standards validator. That structural gate is green, but the open
  review proves 0.2 schemas still accept semantically invalid wire/vector values; it is not release
  evidence until those schemas and negative tests are regenerated.
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
