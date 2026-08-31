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

Source: <https://github.com/beyond10x/substrate/> · Security reports:
<https://github.com/beyond10x/substrate/security/advisories/new>

Substrate is licensed under [Apache-2.0](LICENSE), including all beyond10x-owned material in its
reachable history. Third-party material retains its own licence. Public source does not make the
development wire contract stable.

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

**Tagged `0.2.3` (2026-08-30) — a keyless-signed daemon image. Development bundles, not a stable
published contract.**

| area | state |
|---|---|
| phase 3, lifecycle and recovery | **complete**, under the [archived closure disposition](docs/reviews/archived/2026-08-14-phase-3-closure-review-disposition.md); all 39 review findings carry deterministic or independently observed evidence |
| 0.2.0 bundle, runtime, portable lane, delegated Linux lane | green |
| 0.4.0 successor development bundle | adds independently verified read-only execution capsules; the delegated model-free lane proves capsule/config/hook binding and correlated native hook evidence before model dispatch |
| phase 4, [raw pipe sessions](adr/0007-protocol-processes-use-raw-pipe-sessions.md) | source-typed bounded raw-pipe primitive, distinct durable session identity, leased start, single-attachment Unix-WebSocket route ([plan 04](docs/plan/04-direct-byte-plane.md)) |
| daemon image release | [`.github/workflows/release.yml`](.github/workflows/release.yml) publishes, keyless-signs and digest-pins `ghcr.io/beyond10x/b10x-substrate-daemon:<version>` on an annotated bare-version tag whose commit has a green gate run; a pre-release tag publishes nothing. `0.2.3` is published at `sha256:ab101582…`, keyless-signed. It needs no repository secret: the image push, the release and the changelog commit all use the run's own `GITHUB_TOKEN`. The changelog digest line lands by pull request — `main` is protected and declines a direct push |
| stable publication | **not done.** The contract bundles' OCI packaging, signing and digest pinning are separate release work; no bundle is a stable published contract |
| PTY, network session authority, Git sources | **absent** |
| hosted trust envelope | accepted in design, **not implemented**; the TCP static bearer is explicitly development-only |

Per-area state with the exact next proof each is waiting for is [`STATUS.md`](STATUS.md); ordered
exit criteria are [`ROADMAP.md`](ROADMAP.md).

## Build, test, run

The gate is **`bash scripts/gate.sh`**. It is the full component gate; green here is the bar for
main.

The table is the gate's own order (`scripts/gate.sh`).

| step | command |
|---|---|
| tests | `cargo test --workspace --locked` |
| format | `cargo fmt --all --check` |
| lint | `cargo clippy --workspace --all-targets --locked -- -D warnings` |
| links | `cargo xtask check-links` — rejects machine-local and broken repository-relative links |
| ADRs | `cargo xtask check-adrs` |
| secrets | `cargo xtask check-secrets` — scans every reachable commit, including root trees |
| dependencies | `cargo xtask check-advisories` — rejects RustSec findings, HTTP/2 and publishable crates |
| licences | `cargo xtask check-licenses` — verifies Apache-2.0 workspace metadata and deterministic third-party notices |
| contract bundle 0.1.0 | `python3 scripts/check-contract-bundle.py` |
| contract bundle 0.2.0 | `python3 scripts/check-contract-bundle-0.2.0.py` |
| contract bundle 0.3.0 | `python3 scripts/check-contract-bundle-0.3.0.py` |
| contract bundle 0.4.0 | `python3 scripts/check-contract-bundle-0.4.0.py` |
| contract bundle 0.5.0 | `cargo xtask check-bundle 0.5.0` — re-renders from its authored source and compares bytes |
| contract bundle 0.6.0 | `cargo xtask check-bundle 0.6.0` |
| contract bundle 0.7.0 | `cargo xtask check-bundle 0.7.0` |
| contract bundle 0.8.0 | `cargo xtask check-bundle 0.8.0` |
| contract bundle 0.9.0 | `cargo xtask check-bundle 0.9.0` — checks the multi-major registry and served catch-all paths |
| contract JSON | `cargo xtask check-json` — every JSON under `contracts/` is classified by exactly one bundled schema, or it fails closed |
| toolchain | `cargo xtask check-toolchain` |

Rust 1.97, edition 2024 — the toolchain is pinned by `rust-toolchain.toml`, and
`cargo xtask check-toolchain` fails when it, `Cargo.toml`'s `rust-version` and the `Dockerfile`
builder tag disagree. `.github/workflows/gate.yml` runs the same gate on push and pull request.

Every check the gate runs is a `cargo xtask` verb — anything that runs in a b10x foundation
repository is Rust. Two verbs are not gate steps because `cargo test --workspace --locked`, the
gate's first step, already covers them:

| verb | what it does |
|---|---|
| `cargo xtask package-bundle <version> --out <dir>` | packages a released bundle as a deterministic OCI image layout, so a consumer can pin one manifest digest |
| `cargo xtask render-bundle <version> --out <dir>` | renders a bundle tree from `substrate-wire` and the authored source at `xtask/bundle-source/<version>/`; this is how a successor bundle is cut, and it refuses to write anywhere under `contracts/` |

`cargo xtask check-bundle <version>` **is** a gate step, from `0.5.0` on. It replaces the
per-version Python checker: re-rendering and comparing bytes catches a released tree that has
stopped being the fixed point of its own source, which a hand-written checker cannot see.

The four `scripts/render-contract-bundle*.py` and their `check-contract-bundle*.py` partners are
**not tooling** — they are the reproducibility proof of the frozen `0.1.0`–`0.4.0` bundles, which
are immutable, and `0.4.0`'s own `generator.name` points at one of them. They stay in Python and
are not ported. `render-bundle` dispatches to the frozen renderer for `0.5.0`–`0.8.0` and the
versioned multi-major renderer for `0.9.0`; a test asserts the original renderer still reproduces
the frozen `0.4.0` byte for byte.

`crates/substrate-daemon/tests/runtime_vectors.rs` is the clean-room runner — an independent
Unix-socket HTTP lane that spawns the shipped `substrate-daemon` binary and asserts only on the
wire, linking no implementation. It has no gate step of its own because the gate's first step,
`cargo test --workspace --locked`, runs it. **`bash scripts/delegated-lane.sh` runs that lane**, and needs no privilege: it asks systemd for a
delegated scope (`systemd-run --user -p Delegate=yes --scope`), moves itself into a child group so
the delegation root stays process-free, and sets the variable. A user session's own scope is
root-owned, so trying to `mkdir` in it fails and the lane reports itself **absent, not passed**
(invariant 3) — which reads exactly like a green run if you only look at `cargo test`.

Set `SUBSTRATE_VECTORS_CGROUP_ROOT=<delegated-root>` by hand instead,
**while the test process itself is inside that delegation**, to add the real no-egress,
shaped-environment, pids/memory, timeout, truncation and whole-tree cancellation cases:

```console
cargo test --workspace --locked -- --nocapture   # the runner prints its case inventory
```

The runner prints its current portable or delegated case inventory from each fresh execution;
this document deliberately pins no counts that drift as adversarial coverage grows.

`cargo xtask check-json` fails closed on unclassified or schema-invalid contract JSON and
meta-validates every Draft 2020-12 schema offline, across all nine released bundles. Classification
used to live in a Python module the four checkers imported — shared live machinery, not any one
bundle's reproducibility proof — so it moved with the rest of the tooling, and the four checkers no
longer do it. They verify everything else about the bundles they froze.

`cargo xtask package-bundle <version> --out <dir>` packages a released contract bundle as a
deterministic OCI image layout — `0.4.0` reproduces manifest
`sha256:3758e80bc39f1eb03b15c69410608c9ef1d2ba8095c7e707c6988dbb5894ab00`. It reads
`contracts/` read-only and is not a gate step of its own: its cases run in `cargo test`.

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

### Secret slots

`--secret-slot <name>=<path>` (repeatable) declares a secret the daemon holds for a run. The value
reaches a child only as a **sealed memfd at a declared descriptor** — never in argv, never in the
environment, never in an event, the ledger or an error body. The child gets the mapping and nothing
else, through `SUBSTRATE_SECRET_SLOTS=<name>=<fd>,…`; the daemon closes its own copy immediately
after spawn, and the seal set is `F_SEAL_WRITE|F_SEAL_SHRINK|F_SEAL_GROW|F_SEAL_SEAL`, which a child
can confirm with `fcntl(fd, F_GET_SEALS)`.

```console
target/debug/substrate-daemon \
  --socket ./run/substrate.sock \
  --state ./run/state.db \
  --workspaces ./run/workspaces \
  --deployment personal \
  --allow-uid 1000 \
  --secret-slot model_key=/etc/substrate/model-key
```

A slot **name** is lowercase ASCII, digits and `_`, first character a letter, at most 64 bytes
(`crates/substrate-wire/src/lib.rs:1766`) — a hyphen is refused. The **path** never leaves the
daemon process — it is not a capability fact, not an event field and
not an error message. An error may name a slot; it never names a value. Rotating the file behind a
declared name needs no restart and invalidates no admitted operation. The ledger request hash covers
slot **names** only, so two requests differing only in a slot's value hash identically. Where sealing
is unavailable the capability fact `secrets.slots` is **absent and the operation is refused by
name** — it never degrades to passing the value some other way (invariant 3). ADR 0012.

### Egress apertures

Ordinary execution has no egress and that does not move: every run is under `--unshare-net` in a
namespace with loopback and nothing else. An **aperture** is a separate, operator-declared authority
to reach exactly one destination — `--egress-aperture <name>=<host>:<port>/tcp[/max=<size>]`,
repeatable. A request selects one **by name** and can never carry a destination or a ceiling:

```console
target/debug/substrate-daemon \
  --socket ./run/substrate.sock \
  --state ./run/state.db \
  --workspaces ./run/workspaces \
  --deployment personal \
  --allow-uid 1000 \
  --cgroup-root /sys/fs/cgroup/…/substrate \
  --egress-aperture model=api.example.com:443/tcp \
  --ca-bundle /etc/ssl/certs/ca-certificates.crt
```

```json
{ "sandbox": { "network": "aperture", "aperture": "model", "profile": "workspace", "…": "…" } }
```

The host is resolved **once**, at declaration, and pinned; the sandbox gets no resolver and performs
no lookup. Inside the run, the declared name maps to loopback through a generated read-only
`/etc/hosts` and the forwarder listens on the declared port, so `https://api.example.com/…` is the
URL a child uses unchanged — and, where `--ca-bundle` is configured, verifies against a private
per-run snapshot of that anchor. The pinned address itself is **not** reachable directly: the
aperture is the only peer in the namespace.

What was installed is reported rather than inferred — `applied.network` becomes
`{mode, name, destination, mechanism, bytes, max_bytes}`, with the address the forwarder actually
dialled and the bytes counted where they crossed. An aperture nobody declared is `unserved` **with
the name in the message**; where the mechanism did not verify in a throwaway sandbox at startup, the
capability fact `exec.egress-apertures` is absent and every aperture request is `unserved` — never a
run that quietly got no network instead (invariant 3). ADR 0013.

The optional `/max=<size>` term bounds **how much** may cross, over both directions summed, for one
run — `1048576`, `512KiB`, `64MiB`, `2GiB`, and never a decimal-power unit such as `MB`. An
unrecognised term is a startup error, not an ignored one, and an aperture declared without the term
passes exactly what it passed before. The relay stops relaying at the ceiling, so the total may
exceed it by at most one 16 KiB relay buffer per live connection; the run is then ended and the
observation carries `refusal: {class: "exhausted", code: "exec.aperture-byte-limit", …}` beside a
state of `cancelled`. The child is told nothing — its socket closes mid-stream and the tree is
killed. A ceiling in a request is refused `exec.aperture-ceiling-in-request`. ADR 0014.

### Grant attribution

`--delegated-context-key <kid>=<issuer>=<base64url>` (repeatable) declares a key substrate will
**verify** delegated-context documents against. Substrate holds a verifying key and never a signing
key: which service signs is a configuration of the trusted key and changes no substrate code.

A start may then carry `delegated_context`, a compact JWS, alongside `op` and `input` — never inside
`input`, so it stays outside the canonical request hash. Replaying the same `op` with a *fresh*
context is the same operation and returns the original outcome, and a request without one serializes
exactly as a `0.6.0` client's did.

```console
substrate-daemon \
  --delegated-context-key k1=https://identity.example.com=g5Iv…A0
```

What a verified document contributes is two columns and nothing else: `grant_ref` and
`platform_principal`, on the ledger row and on the `operation.*` events. Substrate verifies
signature, issuer, exact audience, time window and binding to the authenticated subject; it never
evaluates the grant — connectors decides, substrate records. Identity-shaped strings a caller may
legitimately write reach the resource and never the attribution; writing one into the envelope is
`request.schema-invalid`, not a quiet drop. Every failure is a named refusal, never a weaker run:
`delegated-context.absent`, `.malformed`, `.unknown-key`, `.signature-invalid`,
`.audience-mismatch`, `.subject-mismatch`, `.expired`, `.grant-conflict`
(`crates/substrate-daemon/src/delegation.rs:209-256`). ADR 0011.

### Serving exec

Without a delegated cgroup root, workspace operations are still served, exec confinement facts are
absent, and exec admission answers `exec.sandbox-unavailable`. A Linux deployment that serves exec
must:

1. place the daemon in a delegated cgroup subtree carrying `cpu`, `memory` and `pids`;
2. keep the delegation root itself **process-free** — for example systemd `Delegate=yes` plus
   `DelegateSubgroup=daemon`;
3. provide the configured bubblewrap binary and `/usr/bin/socat`, which the runtime probe uses to
   prove that the seccomp profile denies host Unix-socket access;
4. pass that root through `--cgroup-root`.

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
| [`.engineering/planning/`](.engineering/planning/) | the plan: epics and stories as governed artifacts, read with `protocol artifact list` / `board` |
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
