# Repository status

**Observed:** 2026-08-29

## Current state

Phase 3 **lifecycle and recovery** is complete under the
[archived closure disposition](docs/reviews/archived/2026-08-14-phase-3-closure-review-disposition.md),
which records findings 1–39 closed. Phase 4 **direct byte plane** is in progress
([ROADMAP.md](ROADMAP.md), [Plan 04](docs/plan/04-direct-byte-plane.md)): the model-free raw-pipe
session slice and the bounded read-only execution capsule are green, and PTY and network session
authority remain absent. Route closure happened in 0.3.0, which added the seven pipe-session
operations to 0.2.0's nineteen; 0.4.0 keeps those 26 and adds six execution-capsule driver vectors
without adding a route. This is development conformance, not stable publication: nothing in this
repository publishes, signs or digest-pins anything yet.

| Area | State | Next proof |
|---|---|---|
| Source | the standalone public repository `beyond10x/substrate` (`git remote -v`), public under atlas ADR 0003 ([CHANGELOG.md](CHANGELOG.md), 0.2.1); the latest annotated tag is `0.2.2`, cut at the `main` commit whose CI gate run concluded `success` | keep the portable-document and credential invariants enforced — `cargo xtask check-links` in the gate, [`scripts/check-secrets.sh`](scripts/check-secrets.sh) before a release |
| CI | [`.github/workflows/gate.yml`](.github/workflows/gate.yml) runs `bash scripts/gate.sh` bare on push to `main`, on pull request and on dispatch, so the job's status is the gate's own; branch protection on `main` requires the `Full gate` check; the first green run is [33275398365](https://github.com/beyond10x/substrate/actions/runs/33275398365). The delegated lane is **absent** there, never reported as passed: a hosted runner has neither bubblewrap nor a delegated cgroup subtree | either give the delegated lane a runner that has both, or keep it the named local pre-release step it is today |
| Release | [`Dockerfile`](Dockerfile) builds the daemon and `cargo xtask package-bundle <version> --out <dir>` emits a deterministic OCI image layout from a frozen bundle (0.4.0 → manifest `sha256:3758e80bc39f1eb03b15c69410608c9ef1d2ba8095c7e707c6988dbb5894ab00`); **no** workflow builds, publishes, signs or digest-pins either artifact | publish and sign the daemon image (`story:signed-daemon-image`); publish and sign the 0.4.0 bundle layout (`story:contract-bundle-oci-artifact`) |
| Boundary | accepted: standalone, generic execution data plane, Flux-free ([ADR 0001](adr/0001-substrate-is-standalone-and-flux-free.md)) | enforce ADRs 0001–0006 in dependency and conformance tests |
| Wire contract | 0.1.0, 0.2.0 and 0.3.0 remain byte-clean and reproducible; the development bundle 0.4.0 has 199 files, 200 classified JSON documents, 26 closed operations, 21 executable vectors, 71 design vectors, 112 requirements, 11 exact hash fixtures and a reproducible fixed point ([`scripts/check-contract-bundle-0.4.0.py`](scripts/check-contract-bundle-0.4.0.py)) | package, sign and digest-pin a complete runtime closure and a stable release without changing development authority implicitly |
| Drivers | Linux host driver implemented; absent delegation keeps exec facts absent and answers `exec.sandbox-unavailable` (501, error class `unserved`) rather than degrading, proven by the portable lane of [`crates/substrate-daemon/tests/runtime_vectors.rs`](crates/substrate-daemon/tests/runtime_vectors.rs); the delegated lane runs only when that test is given `SUBSTRATE_VECTORS_CGROUP_ROOT`, which the gate and CI do not do | retain the delegated lane as a pre-release step and add no optimistic facts |
| Security | `openat2` beneath/no-link/no-mount I/O, atomic replacement, cleared/shaped environment, namespace no-egress, pids/memory+swap plus cumulatively observed CPU cgroup bounds, backend-identity-bound capability snapshots, output draining, timeout, whole-tree kill, exact capsule-byte verification, read-only `/runtime`, separate writable `/workspace`, owner-private durable state, and bounded normal/restart capsule cleanup are enforced; static-bearer TCP is explicitly development-only | implement the accepted short-lived scoped hosted trust-envelope profile and retain the inline capsule proof while defining a signed complete runtime closure separately |
| Stack integration | trust, session, event, federation, and contract-release seams accepted in umbrella ADRs 0015–0019 | keep later features behind their named phases |
| Implementation | the phase-4 raw-pipe slice has distinct durable session identity, session-native lifecycle operations, one scoped Unix-WebSocket attachment, atomic terminal/restart projection and verified execution capsules, proven by [`crates/substrate-daemon/tests`](crates/substrate-daemon/tests) — `pipe_session.rs`, `websocket.rs`, `contract_vectors.rs`. The delegated model-free harness lane with correlated hook evidence is a recorded prior observation ([Plan 04](docs/plan/04-direct-byte-plane.md)), not something this repository re-runs in its own gate | retain the raw-pipe and capsule evidence while adding only separately gated PTY, authority and release work |

## Repository facts

Each fact names the file, test or script that proves it, so a reader can re-check it rather than
trust this page.

- The Rust workspace has five crates — `substrate-wire`, `substrate-store`, `substrate-host`,
  `substrate-daemon` and the offline `substrate-contract-check` ([`Cargo.toml`](Cargo.toml),
  `[workspace] members`). 0.6.0 is the current development bundle and every earlier bundle
  directory is frozen; [`scripts/gate.sh`](scripts/gate.sh) runs the four Python bundle checkers
  plus `cargo xtask check-bundle` for 0.5.0 and 0.6.0 on every invocation, so a green gate is
  evidence that all six still hold. 0.5.0 is the first bundle whose checker is a `cargo xtask` verb
  rather than a Python script; the four frozen pairs stay Python as the reproducibility proof of the bundles they
  froze. The one recorded exception to immutability is the 2026-08-24 brand rename, which
  re-rendered every bundle in place (AGENTS.md invariant 6). No development bundle becomes a stable
  release without packaging and signing.
- **Egress is destination-bound and operator-declared.** Ordinary execution still has no egress;
  where an operator declares `--egress-aperture <name>=<host>:<port>/tcp` and the mechanism verifies
  in a throwaway sandbox at startup, a run may select it **by name** and reach that one pinned
  address. The sandbox namespace still has loopback and no other interface — the aperture is a
  listening socket inside it, served by a per-run forwarder in the run's own cgroup
  ([`crates/substrate-host/src/egress.rs`](crates/substrate-host/src/egress.rs), ADR 0013). The
  mechanism cases in `egress::tests` run wherever bubblewrap is present; the delegated lane's
  aperture cases in
  [`crates/substrate-daemon/tests/runtime_vectors.rs`](crates/substrate-daemon/tests/runtime_vectors.rs)
  are **absent** without `SUBSTRATE_VECTORS_CGROUP_ROOT`, never reported as passed.
- **A credential reaches a confined run as a sealed `memfd` and as nothing else.** Where an operator
  declares `--secret-slot <name>=<path>` and the probe proves sealing and descriptor pass-through, a
  start names the slot and the descriptor it must arrive at
  ([`crates/substrate-host/src/secrets.rs`](crates/substrate-host/src/secrets.rs), ADR 0012). The
  delegated lane proves it against the shipped binary: the confined child reads the value from the
  declared descriptor and returns a digest of it, reads back the declared seal set, holds exactly
  stdio plus its slot, and the value is in no captured argv, environment, stdout, stderr, event,
  ledger row, applied record, refusal body or daemon diagnostic; `/proc/<daemon>/fd` holds no slot
  `memfd` while the child is still running. Those cases are **absent** without
  `SUBSTRATE_VECTORS_CGROUP_ROOT`; the portable lane asserts `exec.secret-slots-unserved` instead.
- No Flux package, type or checkout is required: `flux` appears in no
  [`Cargo.lock`](Cargo.lock) package and nowhere under [`crates/`](crates), as
  [ADR 0001](adr/0001-substrate-is-standalone-and-flux-free.md) requires.
- The clean-room runner
  [`crates/substrate-daemon/tests/runtime_vectors.rs`](crates/substrate-daemon/tests/runtime_vectors.rs)
  spawns the shipped daemon binary, drives it over its Unix socket and prints its case inventory
  under `cargo test -- --nocapture`. Its portable lane proves the typed refusal without
  confinement; its delegated lane, selected by `SUBSTRATE_VECTORS_CGROUP_ROOT`, adds bounded
  exec, capacity pressure, trapped TERM, output durability, and idle-time whole-cgroup lease
  expiry.
- The Rust tests prove provisional dispatch before host mutation, full first-terminal-wins behaviour
  across signal and expiry, exact post-commit event effects, subject-local wake hints,
  restart-to-unknown without redispatch, observed-effect and store-failure recovery, real WebSocket
  limits, capped fair maintenance across reopen, lease clocks, symlink escape refusal, and strict
  minimum host limits. `cargo test --workspace --locked` is the command that counts them; this page
  does not restate the number.
- All four bundle trees classify every JSON document and meta-validate their declared Draft 2020-12
  schemas with the pinned standards validator. Semantic relations and fixed authorities are checked
  offline, and [`scripts/test_contract_json_gate.py`](scripts/test_contract_json_gate.py) holds
  seven negative tests proving unclassified JSON, invalid payloads, invalid schemas and invalid
  authority targets fail closed.
- Runtime SQLite and guarded filesystem calls use separately bounded 16-slot blocking lanes
  (`crates/substrate-host/src/lib.rs:453`, `crates/substrate-daemon/src/app/service.rs:204`);
  saturation tests prove unrelated async work remains schedulable. Snapshot GC bounds metadata,
  cascade-owned items and expiry markers while preserving expired-versus-never-found behaviour
  (`crates/substrate-store/src/tests.rs:2315`).
- Workspace cleanup advances in descriptor-relative 4,096-item batches
  (`crates/substrate-host/src/fs.rs:31`) without a total depth or item ceiling. Durable
  `destroying` blocks exec start and is automatically resumed after restart under fixed,
  subject-scoped lock stripes until the original destroy operation terminalizes
  (`crates/substrate-daemon/tests/contract_vectors.rs:1739`).
- A workspace root may be a directory the operator already owns: `validate_root_name`
  (`crates/substrate-host/src/fs.rs:1317`) requires a single non-escaping path component and no
  longer a `ws_` prefix, while `create_workspace` still mints `ws_…`. Adoption is half done —
  `crates/substrate-daemon/src/app/operations.rs:241` still gates `exec.start` on the `ws_` prefix,
  so over the socket an adopted directory can be read and written but cannot run an exec. Widening
  that predicate is wire-contract-adjacent and is not done.
- Outside its composition root the daemon names nothing from `substrate_host` except the `Driver`
  trait (`crates/substrate-host/src/lib.rs:171`) and the types that trait's signature forces on a
  caller, asserted by
  [`crates/substrate-daemon/tests/driver_port.rs`](crates/substrate-daemon/tests/driver_port.rs).
- The toolchain is pinned, not floating: [`rust-toolchain.toml`](rust-toolchain.toml) declares the
  channel, and `cargo xtask check-toolchain` fails the gate unless it, the `rust-version` in
  [`Cargo.toml`](Cargo.toml) and the [`Dockerfile`](Dockerfile) builder tag agree.
- Git, PTY, reconnect, workloads, images, volumes, endpoints, Docker and Kubernetes are absent
  rather than stubbed: `contracts/substrate-wire/0.4.0/operations.json` closes 26 operations and
  none of them is one of those. The development pipe session is the sole phase-4 byte-plane slice,
  under [ADR 0007](adr/0007-protocol-processes-use-raw-pipe-sessions.md).

## How this page is refreshed

Re-run these and take every count from their output; none of the numbers above is written by hand.

```console
git describe --tags --abbrev=0                       # the tag named in the Source row
bash scripts/gate.sh                                 # the whole bar for main, in one command
python3 scripts/check-contract-bundle.py             # 0.1.0 counts
python3 scripts/check-contract-bundle-0.2.0.py       # 0.2.0 counts
python3 scripts/check-contract-bundle-0.3.0.py       # 0.3.0 counts
python3 scripts/check-contract-bundle-0.4.0.py       # the Wire contract row
cargo test --workspace --locked -- --nocapture       # the clean-room case inventory
cargo xtask check-toolchain                          # the pinned channel
cargo xtask package-bundle 0.4.0 --out <dir>         # the OCI manifest digest
```

`cargo test --workspace --locked` is the first step of the gate and reports its own totals; the
Release row's claim that nothing publishes or signs is checked by reading
[`.github/workflows`](.github/workflows), which contains only the gate and the documentation site.

## External dependencies

Substrate has no source dependency on another b10x component. Cross-component compatibility
uses stable wire contracts and conformance fixtures, never sibling implementation-path dependencies.
