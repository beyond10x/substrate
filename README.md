# b10x substrate

Substrate is the standalone b10x execution data plane. It turns one machine—or one handed-over
cluster scope—into a governed service for confined workspaces, bounded processes, workloads, images,
volumes, endpoints, leases, and observed state.

Substrate runs things and reports what it observed. It does not decide product policy, run agent
loops, understand connector vendors, or depend on Flux. Consumers choose whether to call its stable
API directly or through a higher-level b10x service.

**Status:** phase 3 lifecycle and recovery is complete under the
[archived closure disposition](docs/reviews/archived/2026-08-14-phase-3-closure-review-disposition.md).
The deterministic 0.2.0 development bundle, runtime, portable lane, and delegated Linux lane are
green. The bundle is not yet a published stable release: OCI packaging, signing, and digest pinning
remain release work. Phase 4 now has a source-typed bounded raw-pipe primitive, distinct durable
session identity, leased start, and single-attachment Unix-WebSocket daemon route. The deterministic
0.4.0 successor development bundle adds independently verified read-only execution capsules; the
delegated model-free Agent compatibility lane proves capsule/config/hook binding and correlated
native hook evidence before model dispatch. PTY, network session authority, and Git sources remain
absent.

## Start here

1. [Vision](docs/VISION.md)
2. [Architecture overview](architecture/overview.md)
3. [Domain model](architecture/domain-model.md)
4. [Stack integration](architecture/stack-integration.md)
5. [API contract](docs/design/01-contract.md)
6. [Specification bundle and minimum wire](docs/design/07-specification-and-conformance.md)
7. [Roadmap](ROADMAP.md)

## Repository map

- [`architecture/`](architecture/) records the current accepted system boundary and dependency
  direction.
- [`docs/design/`](docs/design/) develops the wire, driver, lifecycle, security, session, and trust
  design. Each document states whether it is accepted or still under review.
- [`docs/plan/`](docs/plan/) turns the design into review gates and implementation slices without
  containing implementation.
- [`adr/`](adr/) records accepted component decisions with YAML frontmatter.
- [`crates/`](crates/) contains the standalone Rust wire, durable store, Linux host driver, and
  Unix-socket daemon; there is no sibling-component implementation dependency.
- [`contracts/substrate-wire/`](contracts/substrate-wire/) is the canonical development wire bundle;
  Rust types remain subordinate to it.
- [`scripts/check-runtime-vectors.py`](scripts/check-runtime-vectors.py) is an independent
  Unix-socket HTTP runner with an optional delegated-cgroup confinement lane.
- [`scripts/contract_json_gate.py`](scripts/contract_json_gate.py) fails closed on unclassified or
  schema-invalid contract JSON and meta-validates every Draft 2020-12 schema offline.
- [`STATUS.md`](STATUS.md) records observed progress; [`ROADMAP.md`](ROADMAP.md) records ordered exit
  criteria.

## Lifecycle daemon

By default, `substrate-daemon` serves an owner-permissioned Unix socket. Startup requires at least one
explicit `--allow-uid`; the daemon derives `local:<uid>` from kernel peer credentials and never
accepts a subject from HTTP data.

```console
cargo build --workspace --locked
target/debug/substrate-daemon \
  --socket ./run/substrate.sock \
  --state ./run/state.db \
  --workspaces ./run/workspaces \
  --deployment personal \
  --event-retention 10000 \
  --allow-uid 1000
```

Cloud may enable the current TCP transport only as an explicitly acknowledged development profile
on a private overlay (`--tcp-development-only --tcp-private-overlay`). That mode
requires a bounded `dl_substrate_v1_...` bearer file plus deployment-owned `--tcp-subject` and
`--tcp-actor` bindings; every HTTP route requires that bearer. The daemon opens that file once,
bounds it to 512 bytes, and admits either an owner-private workload file or a root-owned,
group-readable projected Secret with no group write/execute or world access. A configured
`--tcp-path-prefix /api/substrate` publishes the existing v1 contract below
`/api/substrate/v1`. This static bearer does not satisfy the accepted scoped, expiring, rotating
hosted trust-envelope profile and therefore must not be published through external or shared
ingress. The Cloud development chart keeps it cluster-internal and admits only its Connector
workload. A hosted container without a
delegated cgroup/bubblewrap environment continues to report execution sandbox unavailability
rather than weakening confinement.

The `substrate-daemon` crate exposes `DaemonConfig` plus the async `serve` entrypoint for this
component's own binary and tests. Cross-component consumers use the separately released native
`substrate-daemon` artifact and owner-released wire contract; they do not import this implementation
crate. Every operation crosses an authenticated socket boundary.

Without a delegated cgroup root, workspace operations remain served and exec confinement facts are
absent, so exec admission answers `exec.sandbox-unavailable`. A Linux deployment that serves exec
must place the daemon in a delegated cgroup subtree with `cpu`, `memory`, and `pids`, keep the
delegation root itself process-free (for example with systemd `Delegate=yes` plus
`DelegateSubgroup=daemon`), and pass that root through `--cgroup-root`. The runtime probe enables and
tests the controllers, bubblewrap namespaces, cgroup kill, and swap-inclusive memory bound before it
advertises exec.

The daemon continuously drains both stdout and stderr while a process runs, retains their bounded
captures, persists them when the exec is observed, and exposes ranged reads. Phase 3 also streams
lifecycle events. The phase-4 development route now reserves a leased exec durably before pipe
dispatch and provides one owner-permissioned Unix-WebSocket attachment with strict ordered stdin,
stdout, stderr, half-close, signal, truncation, error, and exit frames. Disconnect, protocol error,
send failure, and attachment lifetime expiry trigger whole-tree cancellation; no PTY or reconnect
is implied.

Phase 4 now starts with a raw-pipe mode for machine protocols before PTY support. It preserves
stdin, stdout, and stderr as distinct bounded streams and is initially model-free and no-egress;
see [ADR 0007](adr/0007-protocol-processes-use-raw-pipe-sessions.md) and
[Plan 04](docs/plan/04-direct-byte-plane.md). The route and an Agent-owned exact copy of the 0.4.0
development bundle now compose against a real daemon in a delegated cgroup. That model-free lane
proves no-egress execution, an empty exec-time environment, exact runtime/config/hook capsule
binding, read-only control files, writable-workspace separation, bounded framing and queue pressure,
attachment and protocol-failure containment, lease expiry, restart reconciliation, capsule and
whole-tree cleanup, and exact durable session/exec terminal evidence. Substrate reports the applied
capsule identity but does not claim the host interpreter/libraries/base system as part of that
closure. A signed stable contract release and public Agent `substrate_confined` report remain
separate release and product gates.

Each authenticated subject has a daemon-minted opaque source scope with its own durable generation,
sequence, retention, and coalesced wake hints. Pull and push read the same subject-local journal;
the final 0.2 contract will require snapshot-first durable bootstrap and an opaque resume cursor.
The snapshot and lease implementations provide a complete quota-bounded current-resource
projection, honest bounded provenance, transactional lifecycle freeze, and bounded fair cleanup.
The substrate-side prerequisite for connectors S-029 is therefore available; connector adoption
and contract pinning remain connectors-owned work.
Expired snapshot metadata and materialized items are physically garbage-collected under explicit
per-subject bounds while a bounded marker retains the `expired` versus `not found` distinction.
Terminal exec observations and output remain in memory until the durable store acknowledges them;
maintenance cannot regress a durable terminal state. Blocking filesystem and SQLite work uses
separate bounded lanes so saturation backpressures callers without starving asynchronous service.

Git remains a future, policy-confined workspace materialization/snapshot transport—not a runtime
dependency. The current daemon serves only `source: "empty"` and returns a typed
`workspace.source-unserved` response for a valid Git source request.

Run the portable black-box lane with:

```console
python3 scripts/check-runtime-vectors.py
```

Pass `--cgroup-root <delegated-root>` while the runner itself is inside that delegation to add the
real no-egress, shaped-environment, pids/memory, timeout, truncation, and whole-tree cancellation
cases.

The runner reports the current portable and delegated case inventories from each fresh execution;
documentation does not pin counts that can drift as adversarial coverage grows.

## Relationships

- [Connectors](https://github.com/beyond10x/connectors/tree/a8c393135478973a89c700d14478936eb0ae1df5) may govern substrate operations
  as a first-party provider and may later use substrate to isolate an attested connector artifact.
- [Flux](https://github.com/codewandler/flux) may implement a remote execution adapter over the
  substrate API. The dependency never points back into Flux.
- [autodev](https://github.com/codewandler/autodev) may implement its `Executor` port over substrate.
- The b10x agent and future products consume bounded execution through their own ports.
- The b10x cloud composes and operates substrate deployments; it does not own substrate rules.

The product and binary name are `substrate`. Published packages will use the
`b10x-substrate-*` prefix.
