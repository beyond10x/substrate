# Repository status

**Observed:** 2026-10-29 for every row that states that date and the command it was read from, on
the integration branch `wave/adapter-io` at `55bf488` (`git rev-parse --short HEAD`), which reaches `main` in one pull
request and is released once, as the coming `0.15.0` (`approval-record:one-pr-per-wave`); a row that states 2026-10-28 was last
observed then, and a row that states no date was last observed 2026-09-15. `0.15.0` is not
tagged or released (2026-10-29: `gh release list -R beyond10x/substrate` → latest `0.14.0`).

## Current state

The scoped Git startup correction leaves Git metadata untouched when no Git sources are
configured; [Design23](docs/design/23-git-state-ownership.md) states the boundary. Its eight
disposable regressions were observed failing against the previous implementation and passing on
the correction. The complete workspace test suite also passes; the required full repository gate
is the publication bar. Configured-Git recovery, quota allocation and storage formats retain their
released behavior.

Phase 3 **lifecycle and recovery** is complete under the
[archived closure disposition](docs/reviews/archived/2026-08-14-phase-3-closure-review-disposition.md),
which records findings 1–39 closed. Phase 4 **direct byte plane** is complete
([ROADMAP.md](ROADMAP.md), [Plan 04](docs/plan/04-direct-byte-plane.md)): the model-free raw-pipe
session slice, the bounded read-only execution capsule, the pty session mode and proof-bound hosted
network attachment authority are green. Route closure happened in 0.3.0, which added the seven
pipe-session operations to 0.2.0's nineteen; 0.4.0 keeps those 26 and adds six execution-capsule
driver vectors without adding a route, 0.10.0 adds a terminal to the same 31 operations by growing a
field rather than a route family ([ADR 0019](adr/0019-pty-is-a-second-session-mode.md)), and 0.11.0
preserves those routes while adding two metrics routes plus hard writable-storage quota requests.
This is development conformance, not stable contract publication. Release `0.14.0` publishes,
keyless-signs, verifies and anonymously reads back the daemon, disposable MCP adapter and the
`0.23.0` development bundle; current source advertises the unreleased `0.24.0`, which adds the
host-process event stream (ADR 0036 S12). None of that makes the wire contract stable.

Production TLS 1.3 HTTPS/WSS transport is implemented in current source with explicit identity
files, atomic SIGHUP rotation and fail-closed hosted Identity admission. Five-minute opaque access
credentials are resolved over direct HTTPS for the exact audience and checked against each route
before durable admission. The Rust SDK now uses that listener through explicit roots, DNS identity,
per-request authority and proof-bound WSS. The node-bound Kubernetes serving profile
([ADR 0046](adr/0046-kubernetes-serves-one-node-bound-daemon-per-stable-pod-address.md), accepted
2026-10-28) is implemented as the Helm chart `deploy/helm/substrate`, listed under *Unreleased* in
[CHANGELOG.md](CHANGELOG.md); a daemon run with `--ops-listen` serves probes and metrics on a
separate operational listener
([ADR 0045](adr/0045-operational-health-and-metrics-use-a-separate-listener.md)), also
unreleased. Docker/Kubernetes drivers and a direct Firecracker driver remain unimplemented; the Kubernetes
namespace driver is decided
([ADR 0047](adr/0047-a-kubernetes-namespace-driver-serves-workspaces-on-pvcs-and-execs-as-pods.md),
accepted 2026-10-29), with its design and code in the wave after this one. They are not the `oci`
and `pod` confinement adapters that shipped in 0.12.0: an adapter serves the confinement API in
substrate-host, a driver would serve the whole wire through the daemon
([ADR 0044](adr/0044-confinement-is-a-contract-family-served-by-declared-adapters.md) decision 8).
The observed development EKS nodes expose no KVM device or microVM RuntimeClass,
so Firecracker live conformance needs a dedicated KVM-capable node pool and is absent on the
current nodes. The latest annotated tag is `0.14.0` at commit `0b9d50c`; release run
[`40558015370`](https://github.com/beyond10x/substrate/actions/runs/40558015370) built that tagged
source, keyless-signed all three images and the binaries' `SHA256SUMS`, and published the GitHub
release only after `.github/workflows/gate.yml` had concluded `success` for the exact tagged commit
in run [`40555053197`](https://github.com/beyond10x/substrate/actions/runs/40555053197)
(2026-10-28: `gh release view 0.14.0 -R beyond10x/substrate`,
`gh run list -R beyond10x/substrate --workflow release.yml`).

| Area | State | Next proof |
|---|---|---|
| Source | the standalone public repository `beyond10x/substrate` (`git remote -v`), public under atlas ADR 0003 ([CHANGELOG.md](CHANGELOG.md), 0.2.1); the latest annotated tag and published GitHub release are `0.14.0` at `0b9d50c` (2026-10-29: `git describe --tags --abbrev=0`, `gh release list -R beyond10x/substrate`); `0.15.0` is not tagged. `origin/main` is at `e08fe3d`, **119 commits after that tag**, and this page's base `wave/adapter-io` at `55bf488` is **45 commits after `origin/main`**, which it contains (2026-10-29: `git rev-list --count 0.14.0..origin/main` → 119, `git rev-list --count origin/main..HEAD` → 45, `git merge-base --is-ancestor origin/main HEAD`). What both change is listed under *Unreleased* in [CHANGELOG.md](CHANGELOG.md). From `main`: the host-process event stream and bundle `0.24.0`, the ADR 0045 operational listener, the ADR 0046 Helm chart, the RUSTSEC-2026-0414 `pulldown-cmark` update, ESS 0.75.0, the `session.not-attachable` message, lease-clock and session fixes, `pod` cleanup fixes and the `ev2` cursor fix. From the wave: piped standard input on the confinement adapters (design 32), the host execution profile name as a deployment parameter (design 33), the force-deleted Pod's policy release and the accepted ADR 0047. Live output (design 32, unit 2) is in progress and not on this branch | keep the portable-document and credential invariants enforced — `cargo xtask check-links` and `cargo xtask check-secrets` are full-gate steps |
| CI | [`.github/workflows/gate.yml`](.github/workflows/gate.yml) runs `bash scripts/gate.sh tests` and `bash scripts/gate.sh static` as two parallel jobs on push to `main`, on pull request and on dispatch, and its `Full gate` job succeeds only when both phases did, so that check's status is the gate's own (read 2026-10-29). **Ruleset `main-requires-the-full-gate` (id 23425552, no bypass actors) requires that check on `main` (`gh api repos/beyond10x/substrate/rules/branches/main` → `required_status_checks` beside `creation`, `update`, `deletion`, `non_fast_forward` and the two email patterns); the legacy `GET …/branches/main/protection` still answers 404 `Branch not protected`, because rulesets are not branch protection.** The gate is also enforced at release time: [`.github/workflows/release.yml`](.github/workflows/release.yml) reads `gate.yml`'s own recorded conclusion for the exact commit from the Actions API and refuses to release a tag without a `success`. The first green run is [33275398365](https://github.com/beyond10x/substrate/actions/runs/33275398365). The delegated lane is **absent** there, never reported as passed: a hosted runner has neither bubblewrap nor a delegated cgroup subtree. That lane is a release condition rather than a gate step: `release.yml` refuses a tag with no recorded local run of `bash scripts/delegated-lane.sh` for the tag's own commit, taken from the annotated tag's `Confinement-lane:` trailer or from the `confinement_evidence` dispatch input, and checked against the commit and the `DELEGATED_CASES` the tagged source declares. It records that run; it does not execute the lane | give the delegated lane a runner that has both prerequisites and make it a required check, which would replace the recorded-run condition with an executed one |
| Release | run [`40558015370`](https://github.com/beyond10x/substrate/actions/runs/40558015370) published and keyless-signed daemon `ghcr.io/beyond10x/b10x-substrate-daemon:0.14.0` at `sha256:a22fc8a1707ac750eb86115422e26ecfb2fde64fb3df588e9fced32ef2a52182`, disposable MCP `ghcr.io/beyond10x/b10x-substrate-mcp:0.14.0` at `sha256:c6844980db262412db962228bb263421e285d57226e7c8a3a28fbbb85d589d42`, development bundle `ghcr.io/beyond10x/b10x-substrate-wire:0.23.0` at `sha256:c2a50222da154028011a794cc93724043943581ba4ee44051c38dd084b759ff5`, and two x86_64 Linux binary archives under `SHA256SUMS` at `sha256:521435c3007898ade2a021e13f799dd5789fddc44f81cfce051d186b6c67f480`, each verifiable against the certificate identity `release.yml@refs/tags/0.14.0`; the tag records a confinement-lane run of 180 delegated cases and a quota-lane run of 7 cases, both for `0b9d50c`; the public release is [0.14.0](https://github.com/beyond10x/substrate/releases/tag/0.14.0), authored by `github-actions[bot]`; the bundle remains development, not stable (2026-10-28: `gh release view 0.14.0 -R beyond10x/substrate`) | retain write-once tags, anonymous readback and signature verification in every later release |
| Boundary | accepted: standalone, generic execution data plane, Flux-free ([ADR 0001](adr/0001-substrate-is-standalone-and-flux-free.md)) | ADR 0001's dependency half is now a gate step (`cargo xtask check-packages`); give ADRs 0002–0006 the same treatment in dependency and conformance tests |
| Wire contract | bundles 0.1.0–0.24.0 remain frozen and reproducible (`ls contracts/substrate-wire` → 24 directories; `scripts/gate.sh` runs `check-bundles 0.5.0` through `0.24.0`); current source advertises 0.24.0 at inner `bundle.json` digest `1570cc116e9b38c105607fefde705416b0ac2e199eb157a004c1e9b63d205d34` (`ADVERTISED_CONTRACT_BUNDLE_SHA256` in `crates/substrate-wire/src/lib.rs`, equal to `sha256sum contracts/substrate-wire/0.24.0/bundle.json`), which adds the route `GET /v1/processes/{process_id}/events` and preserves the 44 routes of 0.23.0 (`contracts/substrate-wire/0.24.0/compatibility.json`; `operations.json` lists 45). 0.24.0 is unreleased; release 0.14.0 published 0.23.0 (2026-10-29, [CHANGELOG.md](CHANGELOG.md)) | notify consumers of the source promotion and do not describe either development bundle as stable |
| Drivers | Linux host driver implemented; absent delegation keeps exec, resource-accounting and project-quota facts absent and answers named `unserved` refusals rather than degrading; the delegated execution lane runs only when given `SUBSTRATE_VECTORS_CGROUP_ROOT`, and project quotas require a separately provisioned filesystem and exclusive ID range. Their seven `real_quota_*` cases are `#[ignore]`d (`grep -c 'fn real_quota_' crates/substrate-host/src/git/quota_tests.rs` → 7) and run only through [`cargo xtask quota-lane`](xtask/src/quota_lane.rs), which refuses by name when `SUBSTRATE_TEST_QUOTA_ROOT` or `SUBSTRATE_TEST_PROJECT_QUOTA_IDS` is missing or the backing mount carries no project quotas; `cargo xtask quota-fixture run` provisions that fixture in a container and runs the lane. `scripts/delegated-lane.sh` closes with `cargo xtask quota-lane --inventory`, because the cases are outside `DELEGATED_CASES`. A release requires a recorded `Quota-lane:` run beside the `Confinement-lane:` run; release 0.14.0 records `0b9d50c… 7 2026-10-27T18:05:08Z` (2026-10-28: `gh release view 0.14.0 -R beyond10x/substrate`) | give both lanes a runner with their prerequisites and make them required checks, which would replace the recorded-run conditions with executed ones |
| Confinement adapters | the confinement contract family of [ADR 0044](adr/0044-confinement-is-a-contract-family-served-by-declared-adapters.md) (atlas ADR 0109) is substrate-host's API, not the wire: the `local`, `linux`, `macos`, `systemd`, `oci` and `pod` adapters shipped in 0.12.0, and 0.13.0 and 0.14.0 extended them (2026-10-28, [CHANGELOG.md](CHANGELOG.md)); the contract is the ESS domain `substrate.confinement` in [`spec/confinement/`](spec/confinement). This wave adds piped standard input ([design 32](docs/design/32-adapter-stdin-and-live-output.md), fact `stdin_pipe`): `local`, `linux`, `macos` and `systemd` around either prove it, `oci` only on the runtime flavor, server version and client version its lane pinned, (Docker, 29.7.2, 29.7.2) (`OCI_STDIN_PINNED` in `crates/substrate-host/src/confinement/oci.rs`), and `pod` refuses it by the fact's name. The confinement suite holds 62 scenarios, 57 authored, all 62 passed against the interpreted model (2026-10-29: `cargo xtask check-ess`). Live output (fact `output_live`) is in progress; `story:adapter-stdin-pipe` and `story:adapter-live-output` are `active` (2026-10-29: `aep plan artifact list`) | finish live output; publish adapters and facts on the wire only through a successor bundle, as ADR 0044's consequences state |
| Security | `openat2` beneath/no-link/no-mount I/O, atomic replacement, cleared/shaped environment, namespace no-egress, pids/memory+swap plus cumulatively observed CPU cgroup bounds, backend-identity-bound capability snapshots, output draining, timeout, whole-tree kill, exact capsule-byte verification, read-only `/runtime`, separate writable `/workspace`, owner-private durable state, and bounded normal/restart capsule cleanup are enforced; static-bearer TCP is loopback-only and explicitly development-only; production network transport is TLS 1.3 with owner-safe identity material, atomic rotation, online exact-audience Identity admission and one-use key/channel-bound WSS attachment authority | retain negative hosted-auth and session-authority conformance without weakening the inline capsule proof |
| Stack integration | trust, session, event, federation, and contract-release seams accepted in umbrella ADRs 0015–0019; the planning store now carries dependency-gated remote-serving, Kubernetes, Docker and Firecracker tracks. `epic:remote-serving`, `epic:kubernetes-deployment-and-driver` and `epic:confinement-contract-family` are `active`, `epic:firecracker-driver` is `proposed`; [design 31](docs/design/31-remote-clean-room-conformance-runner.md) (remote clean-room runner) is accepted for implementation and its story `story:remote-clean-room-conformance` is `proposed` (2026-10-29: `aep plan artifact list`) | accept each track's design/ADR and environment gate before capability code; keep product policy outside Substrate |
| Kubernetes serving | the node-bound profile of [ADR 0046](adr/0046-kubernetes-serves-one-node-bound-daemon-per-stable-pod-address.md) is the Helm chart [`deploy/helm/substrate`](deploy/helm/substrate), unreleased; `cargo xtask chart-lane static` and `cargo xtask chart-lane cluster` hold it, and the cluster lane on kind reports the four execution-enabled scenarios absent ([CHANGELOG.md](CHANGELOG.md) *Unreleased*). `story:node-bound-kubernetes-serving-profile` is `implemented`, `story:chart-lane-local-execution-record` (K9 C) `draft`; [ADR 0047](adr/0047-a-kubernetes-namespace-driver-serves-workspaces-on-pvcs-and-execs-as-pods.md) (namespace driver) is accepted, dated 2026-10-29, and its two stories are `proposed` (2026-10-29: `aep plan artifact list`, the ADR's frontmatter) | record the execution-enabled scenarios on a local lane; accept the namespace driver's implementation design before its code |
| Executable specification | ESS 0.75.0 (`spec/ess-inputs.yaml`, `ESS_VERSION` in `.github/workflows/gate.yml`); the accepted-operation ledger suite holds 14 synthesized scenarios, host processes 92 synthesized with 27 refusals (113 executed, all passed), workspaces 23 (23 passed) and confinement 62 (62 passed); the committed suites and CLI contract are current (2026-10-29: `cargo xtask check-ess`) | never describe these bounded projections as full service conformance |
| Implementation | the phase-4 raw-pipe slice has distinct durable session identity, session-native lifecycle operations, one scoped Unix-WebSocket attachment, atomic terminal/restart projection and verified execution capsules, proven by [`crates/substrate-daemon/tests`](crates/substrate-daemon/tests) — `pipe_session.rs`, `websocket.rs`, `contract_vectors.rs`. A pty is a second **mode** on that same slice, with the controlling terminal acquired inside the sandbox after bubblewrap's `setsid`; the delegated lane of [`runtime_vectors.rs`](crates/substrate-daemon/tests/runtime_vectors.rs) drives an interactive shell through one — echo, a resize the child reads back with `TIOCGWINSZ`, and whole-tree cleanup on attachment loss — and the portable lane proves `session.pty-unserved`. The delegated model-free harness lane with correlated hook evidence is a recorded prior observation ([Plan 04](docs/plan/04-direct-byte-plane.md)), not something this repository re-runs in its own gate | retain the raw-pipe, capsule and terminal evidence while adding only separately gated authority and release work |
| Rust SDK | current source verifies the promoted 0.24.0 name and inner digest (`CONTRACT` and `CONTRACT_SHA256` in `crates/b10x-substrate-sdk/src/model.rs`, which take `ADVERTISED_CONTRACT_BUNDLE` and its digest from `substrate-wire`, read 2026-10-29) before serving an operation; sends session requests only to `/v1/sessions`; exposes typed guarded-file, Git-baseline/change-set, PTY, metrics, snapshot, event, host-process event stream (`Client::process_events`, unreleased) and bounded-output APIs; preserves optional capability facts; accepts caller ids on every mutation; serializes SDK observations; supervises an external or linked daemon; and connects remotely over explicit-root TLS 1.3 HTTPS/WSS with per-request Identity authority and fresh proof-bound session authority | retain local/remote parity and distribute the SDK from source rather than crates.io |
| MCP test surface | current source provides a private SDK-only `substrate-mcp` stdio binary with bounded JSONL, a closed tool/resource vocabulary, exact refusal projection, caller operation ids, per-instance authority tracking and ordered cleanup; portable and delegated shipped-binary lanes plus a manual real-Codex run prove the intended harness surface; release 0.14.0 publishes its separately signed image at `sha256:c6844980db262412db962228bb263421e285d57226e7c8a3a28fbbb85d589d42` (2026-10-28: `gh release view 0.14.0 -R beyond10x/substrate`) | keep HTTP, OAuth and production ingress absent |

## Repository facts

Each fact names the file, test or script that proves it, so a reader can re-check it rather than
trust this page.

- The Rust workspace has five source-distributed runtime packages — `b10x-substrate-wire`,
  `b10x-substrate-store`, `b10x-substrate-host`, `b10x-substrate-daemon` and
  `b10x-substrate-sdk` — plus non-publishable MCP and tooling packages
  ([`Cargo.toml`](Cargo.toml), `[workspace] members`). Every member sets `publish = false`;
  `cargo xtask check-packages` checks that closed posture plus exact internal release versions,
  package names, SPDX metadata, READMEs and public documentation targets, and refuses a Flux
  dependency (invariant 1) or a `beyond10x` Git dependency (invariant 2) in any dependency table of
  any manifest — its `a_flux_dependency_is_refused` and `a_beyond10x_git_dependency_is_refused`
  tests prove each refusal fires. Consumers use a local
  path or exact Git revision; tagged releases publish GitHub and GHCR artifacts, not crates.io.
  0.24.0 is the current development bundle and every earlier bundle
  directory is frozen; [`scripts/gate.sh`](scripts/gate.sh) runs the four Python bundle checkers
  plus bounded-parallel `cargo xtask check-bundles` for 0.5.0 through 0.24.0 on every invocation, so a green
  gate is evidence that all twenty-four still hold. 0.5.0 is the first bundle whose checker is a `cargo xtask` verb
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
  minimum host limits. `cargo test --workspace --release --locked` is the command that counts them; this page
  does not restate the number.
- All twenty-four bundle trees classify every JSON document and meta-validate their declared Draft 2020-12
  schemas with the pinned standards validator. Semantic relations and fixed authorities are checked
  offline, and `cargo xtask check-json` ([`xtask/src/json.rs`](xtask/src/json.rs)) carries the
  negative tests proving unclassified JSON, invalid payloads, invalid schemas and invalid
  authority targets fail closed.
- Runtime SQLite and guarded filesystem calls use separately bounded 16-slot blocking lanes
  (`blocking_slots` in `crates/substrate-host/src/lib.rs`, `blocking_store_slots` in
  `crates/substrate-daemon/src/app/service.rs`, both `Semaphore::new(16)`);
  saturation tests prove unrelated async work remains schedulable. Snapshot GC bounds metadata,
  cascade-owned items and expiry markers while preserving expired-versus-never-found behaviour
  (`snapshot_gc_bounds_materialized_rows_and_preserves_expired_posture` in
  `crates/substrate-store/src/tests.rs`).
- Workspace cleanup advances in descriptor-relative 4,096-item batches
  (`DESTROY_BATCH_ITEMS` in `crates/substrate-host/src/fs.rs`) without a total depth or item
  ceiling. Durable
  `destroying` blocks exec start and is automatically resumed after restart under fixed,
  subject-scoped lock stripes until the original destroy operation terminalizes
  (`destroying_workspace_refuses_exec_after_restart_and_replays_stably` and
  `crash_after_destroying_resumes_cleanup_and_terminalizes_original_operation` in
  `crates/substrate-daemon/tests/contract_vectors.rs`).
- A workspace root may be a directory the operator already owns, served after an explicit
  `workspace.adopt` (ADR 0035, bundle `0.19.0`): the adopted row gets a minted `ws_…` id and
  records the directory's `root_name` and `origin: adopted`, so `exec.start` keeps its `ws_`
  predicate and addresses the adopted workspace by that id. The clean-room runner
  (`crates/substrate-daemon/tests/runtime_vectors.rs`, `check_adopted_workspace`) adopts a
  directory without the `ws_` prefix through the shipped daemon, refuses out-of-set names before
  dispatch, and destroys it leaving its files; on the delegated lane it runs an exec in it and
  runs the isolation cases (`check_confined_isolation`) against it as well as against a created
  workspace.
- Outside its composition root the daemon names nothing from `substrate_host` except the `Driver`
  trait (`pub trait Driver` in `crates/substrate-host/src/lib.rs`) and the types that trait's
  signature forces on a
  caller, asserted by
  [`crates/substrate-daemon/tests/driver_port.rs`](crates/substrate-daemon/tests/driver_port.rs).
- The toolchain is pinned, not floating: [`rust-toolchain.toml`](rust-toolchain.toml) declares the
  channel, and `cargo xtask check-toolchain` fails the gate unless it, the `rust-version` in
  [`Cargo.toml`](Cargo.toml) and the [`Dockerfile`](Dockerfile) builder tag agree.
- Workloads, images, volumes, endpoints and the Docker, Kubernetes and Firecracker drivers are
  absent from the wire rather than stubbed: `contracts/substrate-wire/0.4.0/operations.json` closes
  26 operations and none of them is one of those — and `contracts/substrate-wire/0.10.0/operations.json` closes
  31, preserving 0.9.0's v1 and v2 routes because a terminal arrived as a `mode` field rather than as a route family;
  `contracts/substrate-wire/0.24.0/operations.json` closes 45 and none of them is one of those
  either (2026-10-29). Git workspace sources are no longer absent: 0.16.0 added the conditional
  `workspace.git` capability and the two Git observation routes. The
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
cargo xtask check-bundle 0.24.0                     # the current Wire contract row
cargo xtask check-ess                                # the Executable specification row's counts
aep plan artifact list                               # every artifact status named above
cargo test --workspace --release --locked -- --nocapture       # the clean-room case inventory
cargo xtask check-toolchain                          # the pinned channel
cargo xtask package-bundle 0.24.0 --out <dir>        # the current OCI manifest digest
```

`cargo test --workspace --release --locked` is the first step of the gate and reports its own totals. The
Release row's repository-controlled claims are checked by
[`xtask/tests/release_workflow.rs`](xtask/tests/release_workflow.rs): the current bundle pin, exact
OCI-layout copy, write-once tag checks, digest-signing and verification order, development status
and protected-main changelog route all fail closed offline. Actual GHCR and Sigstore results remain
release-time observations and are not claimed before an eligible tag runs.

## External dependencies

Substrate has no source dependency on another b10x component. Cross-component compatibility
uses stable wire contracts and conformance fixtures, never sibling implementation-path dependencies.
