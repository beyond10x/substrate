---
title: Deployment postures
description: How reachability and trust change without weakening Substrate's resource semantics.
---

# Put the same contract behind an explicit trust boundary

A deployment posture changes reachability, authentication, storage, and operations. It does not
change what a workspace, exec, operation, refusal, or observation means.

## Personal

The current starting point is an owner-permissioned Unix socket:

- the operator explicitly allows one or more numeric user IDs;
- each request derives its local subject from kernel peer credentials;
- the daemon governs one owner trust domain;
- callers may connect directly without a platform network hop.

This is the posture used in [run a bounded command](./run-a-command.md#start-with-an-observable-machine).

## Organization and hosted shapes

Private-network and hosted compositions require deployment identity, scoped authority, encrypted
transport, rotation, and operational controls around the same data-plane contract. Current source
serves the control plane over TLS with online Identity admission, proof-bound network session
authority and an explicit-trust remote Rust SDK.

One daemon remains one trust domain. Do not multiplex mutually untrusted tenants through the same
daemon merely because a higher layer can attach different account labels.

## Production TLS transport

Current source can terminate TLS 1.3 for HTTP and WebSocket control traffic with an explicit server
identity:

```bash
substrate-daemon \
  --socket /run/substrate/local.sock \
  --state /var/lib/substrate/state.sqlite \
  --workspaces /var/lib/substrate/workspaces \
  --deployment edge-01 \
  --tls-listen 0.0.0.0:8443 \
  --tls-certificate-chain /run/substrate-tls/chain.pem \
  --tls-private-key /run/substrate-tls/key.pem \
  --hosted-identity-origin https://identity.example.com \
  --hosted-identity-ca-bundle /run/substrate-identity/ca.pem
```

The certificate chain and key must be non-empty regular files rather than symlinks. The key must
belong to the daemon's effective user and carry no group or other permission bits. Startup validates
the leaf certificate's time window and its agreement with the key before binding.

To rotate the identity, replace both files completely and send the daemon SIGHUP. New connections
use the replacement only after the complete pair validates; existing connections keep the snapshot
under which they connected. If a replacement is invalid, new connections continue to use the last
valid pair and the daemon emits the safe condition `tls.reload-invalid` without certificate or key
bytes.

The listener authenticates the daemon with its server certificate, then authenticates each caller
from a short-lived opaque Identity access credential. Configure Identity's deployment-owned
audience registry with this exact relying-party profile:

```json
{
  "version": "identity.audiences/2",
  "session": [],
  "access": [{
    "audience": "urn:b10x:substrate",
    "scopes": ["observe", "workspaces", "exec"],
    "groupScopes": []
  }]
}
```

Clients send the resulting credential as a bearer. For example, after obtaining it through the
deployment's Identity login flow:

```bash
curl --cacert ./substrate-ca.pem \
  --header "Authorization: Bearer $SUBSTRATE_ACCESS_TOKEN" \
  https://substrate.example.com:8443/v1/machine
```

Substrate resolves the credential at Identity's `GET /v1/access-authority` endpoint on every
request, over direct HTTPS rooted only in `--hosted-identity-ca-bundle`. It requires audience
`urn:b10x:substrate`, accepts at most a five-minute authority, and checks the addressed route before
the handler can write durable state:

| Scope | Route families |
|---|---|
| `observe` | machine facts, metrics, events, reconciliation snapshots, operation observations |
| `workspaces` | workspaces, files, workspace leases |
| `exec` | execs, exec leases, raw-pipe and PTY sessions |

Missing, invalid, under-scoped and temporarily unavailable authority returns
`auth.credential-absent`, `auth.authority-invalid`, `auth.scope-denied` or
`auth.authority-unavailable`. No stale authority is cached, so a completed Identity revocation
applies to the next request. The per-request cost is one HTTP round trip to Identity: Substrate
keeps at most 8 idle keep-alive connections to the Identity origin, closes one after 30 seconds
idle, and never reuses a connection after an error, a refusal or a response that asked to close it.
A TCP and TLS handshake happens only when no idle connection is available. Caller-written subject, tenant, actor, UID and forwarded-address
headers do not become identity. There is no production plaintext fallback, mutual-TLS identity
mapping, trusted forwarded address, redirect, proxy, or verification-disable switch.

## Static-bearer TCP is development-only

The currently implemented TCP transport is deliberately restricted. It requires all of:

- an explicit development-only acknowledgement;
- an explicit private-overlay acknowledgement;
- a loopback listen address;
- a bounded bearer file;
- deployment-owned subject and actor bindings.

It is not production hosted admission and cannot be bound to a non-loopback address. Use it only
for local development.

## Operational health and metrics

`--ops-listen <address:port>` (or `SUBSTRATE_OPS_LISTEN`) starts a separate operational listener.
It is off by default, and it must use a different address from every control listener. It is
plaintext and unauthenticated, so bind it to loopback or a network only your monitoring can reach.
It answers `GET` on three paths. Every other path and method, a request body and an upgrade get
`404` with an empty body. It serves at most 8 connections, at most 4 of them from one source
address, and gives each 2 seconds to send its request head.

| Path | Answer |
|---|---|
| `/livez` | `200 ok` while both maintenance tasks run; `503` once either has stopped |
| `/readyz` | `200` when every check passes, otherwise `503`; one `<check> ok` or `<check> failed` line each for `store`, `schema`, `maintenance` and `driver` |
| `/metrics` | OpenMetrics text, `application/openmetrics-text; version=1.0.0` |

The four readiness checks:

- `store`: one read-only statement returns within 1 second.
- `schema`: that statement reads the schema version this binary expects. The daemon refuses to
  start on a database a newer binary stamped, and leaves its stamp unchanged. From 0.16.1 a store
  is stamped schema version 2 when it opens, which 0.15.x and earlier refuse: back up the state
  directory before upgrading if you may roll back.
- `maintenance`: each maintenance task completed a round in the last 5 seconds.
- `driver`: the startup capability snapshot publishes every `--ops-require-fact <fact>`. The flag is
  repeatable (or `SUBSTRATE_OPS_REQUIRE_FACT`, comma-separated). Startup refuses a name that is not
  a capability fact.

`/readyz` never calls Identity. It answers `503` from the moment shutdown begins, before any
listener closes.

The metric set is fixed. Labels are closed sets: listener, route family, outcome class, refusal
code, check, maintenance task and admission result. No subject, resource id, path, address or
credential appears in a label or a value. A scrape carries at most 584 samples on a host daemon
and at most 585 samples on a daemon running `--driver kubernetes`.

- **Build and readiness:** `substrate_build_info`, `substrate_ready`.
- **Requests:** `substrate_http_requests_total`, `substrate_http_request_duration_seconds`,
  `substrate_refusals_total`.
- **Dispatch:** `substrate_dispatch_total`, `substrate_dispatch_duration_seconds`.
- **Connections:** `substrate_connections_open`, `substrate_connections_limit`,
  `substrate_connections_rejected_total`.
- **Output:** `substrate_output_stalls_total`.
- **Maintenance:** `substrate_maintenance_rounds_total`,
  `substrate_maintenance_round_duration_seconds`,
  `substrate_maintenance_last_round_timestamp_seconds`.
- **Hosted Identity:** `substrate_hosted_admissions_total`,
  `substrate_hosted_resolution_duration_seconds`.
- **Kubernetes driver only:** `substrate_kubernetes_collisions_total`, the objects the driver met
  under a name it would use and did not create. It leaves each one untouched. A host daemon does
  not publish it.
- **Process:** the standard `process_*` metrics.

Every histogram uses the same 11 buckets, from 5 ms to 10 s.

## Kubernetes: one daemon per dedicated node

Each release publishes a Helm chart at `oci://ghcr.io/beyond10x/charts/b10x-substrate`, with the
release's version. Its default image is that release's daemon, pinned by digest. The chart runs
one daemon pod per node of a dedicated pool. Each pod is its own trust domain with its own ledger.

Check the chart before you install it:

```bash
cosign verify \
  --certificate-identity https://github.com/beyond10x/substrate/.github/workflows/release.yml@refs/tags/<version> \
  --certificate-oidc-issuer https://token.actions.githubusercontent.com \
  ghcr.io/beyond10x/charts/b10x-substrate@<digest>
gh attestation verify oci://ghcr.io/beyond10x/charts/b10x-substrate@<digest> --repo beyond10x/substrate
helm install substrate oci://ghcr.io/beyond10x/charts/b10x-substrate --version <version> \
  --namespace substrate --values values.yaml
```

Use Helm 3.22.1 or later on Kubernetes 1.32 or later. The chart lane tests Helm 3.22.1 on
Kubernetes 1.37.

### Before you install

Create the namespace yourself. The chart never creates or deletes it. With `execution.enabled`,
label it to enforce Pod Security `privileged`, with `warn` and `audit` at `baseline`. Without
execution, `restricted` is enough.

The chart refuses to render until you supply each of these:

| Value | What it names |
|---|---|
| `nodePool` | the label and taint of the dedicated nodes |
| `persistence.storageClassName` | a class that binds `WaitForFirstConsumer` |
| `tls.secretName` | the TLS secret with each pod's certificate |
| `identity.origin` | the HTTPS Identity origin |
| `identity.caConfigMap` | Identity's trust roots |
| `identity.egress` | Identity's address range and port |
| `cidr` of each Git source and aperture | the range its egress rule admits |

The certificate names each pod's host and nothing else. `helm template` prints the names, and the
chart writes them to the ConfigMap `<release>-certificate-names`.

### Address one pod, never the Service

Every pod has its own name:
`<release>-<ordinal>.<release>.<namespace>.svc.<cluster domain>`. Send a client's requests to one
pod by that name. The Service name resolves to every ready pod. A workspace or operation created on
one pod does not exist on another, and the certificate does not name the Service.

### What the chart refuses

- a privileged container, a host namespace or a host path for the daemon;
- the development TCP listener and a local uid list;
- a cgroup root, a writable root or a read-only host root;
- project quotas without `execution.enabled`, without `persistence.projectQuotaCapable` or without
  an identity range;
- a pod annotation that selects an AppArmor or seccomp profile;
- egress to any prefix length 0, such as `0.0.0.0/0` or `::/0`, unless
  `networkPolicy.allowAnyEgress` is true.

With `execution.enabled`, the daemon starts as design 25's bootstrap. It holds five capabilities
and selects the seccomp and AppArmor profiles from each node. `nodeProfiles.enabled` installs those
profiles with a DaemonSet in its own `privileged` namespace. Otherwise the node image must carry
them.

The AppArmor profile is `substrate-host-exec-v2` unless `execution.appArmorProfile` names
`substrate-host-exec-v1`. `v2` also admits the `noatime` and `nodiratime` remount flags, which
bubblewrap keeps when the node mounts its filesystems `noatime`. On upgrade, nodes load `v2`.
Not verified: whether production nodes mount `noatime`.

### Operate it

| Task | How |
|---|---|
| Probes | `/livez` and `/readyz` on port 9090 |
| Required facts | list them in `readiness.requireFacts` |
| Scraping | name the scrapers in `networkPolicy.scrapers` |
| Clients | name them in `networkPolicy.clients` |
| Certificate rotation | replace the secret, then `kubectl rollout restart statefulset/<release>` |
| Upgrade | `helm upgrade` replaces one ready pod at a time |
| Uninstall | keeps each `ledger-<release>-<ordinal>` claim |

A pod reads its certificate only when it starts, so a rotation needs the restart. Each ledger claim
stays with its ordinal. Delete the claims yourself to destroy the ledgers.

## Kubernetes namespace driver

`--driver kubernetes` serves workspaces as objects in one namespace. It is a development profile,
first released in 0.16.1.

What it does not do yet:

- **Exec is not served.** The probe proves neither the four Linux confinement facts nor
  `confinement.no-egress`, so every exec start is refused `exec.sandbox-unavailable`.
- **Exec on a cluster is unverified.** On kind a Pod under the default-deny policy still reached
  the API server, so no enforced egress denial was observed and `confinement.no-egress` stays
  absent.
- **Shared clean-room conformance follows in the next release.** The namespace lane reports that
  step absent until `story:remote-clean-room-conformance` lands.

```bash
substrate-daemon --driver kubernetes --kubernetes-config driver.json \
  --deployment <id> --state <file> --socket <path> --allow-uid <uid>
```

The daemon reads that one file. It never reads an ambient kubeconfig or an in-cluster account.

| Setting | Where |
|---|---|
| Cluster, namespace, credentials | the file, a self-contained JSON kubeconfig |
| Driver settings | its context extension `b10x-substrate/namespace-driver` |
| Images | `executor_image` and `exec_image`, by digest |
| Network | `default_deny_policy`, your NetworkPolicy |
| Egress probe | `egress_probe_address`, an `ip:port` outside the cluster |
| Workspace size | `workspace_size_bytes`, 1 MiB to 1 TiB, on `storage_class` |
| Node limits | `pod_pids_limit` and `container_log_max_bytes`, as the kubelet holds them |

Host flags such as `--workspaces` are refused with this driver (`config.driver-conflict`). An
unknown extension member is refused.

The release publishes the executor image `ghcr.io/beyond10x/b10x-substrate-executor:<version>`.
Name it by digest as `executor_image`.

### What it creates

| Object | Name | For |
|---|---|---|
| Lease | `b10x-substrate-driver` | the namespace claim, renewed every 10 s |
| PersistentVolumeClaim | `sbx-ws-<h>` | one workspace |
| keeper Pod | `sbx-ws-<h>-keeper` | every file route, through the executor over `pods/exec` |
| exec Pod | `sbx-ex-<h>` | one exec, once an exec is admitted |

Every object carries the deployment's labels, and every delete names the uid the create answered.
The driver never modifies or deletes an object of another deployment, or one it has no record of.

### The Role

Bind the driver's credential to one namespaced Role with exactly these rules:

| Resource | Verbs |
|---|---|
| `pods` | `create`, `get`, `list`, `watch`, `delete` |
| `pods/log` | `get` |
| `pods/exec` | `get`, `create` |
| `persistentvolumeclaims` | `create`, `get`, `list`, `delete` |
| `networkpolicies` | `get`, `list` |
| `leases` (`coordination.k8s.io`) | `create`, `get`, `update` |

The API server authorizes the exec WebSocket as `get`, so `pods/exec` needs both verbs. A
credential that visibly holds more than this and the rules every authenticated subject holds, a
wildcard or any verb on `secrets` is refused at startup (`kubernetes.rbac-excess`).

### Startup, probe and readiness

| Event | Result |
|---|---|
| The file does not load | the daemon stops: `kubernetes.config-invalid` |
| The cluster check fails | it serves; driver facts absent |
| The probe passes | probed facts published |
| A probe fails or lapses | those facts absent again |

The log names each failure's code. The cluster check repeats every 10 s; a failed probe after 60 s.
It refuses a Lease another instance holds, a policy that is not default-deny, a namespace that
admits a `privileged` Pod, and an executor that fails its handshake.

The probe creates a claim, a keeper and a Pod, then deletes them. It proves
`workspace.guarded-io`, `workspace.openat2-beneath`, `workspace.atomic-replace` and
`exec.argv-only`, each from its own observation.

Readiness reads one snapshot, taken after the first probe, or when the cluster check fails for a
reason a retry cannot clear (RBAC, NetworkPolicy, Pod Security). A held Lease or an unanswering API
is retried and keeps the daemon unready until then. A fact named by `--ops-require-fact` and
absent from the snapshot keeps the daemon unready until it restarts.

### What it answers

| Request | Answer |
|---|---|
| a create the namespace's ResourceQuota refuses | `workspace.capacity` or `exec.capacity` (429) |
| `workspace.adopt` | `workspace.adopt-unserved` (501), before any name is looked up |
| a workspace storage quota | `workspace.storage-quota-unserved` (501) |
| a workspace source other than empty | `workspace.source-unserved` (501) |
| any exec start | `exec.sandbox-unavailable` (501) |

A lost Lease makes every fact absent. After a restart, a workspace whose claim is found and whose
keeper answers its handshake is ready again, and an exec still running reads `unknown` and its Pod
is deleted.

## Rules that do not change by posture

- Never bind a reachable unauthenticated control listener.
- Never infer a capability from a posture name.
- Never weaken a requested sandbox because the selected machine lacks enforcement.
- Keep fleet scheduling, membership, rich grants, product quotas, and billing outside the daemon.
- Keep continuous session bytes off an ordinary governance or invocation proxy.
- Scope resources and operation IDs to the authenticated subject.

## Deployment review

Before making a daemon reachable beyond its owner, answer:

1. Which trust domain owns the machine?
2. How does a connection prove its subject and immediate actor?
3. How are credentials bounded, rotated, and removed?
4. Which network can reach the control listener?
5. Which execution capabilities did this exact backend verify?
6. Where do event retention, state storage, and workspace data live?
7. What refuses startup if any required trust control is absent?

If any answer relies on “the surrounding network is probably safe,” keep the Unix socket posture.

## Unrecorded interactive sessions

`--allow-unrecorded-sessions` (or `SUBSTRATE_ALLOW_UNRECORDED_SESSIONS=true`) explicitly permits
authorized clients to select live-only terminal output. It defaults to disabled and does not
change ordinary session capture. The same confinement, identity, scope and attachment checks apply.
See the [Rust SDK guide](./rust-sdk.md#stream-terminal-output-without-recording-it) for selection,
observations, bounded streaming and the no-replay contract.
