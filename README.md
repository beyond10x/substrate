# daemonloom/substrate

Substrate is the standalone Daemonloom execution data plane. It turns one machine—or one handed-over
cluster scope—into a governed service for confined workspaces, bounded processes, workloads, images,
volumes, endpoints, leases, and observed state.

Substrate runs things and reports what it observed. It does not decide product policy, run agent
loops, understand connector vendors, or depend on Flux. Consumers choose whether to call its stable
API directly or through a higher-level Daemonloom service.

**Status:** the phase-2 minimum host slice is implemented and passes the contract, router,
guarded-filesystem, persistence, unserved-host, and delegated Linux confinement lanes. Phase 3 is
next; events, leases, sessions, Git sources, and every later resource family remain absent.

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
- [`STATUS.md`](STATUS.md) records observed progress; [`ROADMAP.md`](ROADMAP.md) records ordered exit
  criteria.

## Minimum daemon

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
  --allow-uid 1000
```

Without a delegated cgroup root, workspace operations remain served and exec confinement facts are
absent, so exec admission answers `exec.sandbox-unavailable`. A Linux deployment that serves exec
must place the daemon in a delegated cgroup subtree with `cpu`, `memory`, and `pids`, keep the
delegation root itself process-free (for example with systemd `Delegate=yes` plus
`DelegateSubgroup=daemon`), and pass that root through `--cgroup-root`. The runtime probe enables and
tests the controllers, bubblewrap namespaces, cgroup kill, and swap-inclusive memory bound before it
advertises exec.

Phase 2 continuously drains both stdout and stderr while a process runs, retains their bounded
captures, persists them when the exec is observed, and exposes ranged reads. It does not yet provide
a live byte stream or stdin: those belong to the phase-4 session channel, including PTY input,
resize, signals, explicit end/truncation frames, and bounded reconnect semantics. Git is likewise a
future, policy-confined workspace materialization/snapshot transport—not a runtime dependency; the
current daemon serves only `source: "empty"` and returns a typed `workspace.source-unserved`
response for a valid Git source request.

Run the portable black-box lane with:

```console
python3 scripts/check-runtime-vectors.py
```

Pass `--cgroup-root <delegated-root>` while the runner itself is inside that delegation to add the
real no-egress, shaped-environment, pids/memory, timeout, truncation, and whole-tree cancellation
cases.

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
