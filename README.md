# daemonloom/substrate

Substrate is the standalone Daemonloom execution data plane. It turns one machine—or one handed-over
cluster scope—into a governed service for confined workspaces, bounded processes, workloads, images,
volumes, endpoints, leases, and observed state.

Substrate runs things and reports what it observed. It does not decide product policy, run agent
loops, understand connector vendors, or depend on Flux. Consumers choose whether to call its stable
API directly or through a higher-level Daemonloom service.

**Status:** phase 3 lifecycle and recovery is complete under the
[archived closure disposition](docs/reviews/archived/2026-08-14-phase-3-closure-review-disposition.md).
The deterministic 0.2.0 development bundle, runtime, portable lane, and delegated Linux lane are
green. The bundle is not yet a published stable release: OCI packaging, signing, and digest pinning
remain release work. Phase 4 now has a source-typed, host-level bounded raw-pipe primitive; the
durable daemon session route, released bundle, PTY, and Git sources remain absent.

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
- [`adr/`](adr/) records accepted repository decisions with YAML frontmatter.
- [`crates/`](crates/) contains the standalone Rust wire, durable store, Linux host driver, and
  Unix-socket daemon; there is no sibling-repository dependency.
- [`contracts/substrate-wire/`](contracts/substrate-wire/) is the canonical development wire bundle;
  Rust types remain subordinate to it.
- [`scripts/check-runtime-vectors.py`](scripts/check-runtime-vectors.py) is an independent
  Unix-socket HTTP runner with an optional delegated-cgroup confinement lane.
- [`scripts/contract_json_gate.py`](scripts/contract_json_gate.py) fails closed on unclassified or
  schema-invalid contract JSON and meta-validates every Draft 2020-12 schema offline.
- [`STATUS.md`](STATUS.md) records observed progress; [`ROADMAP.md`](ROADMAP.md) records ordered exit
  criteria.

## Lifecycle daemon

`substrated` serves only an owner-permissioned Unix socket. Startup requires at least one explicit
`--allow-uid`; the daemon derives `local:<uid>` from kernel peer credentials and never accepts a
subject from HTTP data.

```console
cargo build --workspace --locked
target/debug/substrated \
  --socket ./run/substrate.sock \
  --state ./run/state.db \
  --workspaces ./run/workspaces \
  --deployment personal \
  --event-retention 10000 \
  --allow-uid 1000
```

Without a delegated cgroup root, workspace operations remain served and exec confinement facts are
absent, so exec admission answers `exec.sandbox-unavailable`. A Linux deployment that serves exec
must place the daemon in a delegated cgroup subtree with `cpu`, `memory`, and `pids`, keep the
delegation root itself process-free (for example with systemd `Delegate=yes` plus
`DelegateSubgroup=daemon`), and pass that root through `--cgroup-root`. The runtime probe enables and
tests the controllers, bubblewrap namespaces, cgroup kill, and swap-inclusive memory bound before it
advertises exec.

The daemon continuously drains both stdout and stderr while a process runs, retains their bounded
captures, persists them when the exec is observed, and exposes ranged reads. Phase 3 also streams
lifecycle events; it does not expose a live process-byte stream or stdin. Those belong to the
phase-4 daemon session channel. The host crate now has a development raw-pipe start/read/write/
half-close primitive with bounded queues, while the durable resource, attachment protocol, PTY
input, resize, explicit terminal/truncation frames, and reconnect semantics remain to be built.

Phase 4 now starts with a raw-pipe mode for machine protocols before PTY support. It preserves
stdin, stdout, and stderr as distinct bounded streams and is initially model-free and no-egress;
see [ADR 0007](adr/0007-protocol-processes-use-raw-pipe-sessions.md) and
[Plan 04](docs/plan/04-direct-byte-plane.md). No daemon session route is implemented yet, and the
host primitive does not by itself establish cross-repository or released-contract compatibility.

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

- [daemonloom/connectors](https://github.com/daemonloom/connectors) may govern substrate operations
  as a first-party provider and may later use substrate to isolate an attested connector artifact.
- [Flux](https://github.com/codewandler/flux) may implement a remote execution adapter over the
  substrate API. The dependency never points back into Flux.
- [autodev](https://github.com/codewandler/autodev) may implement its `Executor` port over substrate.
- `daemonloom/agent` and future products consume bounded execution through their own ports.
- `daemonloom/cloud` composes and operates substrate deployments; it does not own substrate rules.

The product and binary name are `substrate`. Published packages will use the
`daemonloom-substrate-*` prefix.
