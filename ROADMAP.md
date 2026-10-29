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
| Confinement contract family | an embedder confines one command through `substrate_host::confinement`: it opens an adapter, reads the families and facts the adapter proved, and admits a profile or gets a named refusal ([ADR 0044](adr/0044-confinement-is-a-contract-family-served-by-declared-adapters.md), atlas ADR 0109) | the ESS domain `substrate.confinement` is specified before adapter code | each adapter's scenarios in `spec/confinement/` pass on its own lane | active — the `local`, `linux`, `macos`, `systemd`, `oci` and `pod` adapters shipped in 0.12.0, and 0.13.0 and 0.14.0 extended them; piped standard input (design 32, fact `stdin_pipe`) is merged for 0.15.0 and listed under *Unreleased* ([CHANGELOG.md](CHANGELOG.md)); live output is in progress. In the store `epic:confinement-contract-family` is `active`, with `story:adapter-stdin-pipe` and `story:adapter-live-output` `active` and `story:confinement-adapter-on-the-wire`, `story:pod-force-deleted-keeps-its-policy` and `story:pod-orphaned-policy-is-swept` `draft` (`aep plan artifact list`, 2026-10-29) |
| Remote serving | consumers address one Substrate instance over HTTPS/WSS with Identity-scoped authority | the promoted contract surface is selected; TLS and trust-envelope designs are accepted before code | remote SDK and clean-room conformance prove durable recovery, event gaps, session authority and negative TLS/auth cases | active — production TLS, hosted Identity admission, proof-bound session authority and remote SDK transport are implemented; independent clean-room conformance remains: [design 31](docs/design/31-remote-clean-room-conformance-runner.md) for its runner is accepted for implementation (2026-10-28), and `story:remote-clean-room-conformance` is `proposed` (`aep plan artifact list`, 2026-10-29) |
| Kubernetes deployment and driver | Kubernetes provides a node-bound host profile and a separately gated namespace workspace/exec driver | remote listener/auth prerequisites hold; namespace work also needs its RBAC and ownership gate | stable per-instance addressing and storage are proven without round-robin mutations; PVC/pod execution passes shared conformance | active — the node-bound serving profile of [ADR 0046](adr/0046-kubernetes-serves-one-node-bound-daemon-per-stable-pod-address.md) (accepted 2026-10-28) is implemented on `main` as the Helm chart `deploy/helm/substrate`, unreleased and listed under *Unreleased* in [CHANGELOG.md](CHANGELOG.md); `cargo xtask chart-lane cluster` reports its four execution-enabled scenarios absent on kind, and `story:chart-lane-local-execution-record` (K9 C) is `draft`. The namespace driver is decided: [ADR 0047](adr/0047-a-kubernetes-namespace-driver-serves-workspaces-on-pvcs-and-execs-as-pods.md) is accepted (2026-10-29, `approval-record:kubernetes-namespace-driver-accepted`); its implementation design and code come in the wave after `wave/adapter-io`, and its two stories are `proposed` (`aep plan artifact list`, 2026-10-29) |
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
bundles. The current development bundle is `substrate-wire/0.25.0` (`ADVERTISED_CONTRACT_BUNDLE` in
`crates/substrate-wire/src/lib.rs`), not yet published; release `0.14.0` published `0.23.0`.
Neither is stable.
Kubernetes, Docker and
Firecracker requests expose property-based capability facts; clients never branch on which driver
answered. Fleet scheduling, billing, product quotas, connector semantics and agent loops remain
outside Substrate.
