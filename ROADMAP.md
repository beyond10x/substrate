# Roadmap

Serves **O1** of `atlas/ROADMAP.md`, the collection's objectives; this page orders outcomes inside
this repository. Work-item status and dependency edges live only in `.engineering/planning/` and
are read with `aep plan artifact board` and `aep plan artifact graph`.

The foundation phases are ordered. After lifecycle and recovery, delivery proceeds as
dependency-gated tracks: a backend does not wait for an unrelated backend, but it cannot cross its
own contract, authority, conformance or environment gate. Starting a track never permits a weaker
capability fact or an unrecorded driver side effect.

## Foundation

| Phase | Outcome | Exit criterion | State |
|---:|---|---|---|
| 0 | Private design component | `foundation/substrate` in the private monorepo, bot-authored `main`, portable docs | complete |
| 1 | Design closure | contract questions are decided or explicitly deferred; canonical schema/translation, trust-domain, destination-security, capability-snapshot, and driver guarantees are reviewable | complete |
| 2 | Minimum host slice | one confined workspace, bounded argv-only exec, observed result, named refusals, and machine facts | complete — portable and delegated-host lanes green |
| 3 | Lifecycle and recovery | operation ledger, events, leases, cancellation, and unanswered-outcome reconciliation | complete — all 39 closure findings resolved; portable and delegated lanes green |
| 4 | Direct byte plane | raw-pipe and PTY sessions have bounded channel authority over local and production network transports | complete — local raw-pipe, capsule and PTY slices are green; hosted WSS attachments use one-use proof-bound authority over TLS and fail closed |

## Dependency-gated delivery tracks

| Track | Outcome | Opens when | Exit criterion | State |
|---|---|---|---|---|
| Contract distribution and SDK | consumers pin a signed bundle and use the contract the daemon actually advertises through a source-distributed Rust SDK | current hardening and bundle-publication evidence are complete | signed bundle publication, coordinated advertised-header migration, SDK parity and exact-revision source consumption are proven | implemented — the store holds no active artifact for this track: `epic:rust-sdk` and `epic:release-hardening` are `implemented`, and `story:rust-sdk-publication` is `archived` under its recorded disposition that source distribution replaces registry publication. Reopening it means opening a story first |
| Confinement contract family | an embedder confines one command through `substrate_host::confinement`: it opens an adapter, reads the families and facts the adapter proved, and admits a profile or gets a named refusal ([ADR 0044](adr/0044-confinement-is-a-contract-family-served-by-declared-adapters.md), atlas ADR 0109) | the ESS domain `substrate.confinement` is specified before adapter code | each adapter's scenarios in `spec/confinement/` pass on its own lane | active — the `local`, `linux`, `macos`, `systemd`, `oci` and `pod` adapters shipped in 0.12.0, and 0.13.0 and 0.14.0 extended them; 0.15.0 added piped standard input and live output (design 32, facts `stdin_pipe` and `output_live`; [CHANGELOG.md](CHANGELOG.md)). In the store `epic:confinement-contract-family` is `active`, `story:adapter-stdin-pipe`, `story:adapter-live-output` and `story:pod-force-deleted-keeps-its-policy` are `implemented`, and `story:confinement-adapter-on-the-wire` and `story:pod-orphaned-policy-is-swept` are `draft` (`aep plan artifact list`, 2026-10-29) |
| Remote serving | consumers address one Substrate instance over HTTPS/WSS with Identity-scoped authority | the promoted contract surface is selected; TLS and trust-envelope designs are accepted before code | remote SDK and clean-room conformance prove durable recovery, event gaps, session authority and negative TLS/auth cases | active — production TLS, hosted Identity admission, proof-bound session authority and remote SDK transport are implemented; the clean-room runner of [design 31](docs/design/31-remote-clean-room-conformance-runner.md) is implemented as `crates/b10x-substrate-conformance` (wave 2026-10-30-remote-conformance) and runs as the namespace lane's step 4 and in the hosted chart lane |
| Kubernetes deployment and driver | Kubernetes provides a node-bound host profile and a separately gated namespace workspace/exec driver | remote listener/auth prerequisites hold; namespace work also needs its RBAC and ownership gate | stable per-instance addressing and storage are proven without round-robin mutations; PVC/pod execution passes shared conformance | active — the node-bound serving profile of [ADR 0046](adr/0046-kubernetes-serves-one-node-bound-daemon-per-stable-pod-address.md) (accepted 2026-10-28) shipped in 0.15.0 as the Helm chart `deploy/helm/substrate`, published at `oci://ghcr.io/beyond10x/charts/b10x-substrate`; the hosted chart lane reports its four execution-enabled scenarios absent on kind, and 0.16.1 adds `cargo xtask chart-lane cluster --node-profiles`, whose `Chart-lane:` record a release now requires; `story:chart-lane-local-execution-record` (K9 C) is `implemented`. The namespace driver of [ADR 0047](adr/0047-a-kubernetes-namespace-driver-serves-workspaces-on-pvcs-and-execs-as-pods.md) shipped in 0.16.1 to [design 34](docs/design/34-kubernetes-namespace-driver.md) (`approval-record:design-34-accepted`) as a development profile, listed under 0.16.0 and 0.16.1 in [CHANGELOG.md](CHANGELOG.md): it serves empty workspaces; up to 0.17.0 it refused every exec start `exec.sandbox-unavailable`, and from 0.18.0 ([ADR 0048](adr/0048-the-wire-states-what-a-confinement-adapter-proves-never-its-name.md)) it admits an exec on the confinement its probe observed, so only where the network plugin denies egress inside the Pod; a timeout never proves no egress (D333), and kind's plugin, which drops silently, keeps exec refused as in 0.17.0; `story:namespace-lane-runs-exec-on-an-enforcing-network-plugin` is next, and `story:namespace-probe-repeats-and-sticks-on-egress` is proposed. Its lane's step 4 runs that runner against the `--driver kubernetes` daemon. `story:kubernetes-namespace-driver-entry-gate` is `implemented` and `story:kubernetes-namespace-workspace-and-exec-slice` is `active` (`aep plan artifact list`, 2026-10-29) |
| Docker driver | the same contract serves container-backed workspace/exec and immutable image-backed workloads | phase-4 authority and shared remote conformance are green; the Docker root-equivalence gate is accepted | closed container specs, durable Docker dispatch, immutable image identity and restart cleanup pass the shared driver journey | proposed |
| Firecracker driver | one bounded execution runs in a fresh directly managed microVM | the direct-driver design, immutable boot-artifact gate and a dedicated KVM-capable host are available | jailer/KVM probes and the microVM workspace/exec slice pass live conformance; unsupported hosts report absence | proposed — current dev nodes provide no KVM surface |
| Stack adoption | independent consumers prove the public contract rather than sibling implementation paths | a consumer records adoption in its own repository | at least one product execution path uses the remote SDK and published contract evidence | externally gated |
| Hosted composition | identity, placement and product services operate Substrate without moving their policy into it | remote conformance and an external adoption record are complete | production deployment evidence shows the execution data plane remains standalone and policy-free | pending |

## Definition of done

Operator decision, 2026-10-29 (`approval-record:definition-of-done`). Substrate is done when all
four hold:

1. **Remote serving** meets its exit criterion: clean-room conformance passes against a remote
   instance.
2. **Kubernetes deployment and driver** meets its exit criterion: the node-bound serving profile
   of ADR 0046 and the namespace workspace/exec driver both pass shared conformance.
3. **Stack adoption** meets its exit criterion: one product execution path uses the remote SDK and
   published contract evidence.
4. The O1 evidence in `atlas/ROADMAP.md` is recorded against those three.

Follow-up tracks, not part of done: **Firecracker driver** (needs a KVM-capable host), **Docker
driver** and **Hosted composition**.

The Kubernetes and Docker driver tracks are not the `pod` and `oci` adapters. An adapter serves the
confinement API in substrate-host to an embedder; a driver serves the whole wire through the daemon
(ADR 0044 decision 8).

Consumers today, each pin read from its manifest on the default branch on 2026-10-29
(`gh api repos/beyond10x/<repo>/contents/<path>`):

| Consumer | Manifest @ commit | Substrate crates | Pin |
|---|---|---|---|
| harness | `crates/harness-substrate/Cargo.toml` @ `3eca074` | `b10x-substrate-host`, `b10x-substrate-wire` | rev `0b9d50c` (tag `0.14.0`) |
| loom | `crates/loom-intake-slice/Cargo.toml` @ `b492afe` | `b10x-substrate-host`, `b10x-substrate-wire` | rev `0b9d50c` (tag `0.14.0`) |
| takt | `Cargo.toml` @ `26ed4ac` | `b10x-substrate-host` | tag `0.14.0` |
| mantle | `crates/mantle/Cargo.toml` @ `fc141b9` | `b10x-substrate-sdk` | rev `65304ed` (tag `0.7.10`) |

The tagged Substrate releases are software releases, not the similarly numbered historical contract
bundles. The current development bundle is `substrate-wire/0.26.0` (`ADVERTISED_CONTRACT_BUNDLE` in
`crates/substrate-wire/src/lib.rs`), which no release publishes yet; the `0.16.1` and `0.17.0`
release workflows pin `0.25.0`, and release `0.15.0` published `0.24.0`. None is stable.
Kubernetes, Docker and
Firecracker requests expose property-based capability facts; clients never branch on which driver
answered. Fleet scheduling, billing, product quotas, connector semantics and agent loops remain
outside Substrate.
