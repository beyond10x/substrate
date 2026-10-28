---
status: accepted
date: 2026-10-28
---

# ADR 0046: Kubernetes serves one node-bound daemon per stable pod address

Decides `story:node-bound-kubernetes-serving-profile`. **Accepted** by the operator on 2026-10-28
with the recommended option of each question in § *Decisions for the operator*: K1 to K6 and K8
are A, K7 is C, and K9 is B with C as the fallback for any scenario a hosted runner cannot
execute. § *Decision* stands as written. The profile's probes need
[ADR 0045](0045-operational-health-and-metrics-use-a-separate-listener.md), accepted the same day,
so the story depends on `story:operational-health-and-telemetry` (K8).

## Context

- **The story asks for:** a public, signed and provenance-attested Helm chart. The chart deploys a
  StatefulSet on a dedicated, labelled and tainted node pool, with one owner-private PVC per ordinal
  and stable per-pod DNS. No Service may round-robin mutations across independent ledgers.
  Upgrades preserve identity and state. Install, upgrade and uninstall pass conformance.
- **De-duplication is per pod.** Each daemon has its own SQLite ledger keyed by
  `(deployment, subject, operation)` (`xtask/src/render.rs:740`,
  [ADR 0005](0005-operations-are-durable-before-driver-dispatch.md)). A retry that reaches another
  ordinal runs again. A headless Service's own DNS name resolves to every ready pod, so a client
  that addresses the Service name can land a retry on the wrong ledger.
- **One audience for all ordinals.** The hosted audience is the constant `urn:b10x:substrate`
  (`crates/substrate-daemon/src/hosted.rs:37`), shared by every ordinal. Per-pod addressing
  therefore changes nothing on the wire.
- **The daemon already enforces the transport.** TLS and hosted Identity require each other
  (`crates/substrate-daemon/src/runtime.rs:573-577`). With TLS, `--allow-uid` is not required
  (`runtime.rs:578`), and the development TCP listener conflicts with TLS
  (`crates/substrate-daemon/src/main.rs:405`).
- **Key file rule.** [ADR 0024](0024-production-network-control-uses-server-authenticated-tls.md)
  :34-38: the private key is "owned by the daemon's effective uid with no group or other permission
  bits", and "Neither path may be a symlink". `read_bounded_regular_file`
  (`crates/substrate-daemon/src/tls.rs:108-126`) opens with `O_NOFOLLOW` and checks
  owner and mode. `load_connector` (`hosted.rs:373-385`) opens the Identity CA bundle with
  `O_NOFOLLOW`, with no owner rule. Kubelet presents each key of a Secret or ConfigMap volume as a
  root-owned symlink into a `..data` directory. That is known kubelet behaviour, not observed in
  this repository.
- **One precedent for projected files.** The development bearer file admits a root-owned file whose
  group is the effective gid, with group read and no other bits (`root_projected_group`,
  `runtime.rs:1582-1585`). It is opened following symlinks (`runtime.rs:1575`).
- **State layout.** The state database's parent directory must be uid-owned with mode 0700, and the
  daemon creates it if absent (`prepare_private_state_path`, `runtime.rs:1632-1659`). The image
  runs as `65532:65532` with `/var/lib/b10x-substrate` as its state volume (`Dockerfile:70-73`).
- **Container execution needs privilege.** The bootstrap starts as UID 0 with exactly `SYS_ADMIN`,
  `CHOWN`, `SETUID`, `SETGID` and `SETPCAP`, then drops to 65532
  (`docs/design/25-container-private-host-execution.md:29-57`). It needs a Localhost seccomp profile
  (:93-102) and an AppArmor profile that a separate node-profile DaemonSet installs with `MAC_ADMIN`
  and securityfs (:104-113). Chart composition selects the bootstrap only by explicit opt-in
  (:124-128). The Kubernetes Pod Security `baseline` level does not admit an added `SYS_ADMIN`, or a
  host path.
- **NetworkPolicy proves nothing by itself.** It is enforced by the network plugin, which the API
  does not let a client prove (`docs/design/28-pod-confinement-adapter.md:17-19`).
- **Distribution.** [ADR 0030](0030-rust-crates-are-source-distributed-and-non-publishable.md):
  releases publish GitHub and GHCR artifacts only. [ADR 0006](0006-substrate-publishes-its-own-contract-bundle.md)
  :38-44 is the pattern: a write-once tag, a keyless signature, verification against the tagged
  workflow identity, and an anonymous read-back before anything is announced. The release needs no
  repository secret (AGENTS.md § *Bot identity*). Provenance is off on purpose:
  `.github/workflows/release.yml:74-76` disables buildx attestations because they wrap the pushed
  digest in an index.
- **CI.** The `pod-lane` job (`.github/workflows/gate.yml:166-271`) pins kind `v0.33.0` and
  `kindest/node:v1.37.0` (:175-177). It creates one control-plane node (:215-226) with a
  `restricted` namespace (:234). AGENTS.md § *The gate*: "A change does not buy confidence with CI
  time". A test runs its scenario once, in seconds, and a workflow grows by about a minute at most.

## Decision

1. **The chart runs the host driver, one daemon per node.** It lives in `deploy/helm/substrate/` and
   is named `b10x-substrate`. It is not the namespace-scoped Kubernetes driver, not a fleet
   scheduler, and not high availability for one ledger.
2. **A StatefulSet with one PVC per ordinal.** The `volumeClaimTemplates` PVC (ReadWriteOnce) is
   mounted at `/var/lib/b10x-substrate`, and the daemon gets
   `--state /var/lib/b10x-substrate/state/state.db`, so it creates `state/` itself, uid-owned with
   mode 0700. The pod sets `fsGroup: 65532`. `--deployment` is the pod's own name, taken from the
   downward API, which the StatefulSet keeps stable across restarts and upgrades.
3. **Node binding.** A required `nodeSelector` and toleration for the dedicated pool. Required pod
   anti-affinity on `kubernetes.io/hostname`, so there is at most one daemon per node. The storage
   class must bind on first consumer, so the volume follows the node.
4. **Production transport only.** The values have no development-TCP or `--allow-uid` field:
   `values.schema.json` is closed (`additionalProperties: false`) and requires the TLS certificate,
   the key, the Identity origin and the Identity CA. The daemon refuses a partial configuration
   itself (`runtime.rs:573-577`).
5. **Execution is an explicit opt-in.** `execution.enabled: true` selects `substrate-container-exec`
   as the entrypoint, with the Localhost seccomp and AppArmor profiles of design 25. Without it, the
   plain daemon starts and exec answers `exec.sandbox-unavailable`; nothing claims a fact it lacks.
   The schema refuses `privileged: true`, host namespaces and host paths for the daemon pod. A
   quota profile is selectable only together with a declared quota-capable storage class.
6. **No Kubernetes API access.** The daemon's ServiceAccount has no Role and
   `automountServiceAccountToken: false`. The node-profile DaemonSet receives no token either
   (design 25:105-106).
7. **Upgrades and removal keep state.** `updateStrategy: RollingUpdate`, one pod at a time, gated on
   readiness. Each restarted pod drops its own live sessions, and its live execs go through restart
   reconciliation. `persistentVolumeClaimRetentionPolicy` is `Retain` both when the StatefulSet is
   deleted and when it is scaled down. Uninstall leaves every ledger PVC; deleting one is a
   documented manual step. Scaling back up re-attaches the same ordinal's ledger.
8. **Addressing, certificates, key loading, Pod Security, egress, distribution, provenance, probes
   and the lane** follow K1 to K9 below, as recommended.

## Decisions for the operator

### K1 — how a caller addresses one pod

| option | what it does | what it costs |
|---|---|---|
| **A** the headless Service and its per-pod DNS names (`<pod>.<headless>.<namespace>.svc.<cluster-domain>`) only. No other Service selects the pods | each caller pins one ordinal by name. A pod's record is published only while it is ready | reachable only inside the cluster. Out-of-cluster callers need a later exposure decision |
| **B** one ClusterIP Service per ordinal, selecting `statefulset.kubernetes.io/pod-name` | stable virtual IPs per pod, which a later load balancer per pod can front | N Services. Scaling up means re-rendering the chart. Each Service adds a name that the SANs must list |
| **C** a Gateway with TLS passthrough, routing by SNI to each pod | out-of-cluster reach with end-to-end TLS (ADR 0024: no terminating proxy) | needs a Gateway implementation in the cluster, and the chart cannot verify that it passes TLS through untouched |

**Recommendation: A.** It is the smallest surface that meets the story. B and C remain additive later
choices.

### K2 — which names the certificate carries

The rule for every option: no SAN names the headless Service, or any name that resolves to more
than one pod. A client that addresses the Service then fails verification instead of being spread
across ledgers.

| option | what it does | what it costs |
|---|---|---|
| **A** one certificate listing every ordinal's per-pod name, in one Secret shared by every pod | no cluster dependency. The chart computes the SAN list from `replicas` for the operator's issuer | every pod holds the same key, so one compromised pod can impersonate any ordinal. Scaling beyond the listed ordinals needs a reissue |
| **B** as A, with the one-label wildcard `*.<headless>.<namespace>.svc.<cluster-domain>` | scaling needs no reissue. The wildcard does not match the Service name itself | the wildcard also matches any pod in the namespace that sets `subdomain: <headless>`, which would still need the key to impersonate |
| **C** a per-pod key issued on the node by a CSI certificate driver | each pod holds only its own key | a cluster dependency the chart does not own. Whether that driver's files are a symlink or root-owned is unverified against `tls.rs`, so it may also need K3 B. The lane must install the driver |

**Recommendation: A.** No dependency, and scale is explicit configuration. C is a later opt-in if
per-pod keys become a requirement.

### K3 — how TLS and Identity CA material get from Secrets to the daemon

| option | what it does | what it costs |
|---|---|---|
| **A** an init container, running as 65532, copies the certificate, the key and the Identity CA from the Secret and ConfigMap volumes (`defaultMode: 0440`, group 65532) into a memory-backed `emptyDir`. Each file is a regular file owned by 65532, the key mode 0400 | ADR 0024 and the daemon are unchanged | rotation needs a pod restart, so ADR 0024's "rotation does not require a restart" does not hold in this profile: each rotation rolls the StatefulSet and drops live sessions. The key sits in two tmpfs copies inside the pod |
| **B** amend ADR 0024 to admit the projected form. A file may resolve beneath a declared directory (`openat2` with `RESOLVE_BENEATH` and `RESOLVE_NO_MAGICLINKS`), and may be root-owned with group equal to the effective gid and group read only, as `root_projected_group` admits the bearer (`runtime.rs:1582-1585`) | restart-free rotation: kubelet swaps `..data` atomically, so the certificate and key change together | changes an accepted security rule, `tls.rs` and `hosted.rs`. Something must still send SIGHUP after kubelet swaps the files, a sidecar sharing the process namespace or a daemon-side file watch, and this ADR does not decide which |
| **C** point the flags at kubelet's internal `<mount>/..data/<file>`, so the final component is a regular file | no copy step | `..data` is an undocumented kubelet layout. The files are still root-owned, so the owner rule fails and B's amendment is needed anyway |

**Recommendation: A** for the first chart, with B as a labelled later milestone if restart-free
rotation becomes a requirement. A changes no accepted security rule and no daemon code.

### K4 — Pod Security level

| option | what it does | what it costs |
|---|---|---|
| **A** a dedicated namespace with `enforce: privileged` and `warn`/`audit: baseline`. A chart test asserts that the daemon pod adds only design 25's five capabilities, sets no `privileged: true`, no host namespace and no host path, and selects Localhost seccomp and AppArmor | admitted on any conformant cluster. Every deviation from baseline is reported on apply | namespace admission no longer stops another privileged pod in that namespace, so nothing else may run there |
| **B** a `baseline` namespace, with the daemon admitted through a cluster-level Pod Security exemption (by namespace, user or RuntimeClass) | the namespace keeps a stricter label | the exemption is API-server admission configuration that the chart can neither install nor check. It differs per cluster, and kind needs a custom API-server configuration |
| **C** `baseline` or `restricted` without the bootstrap | no added privilege | exec is refused `exec.sandbox-unavailable`. The profile then serves no execution, which is its purpose |

The node-profile DaemonSet needs `MAC_ADMIN` and securityfs under every option. It goes into its own
namespace at `privileged`.

**Recommendation: A.** It is the only option the chart can state, test and keep honest by itself.

### K5 — NetworkPolicy

| option | what it does | what it costs |
|---|---|---|
| **A** default deny in both directions. Ingress to the TLS port from selectors named in values, and to the operational port from a named scraper selector. Egress to cluster DNS, plus one rule per configured destination (Identity origin, each Git source, each egress aperture), each with an explicit `cidr` and `port` that `values.schema.json` requires. `0.0.0.0/0` is refused unless `networkPolicy.allowAnyEgress: true` is set | portable to every plugin that enforces NetworkPolicy | the operator keeps CIDRs beside hostnames. A destination whose addresses move breaks until the values change (ADR 0013 already pins an aperture's address when it is declared) |
| **B** plugin-specific FQDN policies as opt-in templates | follows hostnames | not portable. Two policy dialects to test, and kind's default plugin serves neither |
| **C** default deny for ingress only | nothing to maintain for egress | a compromised daemon reaches anything. The story asks for default-deny egress |

Under every option the chart claims nothing from the policy (design 28:17-19). The lane proves
enforcement with a refused connection, and reports it absent where the plugin does not enforce.
Whether the plugin admits kubelet probes under default-deny ingress varies. The chart adds no rule
for them, and the lane observes the result.

**Recommendation: A.**

### K6 — chart distribution

| option | what it does | what it costs |
|---|---|---|
| **A** an OCI chart at `ghcr.io/beyond10x/charts/b10x-substrate:<version>`, packaged after the daemon image is pushed, so its default values pin that image by digest. Write-once tag, keyless cosign signature, verification against the tagged workflow identity and an anonymous read-back before the release is announced, all with `GITHUB_TOKEN` (the ADR 0006 pattern) | `helm install oci://…` with a verifiable digest. No secret | the release job gains a pinned, checksum-verified `helm`, and GHCR gains a fourth artifact. Consumers need Helm 3.8 or later |
| **B** a `.tgz` attached to the GitHub release, signed with `cosign sign-blob` like the release binaries | no registry artifact | installs by URL, not `helm pull oci://`. A second signing path for one kind of artifact |
| **C** a classic Helm repository (`index.yaml`) on GitHub Pages | familiar `helm repo add` | Pages hosts the docs site (AGENTS.md § *Public website*). The index is mutable, and Helm's PGP `.prov` needs a signing key, which would be the release's first repository secret |

**Recommendation: A.** It reuses the bundle's exact publication pattern and needs no secret.

### K7 — provenance

| option | what it does | what it costs |
|---|---|---|
| **A** the cosign signature only, with the story's "provenance-attested" reworded to "signed" | nothing new | a signature says who released the chart, not from which source and run |
| **B** `cosign attest` with an SLSA provenance predicate the workflow writes | an in-toto attestation under the same keyless identity | the predicate's content is asserted by the workflow's own script |
| **C** `actions/attest-build-provenance` with `push-to-registry` on the chart digest. The attestation is stored as an OCI referrer, so the chart digest is unchanged, and the release verifies it before announcing | SLSA provenance built from the run's OIDC claims | one pinned action and one permission, `attestations: write`, on the release job. The images keep provenance off (`release.yml:74-76`) unless decided separately |
| **D** the SLSA GitHub generator's reusable workflow (build level 3) | an isolated builder | a larger restructuring of `release.yml` and another pinned dependency, for a chart tarball where the isolation buys little |

**Recommendation: C.** It meets the story without changing any digest and without a secret.
`release.yml:74-76` turned provenance off only because buildx changes the digest. This does not.

### K8 — probes, which depend on ADR 0045

| option | what it does | what it costs |
|---|---|---|
| **A** sequence ADR 0045 first. The chart ships `httpGet` probes on the operational port: `/livez` for liveness, `/readyz` for readiness and as the startup gate | readiness reflects the store, maintenance and required driver facts. A not-ready pod's DNS record is withdrawn | the chart waits for the telemetry story |
| **B** ship `tcpSocket` probes on the TLS port, amend later | no wait | proves only that the accept loop runs. Every probe is a failed TLS handshake in the daemon's log, and a later probe change is an upgrade that rolls every pod |
| **C** `exec` probes running a client inside the container | no listener needed | a process per probe period, and a client binary the image does not ship |

**Recommendation: A.** `story:node-bound-kubernetes-serving-profile` should gain a `depends_on`
edge to `story:operational-health-and-telemetry`.

### K9 — the conformance lane and the CI budget

The lane: a kind cluster with one control-plane node and two workers that carry the pool's label
and taint. The node-profile DaemonSet installs the profiles, then the chart installs with an
in-cluster Identity stub. Each scenario runs once:

- both ordinals become ready on distinct workers;
- a per-pod name verifies, and the headless Service name fails verification;
- a workspace written on ordinal 0 survives an upgrade with changed values, as does that pod's
  ledger;
- a denied egress connection is refused, or the lane reports it absent;
- the chart is admitted in a `privileged` namespace and refused in a `baseline` one;
- uninstall leaves the ledger PVCs and nothing else.

Several items are unproven inside a kind node: AppArmor profile loading, the private cgroup
delegation, and whether kind's default plugin enforces NetworkPolicy.

| option | what it does | what it costs |
|---|---|---|
| **A** extend the `pod-lane` job in `gate.yml` | one workflow, one cluster | every pull request pays the daemon image build and a three-node cluster. That breaks the budget of about a minute per workflow |
| **B** a workflow of its own (`chart-lane.yml`). Its first step is the static checks: rendering, values-schema validation, the K4 pod-spec assertions, and that no Service other than the headless one selects the pods. These run in seconds, then the cluster scenarios run. It runs on every push to `main`, on pull requests that touch `deploy/helm/`, `Dockerfile` or the daemon crate, and on dispatch. `release.yml` refuses a tag whose commit has no successful conclusion, as it already does for `gate.yml` | `gate.yml` grows by zero. Chart breakage shows up on the pull request that causes it | a second required signal for releases. The lane's duration is dominated by the image build and is unmeasured. The story records the first run's time |
| **C** static checks in a workflow, with the cluster scenarios as a recorded local lane (a `Chart-lane:` line in the annotated tag, like `Confinement-lane:`) | no hosted-runner cluster time | the evidence is a workstation run. The release preflight gains a third record format |

**Recommendation: B**, falling back to C for any scenario that a hosted runner cannot execute (for
example, AppArmor profile loading). The Full gate stays as it is, and the cluster cost lands only on
changes that can break the chart.

## Consequences

- New surfaces: `deploy/helm/substrate/`, `chart-lane.yml` with a Rust lane runner (an `xtask` verb,
  per the language rule), and chart packaging, signing, attestation and read-back steps in
  `release.yml` with their refusals in `xtask/tests/release_workflow.rs`. AGENTS.md § *Releases*
  names the fourth artifact and the new preflight condition. No daemon code changes under the
  recommended options. K3 B or K2 C would change `tls.rs` and `hosted.rs`.
- No wire change. None of the options needs one. One would arise only from a per-ordinal audience
  or an instance-identifying response header, and neither is proposed.
- A rotated certificate reaches a pod only when that pod restarts (K3 A). The operator's issuer
  rotation therefore schedules a rolling restart, and the deployment guide says so.
- Execution in the pod remains subject to design 25's startup checks. A node without the profiles
  refuses startup, and the pod is never ready.

## What this does not decide

- The namespace-scoped Kubernetes driver, scheduling across nodes, and high availability or
  replication of one ledger.
- Exposure outside the cluster (K1 B or C), certificate issuance automation, and restart-free
  rotation (K3 B).
- Project quotas on PVCs. The lane's local-path storage has none, so `scratch` is refused there,
  and a quota-capable storage class is the operator's.
- arm64 images, PVC backup and restore, and autoscaling.
- Provenance for the daemon, MCP and bundle images (`release.yml:74-76` stays as it is for them).
