# Roadmap

Serves **O1** of `atlas/ROADMAP.md`, the collection's objectives; this page orders outcomes inside
this repository. Work-item status and dependency edges live only in `.engineering/planning/` and
are read with `protocol artifact board` and `protocol artifact graph`.

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
| Contract distribution and SDK | consumers pin a signed bundle and use the contract the daemon actually advertises through a source-distributed Rust SDK | current hardening and bundle-publication evidence are complete | signed bundle publication, coordinated advertised-header migration, SDK parity and exact-revision source consumption are proven | active — signed bundle publication, the advertised-header migration and SDK parity are implemented; source-only distribution is gated |
| Remote serving | agent-platform and other services address one Substrate instance over HTTPS/WSS with Identity-scoped authority | the promoted contract surface is selected; TLS and trust-envelope designs are accepted before code | remote SDK and clean-room conformance prove durable recovery, event gaps, session authority and negative TLS/auth cases | active — production TLS, hosted Identity admission, proof-bound session authority and remote SDK transport are implemented; independent clean-room conformance remains |
| Kubernetes deployment and driver | Kubernetes provides a node-bound host profile and a separately gated namespace workspace/exec driver | remote listener/auth prerequisites hold; namespace work also needs its RBAC and ownership gate | stable per-instance addressing and storage are proven without round-robin mutations; PVC/pod execution passes shared conformance | proposed |
| Docker driver | the same contract serves container-backed workspace/exec and immutable image-backed workloads | phase-4 authority and shared remote conformance are green; the Docker root-equivalence gate is accepted | closed container specs, durable Docker dispatch, immutable image identity and restart cleanup pass the shared driver journey | proposed |
| Firecracker driver | one bounded execution runs in a fresh directly managed microVM | the direct-driver design, immutable boot-artifact gate and a dedicated KVM-capable host are available | jailer/KVM probes and the microVM workspace/exec slice pass live conformance; unsupported hosts report absence | proposed — current dev nodes provide no KVM surface |
| Stack adoption | independent consumers prove the public contract rather than sibling implementation paths | a consumer records adoption in its own repository | at least one product execution path uses the remote SDK and published contract evidence | externally gated |
| Hosted composition | identity, placement and product services operate Substrate without moving their policy into it | remote conformance and an external adoption record are complete | production deployment evidence shows the execution data plane remains standalone and policy-free | pending |

The tagged Substrate `0.4.0` release is a software release, not the similarly numbered historical
contract bundle or a promise that the current `substrate-wire/0.15.0` development bundle is stable.
Kubernetes, Docker and
Firecracker requests expose property-based capability facts; clients never branch on which driver
answered. Fleet scheduling, billing, product quotas, connector semantics and agent loops remain
outside Substrate.
