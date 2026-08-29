# substrate

The b10x execution data plane. It turns one machine — or one handed-over cluster scope — into a
governed service for confined workspaces, bounded processes, workloads, images, volumes, endpoints,
leases, and observed state.

The problem it removes: anything that wants to run a command on behalf of somebody else has to
build confinement, quotas, durable lifecycle state and honest reporting for itself, and usually
builds the reporting optimistically. Substrate runs things and reports **what it observed**. Where
the machine cannot confine, it says so — an exec on a host with no delegated cgroup answers
`exec.sandbox-unavailable` rather than running unconfined.

It does not decide product policy, run agent loops, or understand connector vendors.

Public documentation: <https://beyond10x.github.io/substrate/>

## Where it sits

| direction | what |
|---|---|
| confines | [harness](https://github.com/beyond10x/harness) — embedded in-process, or over the daemon socket |
| may govern | [connectors](https://github.com/beyond10x/connectors) — as a first-party provider, and later to isolate an attested connector artifact |
| may execute for | [autodev](https://github.com/beyond10x/autodev) — over its `Executor` port |
| may adapt | [flux](https://github.com/codewandler/flux) — a remote execution adapter over the substrate API. The dependency never points back into Flux |
| mapped in | [atlas](https://github.com/beyond10x/atlas) |

There is **no sibling-component implementation dependency**. Cross-component consumers use the
released native `substrate-daemon` artifact and the owner-released wire contract; they do not
import the implementation crates.

The product and binary name are `substrate`. Published packages will use the `b10x-substrate-*`
prefix.

## Status

**Tagged `0.2.1` (2026-08-29) — a public documentation and distribution release with no runtime or
wire change. Development bundles, not a stable published contract.**

| area | state |
|---|---|
| phase 3, lifecycle and recovery | **complete**, under the [archived closure disposition](docs/reviews/archived/2026-08-14-phase-3-closure-review-disposition.md); all 39 review findings carry deterministic or independently observed evidence |
| 0.2.0 bundle, runtime, portable lane, delegated Linux lane | green |
| 0.4.0 successor development bundle | adds independently verified read-only execution capsules; the delegated model-free lane proves capsule/config/hook binding and correlated native hook evidence before model dispatch |
| phase 4, [raw pipe sessions](adr/0007-protocol-processes-use-raw-pipe-sessions.md) | source-typed bounded raw-pipe primitive, distinct durable session identity, leased start, single-attachment Unix-WebSocket route ([plan 04](docs/plan/04-direct-byte-plane.md)) |
| stable publication | **not done.** OCI packaging, signing and digest pinning are separate release work |
| PTY, network session authority, Git sources | **absent** |
| hosted trust envelope | accepted in design, **not implemented**; the TCP static bearer is explicitly development-only |

Per-area state with the exact next proof each is waiting for is [`STATUS.md`](STATUS.md); ordered
exit criteria are [`ROADMAP.md`](ROADMAP.md).

## Build, test, run

The gate is **`bash scripts/gate.sh`**. It is the full component gate; green here is the bar for
main.

| step | command |
|---|---|
| tests | `cargo test --workspace --locked` |
| format | `cargo fmt --all --check` |
| lint | `cargo clippy --workspace --all-targets --locked -- -D warnings` |
| links | `python3 scripts/check-links.py` — rejects machine-local and broken repository-relative links |
| ADRs | `python3 scripts/check-adrs.py` |
| contract bundle | `python3 scripts/check-contract-bundle.py` |
| runtime vectors | `python3 scripts/check-runtime-vectors.py` |
| brand | `bash scripts/check-brand.sh` |

Rust 1.97, edition 2024.

`scripts/check-runtime-vectors.py` is an independent Unix-socket HTTP runner — a black-box lane that
does not link the implementation. Pass `--cgroup-root <delegated-root>`, **while the runner itself
is inside that delegation**, to add the real no-egress, shaped-environment, pids/memory, timeout,
truncation and whole-tree cancellation cases. The runner reports its current portable and delegated
case inventories from each fresh execution; this document deliberately pins no counts that drift as
adversarial coverage grows.

`scripts/contract_json_gate.py` fails closed on unclassified or schema-invalid contract JSON and
meta-validates every Draft 2020-12 schema offline.

### Running the daemon

By default `substrate-daemon` serves an owner-permissioned Unix socket. Startup requires at least
one explicit `--allow-uid`; the daemon derives `local:<uid>` from **kernel peer credentials** and
never accepts a subject from HTTP data.

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

### Serving exec

Without a delegated cgroup root, workspace operations are still served, exec confinement facts are
absent, and exec admission answers `exec.sandbox-unavailable`. A Linux deployment that serves exec
must:

1. place the daemon in a delegated cgroup subtree carrying `cpu`, `memory` and `pids`;
2. keep the delegation root itself **process-free** — for example systemd `Delegate=yes` plus
   `DelegateSubgroup=daemon`;
3. pass that root through `--cgroup-root`.

The runtime probe enables and tests the controllers, bubblewrap namespaces, cgroup kill and the
swap-inclusive memory bound **before** it advertises exec.

### The TCP transport is development-only

The current TCP transport is enabled only as an explicitly acknowledged development profile on a
private overlay (`--tcp-development-only --tcp-private-overlay`), and requires a bounded bearer file
plus deployment-owned `--tcp-subject` and `--tcp-actor` bindings. The daemon opens that file once,
bounds it to 512 bytes, and admits either an owner-private workload file or a root-owned,
group-readable projected Secret with no group write/execute and no world access.

**This static bearer does not satisfy the accepted scoped, expiring, rotating hosted trust-envelope
profile, and must not be published through external or shared ingress.** A hosted container without
a delegated cgroup or bubblewrap environment continues to report execution sandbox unavailability
rather than weakening confinement.

## What is enforced

| area | enforced |
|---|---|
| filesystem | `openat2` beneath / no-link / no-mount I/O, atomic replacement, symlink escape refusal |
| process | cleared and shaped environment, namespace no-egress, `pids` and memory-plus-swap bounds, cumulatively observed CPU bounds, timeout, whole-tree kill |
| capsules | exact capsule-byte verification, read-only `/runtime`, separate writable `/workspace`, bounded normal and restart cleanup |
| output | both stdout and stderr drained continuously while a process runs, bounded captures retained, persisted when the exec is observed, ranged reads exposed |
| durability | terminal observations and output stay in memory until the durable store acknowledges them; maintenance cannot regress a durable terminal state |
| concurrency | blocking filesystem and SQLite work runs in separate bounded lanes, so saturation backpressures callers without starving asynchronous service |

Substrate reports the applied capsule identity. It does **not** claim the host interpreter,
libraries or base system as part of that closure.

Git remains a future, policy-confined workspace materialization and snapshot transport — not a
runtime dependency. The current daemon serves only `source: "empty"` and returns a typed
`workspace.source-unserved` for a valid Git source request.

## Layout

| crate | owns |
|---|---|
| `crates/substrate-wire` | the closed Rust representation of the wire; **subordinate to the contract bundle**, never the other way round |
| `crates/substrate-store` | durable operation and resource state |
| `crates/substrate-host` | the Linux host driver |
| `crates/substrate-daemon` | the standalone HTTP daemon: `DaemonConfig` plus the async `serve` entrypoint |
| `crates/substrate-contract-check` | the offline contract checker |

| path | holds |
|---|---|
| [`contracts/substrate-wire/`](contracts/substrate-wire/) | the canonical wire bundles, one directory per version; earlier bundles are immutable |
| [`architecture/`](architecture/) | the accepted system boundary and dependency direction |
| [`docs/design/`](docs/design/) | wire, driver, lifecycle, security, session and trust design; each document states whether it is accepted or under review |
| [`docs/plan/`](docs/plan/) | design turned into review gates and implementation slices, without implementation |
| [`adr/`](adr/) | accepted component decisions, with YAML frontmatter |
| [`scripts/`](scripts/) | `gate.sh` and the checks it runs |

## Read more

Start here, in order:

1. [Vision](docs/VISION.md)
2. [Architecture overview](architecture/overview.md)
3. [Domain model](architecture/domain-model.md)
4. [Stack integration](architecture/stack-integration.md)
5. [API contract](docs/design/01-contract.md)
6. [Specification bundle and minimum wire](docs/design/07-specification-and-conformance.md)
7. [Roadmap](ROADMAP.md)

Also: [`glossary.md`](glossary.md), [`STATUS.md`](STATUS.md), [`CHANGELOG.md`](CHANGELOG.md), and
[`AGENTS.md`](AGENTS.md) for the working agreements and invariants.
