# Repository status

**Observed:** 2026-09-01

## Current state

Phase 3 **lifecycle and recovery** is complete under the
[archived closure disposition](docs/reviews/archived/2026-08-14-phase-3-closure-review-disposition.md),
which records findings 1–39 closed. Phase 4 **direct byte plane** is in progress
([ROADMAP.md](ROADMAP.md), [Plan 04](docs/plan/04-direct-byte-plane.md)): the model-free raw-pipe
session slice, the bounded read-only execution capsule and the pty session mode are green, and
network session authority remains absent. Route closure happened in 0.3.0, which added the seven
pipe-session operations to 0.2.0's nineteen; 0.4.0 keeps those 26 and adds six execution-capsule
driver vectors without adding a route, 0.10.0 adds a terminal to the same 31 operations by growing a
field rather than a route family ([ADR 0019](adr/0019-pty-is-a-second-session-mode.md)), and 0.11.0
preserves those routes while adding two metrics routes plus hard writable-storage quota requests.
This is development conformance, not stable contract publication: daemon image `0.3.1` is
published, signed and digest-pinned, but no contract bundle is published as stable.

Remote HTTPS/WSS serving, hosted trust-envelope admission, remote SDK transport, a node-bound
Kubernetes serving profile, Docker/Kubernetes drivers and a direct Firecracker driver are proposed,
not implemented. The observed development EKS nodes expose no KVM device or microVM RuntimeClass,
so Firecracker live conformance needs a dedicated KVM-capable node pool and is absent on the
current nodes. Substrate `0.4.0` is merged and annotated at commit `31340a6`; it carries ADR 0023,
`substrate-wire/0.12.0`, scoped workspace writes and broader SDK parity. Its GitHub release exists,
but no release workflow run or image digest is recorded for the tag, so `0.3.1` remains the latest
verified daemon image.

| Area | State | Next proof |
|---|---|---|
| Source | the standalone public repository `beyond10x/substrate` (`git remote -v`), public under atlas ADR 0003 ([CHANGELOG.md](CHANGELOG.md), 0.2.1); the latest published annotated tag is `0.4.0` | keep the portable-document and credential invariants enforced — `cargo xtask check-links` and `cargo xtask check-secrets` are full-gate steps |
| CI | [`.github/workflows/gate.yml`](.github/workflows/gate.yml) runs `bash scripts/gate.sh` bare on push to `main`, on pull request and on dispatch, so the job's status is the gate's own; branch protection on `main` requires the `Full gate` check; the first green run is [33275398365](https://github.com/beyond10x/substrate/actions/runs/33275398365). The delegated lane is **absent** there, never reported as passed: a hosted runner has neither bubblewrap nor a delegated cgroup subtree | either give the delegated lane a runner that has both, or keep it the named local pre-release step it is today |
| Release | [`Dockerfile`](Dockerfile) builds the daemon and `cargo xtask package-bundle <version> --out <dir>` emits a deterministic OCI image layout from a frozen bundle (0.4.0 → manifest `sha256:3758e80bc39f1eb03b15c69410608c9ef1d2ba8095c7e707c6988dbb5894ab00`); daemon image `0.3.1` is published, anonymously pullable, keyless-signed and verified at `sha256:48068ba167487fa84fe77e0acf7f8e3f556060f4d220a074d9b9f60f9af71ebf`; [`.github/workflows/release.yml`](.github/workflows/release.yml) now pins development bundle 0.12.0, copies the exact layout to its write-once GHCR tag, verifies the remote digest and development annotation, then keyless-signs and verifies it before announcement. No contract-bundle tag has run yet | on the next eligible tag, observe the GHCR digest/signature and land the workflow-emitted changelog stanza through a fully gated bot pull request |
| Boundary | accepted: standalone, generic execution data plane, Flux-free ([ADR 0001](adr/0001-substrate-is-standalone-and-flux-free.md)) | enforce ADRs 0001–0006 in dependency and conformance tests |
| Wire contract | bundles 0.1.0–0.11.0 remain frozen and reproducible; 0.12.0 is the current development successor, preserves all 33 predecessor routes and adds exact read-only/scoped workspace access plus its capability and applied observation | package, sign and digest-pin a complete runtime closure and a stable release without changing development authority implicitly |
| Drivers | Linux host driver implemented; absent delegation keeps exec, resource-accounting and project-quota facts absent and answers named `unserved` refusals rather than degrading; the delegated execution lane runs only when given `SUBSTRATE_VECTORS_CGROUP_ROOT`, and project quotas require a separately provisioned filesystem and exclusive ID range | retain the delegated lane as a pre-release step and add no optimistic facts |
| Security | `openat2` beneath/no-link/no-mount I/O, atomic replacement, cleared/shaped environment, namespace no-egress, pids/memory+swap plus cumulatively observed CPU cgroup bounds, backend-identity-bound capability snapshots, output draining, timeout, whole-tree kill, exact capsule-byte verification, read-only `/runtime`, separate writable `/workspace`, owner-private durable state, and bounded normal/restart capsule cleanup are enforced; static-bearer TCP is explicitly development-only | implement the accepted short-lived scoped hosted trust-envelope profile and retain the inline capsule proof while defining a signed complete runtime closure separately |
| Stack integration | trust, session, event, federation, and contract-release seams accepted in umbrella ADRs 0015–0019; the planning store now carries dependency-gated remote-serving, Kubernetes, Docker and Firecracker tracks | accept each track's design/ADR and environment gate before capability code; keep product policy outside Substrate |
| Implementation | the phase-4 raw-pipe slice has distinct durable session identity, session-native lifecycle operations, one scoped Unix-WebSocket attachment, atomic terminal/restart projection and verified execution capsules, proven by [`crates/substrate-daemon/tests`](crates/substrate-daemon/tests) — `pipe_session.rs`, `websocket.rs`, `contract_vectors.rs`. A pty is a second **mode** on that same slice, with the controlling terminal acquired inside the sandbox after bubblewrap's `setsid`; the delegated lane of [`runtime_vectors.rs`](crates/substrate-daemon/tests/runtime_vectors.rs) drives an interactive shell through one — echo, a resize the child reads back with `TIOCGWINSZ`, and whole-tree cleanup on attachment loss — and the portable lane proves `session.pty-unserved`. The delegated model-free harness lane with correlated hook evidence is a recorded prior observation ([Plan 04](docs/plan/04-direct-byte-plane.md)), not something this repository re-runs in its own gate | retain the raw-pipe, capsule and terminal evidence while adding only separately gated authority and release work |
| Rust SDK | `b10x-substrate-sdk` verifies the advertised 0.4.0 contract, exposes typed workspace, file, exec, raw-pipe, event and operation handles—including workspace access, network, mounts, scratch, secret slots, capsules and exact usage—and supervises either an external daemon binary or a linked current-executable re-exec as a separate child | publish the owner-released package chain manually from a fully gated annotated tag, then close the remaining parity against the separately promoted contract |

## Repository facts

Each fact names the file, test or script that proves it, so a reader can re-check it rather than
trust this page.

- The Rust workspace has five approved runtime registry packages — `b10x-substrate-wire`,
  `b10x-substrate-store`, `b10x-substrate-host`, `b10x-substrate-daemon` and
  `b10x-substrate-sdk` — plus the private offline `substrate-contract-check` and `xtask`
  ([`Cargo.toml`](Cargo.toml), `[workspace] members`). `cargo xtask check-packages` refuses any
  other publishable member and checks exact internal release versions and archive contents.
  0.12.0 is the current development bundle and every earlier bundle
  directory is frozen; [`scripts/gate.sh`](scripts/gate.sh) runs the four Python bundle checkers
  plus `cargo xtask check-bundle` for 0.5.0 through 0.12.0 on every invocation, so a green
  gate is evidence that all twelve still hold. 0.5.0 is the first bundle whose checker is a `cargo xtask` verb
  rather than a Python script; the four frozen pairs stay Python as the reproducibility proof of the bundles they
  froze. The one recorded exception to immutability is the 2026-08-24 brand rename, which
  re-rendered every bundle in place (AGENTS.md invariant 6). No development bundle becomes a stable
  release merely through packaging, signing or OCI publication; atlas ADR 0019 governs stability.
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
- **A declared aperture can carry a byte ceiling, and it stops the bytes.**
  `--egress-aperture <name>=<host>:<port>/tcp/max=<size>` bounds `to_destination + from_destination`
  for one run; the relay stops relaying at the ceiling and the parent's 1 ms supervision loop ends
  the run and names the refusal `exhausted` / `exec.aperture-byte-limit` on the exec observation
  ([`crates/substrate-host/src/egress.rs`](crates/substrate-host/src/egress.rs),
  [`crates/substrate-host/src/process.rs`](crates/substrate-host/src/process.rs), ADR 0014). An
  aperture declared without the term passes what it always passed, and a request that carries a
  ceiling is refused `exec.aperture-ceiling-in-request`. The mid-run cases are in the delegated
  lane and are **absent** without `SUBSTRATE_VECTORS_CGROUP_ROOT`.
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
- All twelve bundle trees classify every JSON document and meta-validate their declared Draft 2020-12
  schemas with the pinned standards validator. Semantic relations and fixed authorities are checked
  offline, and `cargo xtask check-json` ([`xtask/src/json.rs`](xtask/src/json.rs)) carries the
  negative tests proving unclassified JSON, invalid payloads, invalid schemas and invalid
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
- Git, reconnect, workloads, images, volumes, endpoints, production remote TLS/auth, Docker,
  Kubernetes and Firecracker are absent
  rather than stubbed: `contracts/substrate-wire/0.4.0/operations.json` closes 26 operations and
  none of them is one of those — and `contracts/substrate-wire/0.10.0/operations.json` closes
  31, preserving 0.9.0's v1 and v2 routes because a terminal arrived as a `mode` field rather than as a route family. The
  development session is the sole phase-4 byte-plane slice, in two modes: raw pipes under
  [ADR 0007](adr/0007-protocol-processes-use-raw-pipe-sessions.md) and a terminal under
  [ADR 0019](adr/0019-pty-is-a-second-session-mode.md).

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

`cargo test --workspace --locked` is the first step of the gate and reports its own totals. The
Release row's repository-controlled claims are checked by
[`xtask/tests/release_workflow.rs`](xtask/tests/release_workflow.rs): the current bundle pin, exact
OCI-layout copy, write-once tag checks, digest-signing and verification order, development status
and protected-main changelog route all fail closed offline. Actual GHCR and Sigstore results remain
release-time observations and are not claimed before an eligible tag runs.

## External dependencies

Substrate has no source dependency on another b10x component. Cross-component compatibility
uses stable wire contracts and conformance fixtures, never sibling implementation-path dependencies.
