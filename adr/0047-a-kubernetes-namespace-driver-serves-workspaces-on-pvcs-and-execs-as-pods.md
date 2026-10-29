---
status: accepted
date: 2026-10-29
---

# ADR 0047: A Kubernetes namespace driver serves workspaces on PVCs and execs as Pods in one claimed namespace

Decider: operator.

Decides `story:kubernetes-namespace-driver-entry-gate`, the authority gate in front of
`story:kubernetes-namespace-workspace-and-exec-slice`. **Accepted** by the operator on 2026-10-29
with the recommended option of each of the twelve questions in § *Decisions for the operator*, N1
to N12, as written at commit `ce3fb77c` (`approval-record:kubernetes-namespace-driver-accepted`).
The operator's answer, verbatim: "A". § *Decision* stands as written. The four behaviours
§ *Consequences* lists as unobserved are checked on the kind lane before the implementation relies
on them, and the implementation's own design comes before code (invariant 8).

**Amended** by the operator on 2026-10-29 through design 34 § 14 Q1, answer B: decision 3's table
adds `list` on `networkpolicies`, so the driver binds the namespace's whole policy set into the
capability snapshot rather than only the named default-deny policy
(`approval-record:design-34-accepted`). The operator's answer, verbatim: "accept all if it s the
right and not the cheapest low effort choice".

The operator made the track *Kubernetes deployment and driver* part of the definition of done on
2026-10-29 ([ROADMAP.md](../ROADMAP.md) § *Definition of done*, item 2). Its node-bound half is
[ADR 0046](0046-kubernetes-serves-one-node-bound-daemon-per-stable-pod-address.md). This is the
other half.

## Context

- **A driver, not an adapter.** The `pod` adapter serves the confinement API in substrate-host to an
  embedder. A driver serves the whole wire through the daemon
  ([ADR 0044](0044-confinement-is-a-contract-family-served-by-declared-adapters.md) decision 8).
  [Design 28](../docs/design/28-pod-confinement-adapter.md) § *Not decided here* leaves restart
  reconciliation, durable-before-API dispatch, watch gaps, quotas, server-side apply and finalizers
  to this gate.
- **The story asks for:** configured namespace ownership, ServiceAccount and minimal RBAC, object
  naming and labels, server-side-apply ownership, finalizers, durable-before-API dispatch, immutable
  backend identity, watch gaps, restart reconciliation, cancellation, garbage collection, quotas,
  NetworkPolicy defaults and refusal order. A startup probe publishes only facts it proves with
  disposable resources and cleans them. Requests cannot select namespaces, service accounts, host
  paths, privileged settings, node names, runtime classes or arbitrary Pod fields. The lane creates
  no cluster-scoped resource and leaves no object after cleanup.
- **What is already established for one Pod** (design 28, `crates/substrate-host/src/confinement/pod.rs`):
  the hardened kubeconfig loader and its refusals; the fixed Pod rendering under Pod Security
  `restricted`; the namespaced Role `create`, `get`, `list`, `watch`, `delete` on `pods` and
  `networkpolicies` plus `get` on `pods/log`, with `watch` on `pods` added by decision D173
  (`decision-blocker:pod-force-deleted-policy-rbac`); the rule that a gone Pod proves nothing until
  the API shows its container terminated; no Kubernetes client crate and no `kubectl`; and that a
  NetworkPolicy is enforced by the network plugin, which the API does not let a client prove.
- **Durability.** [ADR 0005](0005-operations-are-durable-before-driver-dispatch.md) and
  [design 03](../docs/design/03-lifecycle-observation-and-reconciliation.md) V1 decisions 1 to 3:
  intent is committed before dispatch, a crash after dispatch leaves `accepted`, restart reconciles
  from driver observation, and nothing is repeated because the daemon restarted.
- **Driver model.** [Design 02](../docs/design/02-driver-and-capability-model.md): one active driver
  per daemon; configured intent is not a fact; an unreachable cluster yields absent facts; the
  security-critical backend binding is recomputed immediately before dispatch. Design 01 § 10.3:
  Kubernetes is namespace-per-daemon with no CRD.
- **Admission reads Linux facts.** Exec start is refused `exec.sandbox-unavailable` (501) unless
  `exec.namespaces`, `exec.cgroup-limits`, `exec.cgroup-kill` and `exec.no-egress` hold
  (`crates/substrate-daemon/src/app/operations.rs`). Those facts name Linux mechanisms. Redefining a
  released fact's meaning breaks invariant 6
  ([design 30](../docs/design/30-confinement-adapter-on-the-wire.md) D3). Design 30, proposed,
  adds mechanism-neutral `confinement.*` facts and leaves a later driver's `driver` value to its
  entry gate (D9).
- **The wire names one driver.** `HostDriverKind` has the one variant `host`, and the released
  capability schema pins `"driver": {"const": "host"}`. Design 30 § 4.1 fixes `host` to mean
  workspaces and processes on the machine the daemon runs on.
- **Restart, as conformance sees it.** [Design 31](../docs/design/31-remote-clean-room-conformance-runner.md)
  § 4: after a restart an in-flight exec reads `unknown`, and "a driver that re-observes execs across
  a restart would be a contract change with its own design".
- **Container drivers in general.** [Design 15](../docs/design/15-docker-driver-entry-gate.md)
  (proposed) fixes the shape this ADR follows: entry conditions are probe observations, the
  specification is constructed whole with no pass-through, the floor is stated clause by clause with
  the refusal where it cannot be met, and sealed secret slots are absent on a driver that does not
  fork the child.

## Decision

1. **Scope.** The driver serves workspaces and execs from one namespace. A workspace is a PVC; an
   exec is a one-shot Pod. It creates no cluster-scoped object, no CRD, no Deployment, Service or
   Ingress, builds no image and schedules nothing: the cluster's scheduler places Pods. A daemon runs
   exactly one driver (design 02); this driver is selected by daemon configuration and never by a
   request.
2. **One namespace, claimed.** The operator names it in the driver's configuration (N1). At startup
   the driver takes the coordination Lease `b10x-substrate-driver` in that namespace, holder
   `<deployment>/<instance nonce>`, and renews it. A Lease held by another deployment, or by a live
   instance of the same one, leaves the driver unopened and the daemon unready (N2). Before every
   dispatch the driver confirms it still holds the Lease; a lost Lease is `exec.capability-stale`
   (422) for a start and makes every fact absent until it is retaken.
3. **Minimal RBAC, one namespaced Role, nothing cluster-wide.**

   | resource | verbs | for |
   |---|---|---|
   | `pods` | `create`, `get`, `list`, `watch`, `delete` | keeper and exec Pods; `watch` for watch gaps and gone-Pod evidence (D173) |
   | `pods/log` | `get` | exec output (N10) |
   | `pods/exec` | `create` | workspace file I/O in the keeper (N3) |
   | `persistentvolumeclaims` | `create`, `get`, `list`, `delete` | workspaces |
   | `networkpolicies` | `get`, `list` | the namespace default-deny policy is read, never written (N5); `list` binds the digest of the namespace's whole policy set into the snapshot, so an added or changed policy moves `config_generation` (amendment of 2026-10-29, below) |
   | `leases.coordination.k8s.io` | `create`, `get`, `update` | the namespace claim |

   No `secrets`, `configmaps`, `serviceaccounts`, `services`, `nodes`, `namespaces`,
   `pods/ephemeralcontainers`, `pods/attach`, `escalate`, `bind` or `impersonate`. At startup the
   driver creates a `SelfSubjectRulesReview` for its namespace and refuses to open when its
   credential visibly holds more than this table, a wildcard, or any verb on `secrets`. The request
   path holds no field that reaches a namespace, ServiceAccount, node, runtime class, host path or
   security context: `ConfinementRequest` is `deny_unknown_fields`, so such a member is
   `request.schema-invalid` (422).
4. **Ownership is proved per object, by name, label and uid.**
   - Names are derived before dispatch from the deployment and the resource id:
     `sbx-ws-<h>` (PVC), `sbx-ws-<h>-keeper` (keeper Pod), `sbx-ex-<h>` (exec Pod), `<h>` a
     lowercase base32 prefix of SHA-256 over `(deployment, resource id)`. The ledger row holds the
     name before the API call.
   - Every object carries `app.kubernetes.io/managed-by: b10x-substrate`,
     `b10x-substrate/deployment: <digest of deployment>`, the resource label, and the annotation
     `b10x-substrate/operation: <digest of (subject, op)>`. No subject, uid or credential is written
     into the cluster.
   - The uid the create answered is committed as the backend identity. Every delete carries
     `preconditions.uid`, so the API server refuses to delete a same-named replacement.
   - An object whose labels name another deployment, whose uid differs from the ledger's, or that
     has no ledger row is never modified or deleted. It is logged and counted on the operational
     listener, and the row it collides with stays `unknown`.
   - Writes are `POST` with `fieldManager=b10x-substrate`. No server-side apply: the driver never
     updates a Pod or PVC after creating it, so there is no field ownership to share.
   - No finalizers. A finalizer held by a daemon that is gone blocks namespace deletion forever.
   - `ownerReferences`: each Pod points at its workspace's PVC by uid, `controller: false`, no
     `blockOwnerDeletion`, as a backstop only. The driver deletes Pods first, then the PVC.
5. **Workspaces.** A PVC of the operator's storage class, `ReadWriteOnce`, sized by the operator's
   default or the request's storage quota. A keeper Pod mounts it at `/workspace` and runs the
   Substrate executor, which performs every file operation with the host driver's
   `openat2` beneath/no-link/no-mount code. The daemon reaches it through `pods/exec` on the API
   server, one bounded operation per call (N3). No shell performs file I/O. Exec Pods of the
   workspace are pinned to the keeper's node by required node affinity on
   `kubernetes.io/hostname`, read from the keeper's own `spec.nodeName`.
6. **Execs.** The Pod is rendered as design 28 renders one, under `restricted`: no token, no host
   namespace, `restartPolicy: Never`, read-only root, `drop: [ALL]`, `RuntimeDefault` seccomp,
   requests equal to limits, `activeDeadlineSeconds` from the wall bound, the operator's exec image
   by digest (N4), the workspace PVC at `/workspace` as the only writable mount. The executor is the
   container's PID 1, copied from the executor image by an init container into a read-only
   `emptyDir`. It runs the argv without a shell, separates stdout and stderr (N10), reads the
   applied bounds back from its own cgroup (`pids.max`, `memory.max`, `cpu.max`) and reports them.
   Cancellation deletes the Pod with a grace of 5 s (`TERM`) or 1 s (`KILL`); `exec.signals`
   publishes exactly those two. A forced deletion by someone else is handled by design 28 § *A gone
   Pod*.
7. **Durable before every API call.**

   | operation | committed before the call | the call | committed after |
   |---|---|---|---|
   | workspace create | operation `accepted`, row `creating` with PVC and keeper names | create PVC, then keeper | PVC uid, keeper uid; `ready` once the executor handshake passes |
   | file write or delete | operation `accepted` | one `pods/exec` executor call | the observed version |
   | exec start | operation `accepted`, row with the Pod name, admitted snapshot | binding re-read (decision 9), then create Pod | Pod uid; then observations |
   | signal | the signal intent | delete with grace and `preconditions.uid` | the observed end, only once shown |
   | workspace destroy | `destroying` (design 03 V1 decision 8) | delete keeper, confirm its container stopped, delete PVC | `destroyed` once both read absent |

8. **Restart reconciliation.** After retaking the Lease the driver reads each non-terminal row's
   object by its recorded name, then:
   - a matching object with a provable state ends the operation with that state: a PVC and a ready
     keeper make a workspace `ready`; an exec Pod whose container the API shows `terminated` gives
     its exit status, and its log is read before the Pod is deleted;
   - an exec Pod still running is deleted under decision 6's rule, and the exec stays `unknown`, as
     on the host driver. Re-attaching a run across a restart is not decided here (N8);
   - an object that is absent stays `unknown` until the absence bound of N9 has passed since
     acceptance, then the operation is `failed` with no resource. Nothing is created again;
   - a collision under decision 4 stays `unknown` and is never touched.
   Thereafter the driver lists its labelled Pods and watches from the list's `resourceVersion`. A
   `410` relists. Termination is never inferred from absence, from a `Failed` phase or from a watch
   gap.
9. **Backend identity, bound into the snapshot and re-read before dispatch:** the API server URL and
   CA digest, the namespace, the Lease holder, the default-deny policy's uid, `resourceVersion` and
   spec digest, the executor and exec image digests, and the storage class. A change moves
   `config_generation`; a start whose snapshot no longer matches is `exec.capability-stale` (422)
   before the Pod is created.
10. **Startup refuses rather than degrades.** The driver does not open, and the daemon is not ready
    (design 02 V1 decision 4), when: the kubeconfig fails design 28's loader; the Lease is held
    (decision 2); the rules review shows excess (decision 3); a server-side dry run
    (`dryRun=All`) of a Pod with `privileged: true` is admitted, so the namespace does not enforce
    `restricted`; or the executor image cannot be run and answer its version handshake.
11. **Facts are observations of disposable resources, then cleaned.** The startup probe, repeated
    when `valid_until` passes, creates a labelled PVC, keeper and exec Pod, observes, and deletes
    them under decision 4's rules. A fact is published only for what it observed (N6):

    | fact | observed by |
    |---|---|
    | `workspace.guarded-io`, `workspace.openat2-beneath`, `workspace.atomic-replace` | the executor's own checks inside the keeper |
    | `exec.argv-only` | the probe argv round-trips byte for byte, `$` included |
    | the families `process-run`, `process-watch`, `output-read`, `wait`, `workspace`, `limits`, `audit` (design 30 § 4.3) | a probe run ends, its output is read, its bounds are read back from inside |
    | `confinement.whole-tree-kill` | a probe child outlives its parent and is gone once the container is shown terminated |
    | no egress (N5) | connections from inside a probe Pod fail |
    | `workspace.storage-quota` | a probe write past the PVC's size fails with `ENOSPC` or `EDQUOT` |

    It never publishes `exec.namespaces`, `exec.cgroup-limits`, `exec.cgroup-kill` or
    `exec.no-egress`: they name Linux mechanisms.
12. **Named refusals, never a weaker run.**

    | request | answer | new? |
    |---|---|---|
    | exec while the probe proved no confinement | `exec.sandbox-unavailable` (501) | existing |
    | `read_only_roots` | `exec.sandbox-unavailable` (501): `exec.read-only-roots` absent | existing |
    | `read_only_files` | `exec.read-only-files-unserved` (501) | existing |
    | a host or Git writable root | `exec.writable-roots-host-unserved`, `exec.writable-roots-git-unserved` (501) | existing |
    | `secret_slots`: a Pod spec is stored in the cluster | `exec.secret-slots-unserved` (501) | existing |
    | `aperture` or `apertures` | `exec.egress-apertures-unserved` (501) | existing |
    | `scratch` | `exec.scratch-quota-unserved` (501) | existing |
    | `workdir` | `exec.workdir-unserved` (501) | existing |
    | scoped `workspace_access` | `exec.workspace-scoped-write-unserved` (501) | existing |
    | `measurements` | `exec.metrics-unserved` (501) | existing |
    | an inline capsule | `exec.inline-capsule-unserved` (501) | **new** |
    | a raw-pipe or PTY session | `session.unserved`, `session.pty-unserved` (501) | existing |
    | the `process.*` family | `process.host-unserved` (501) | existing |
    | a Git or bundle workspace source | `workspace.git-unserved`, `workspace.source-unserved` (501) | existing |
    | a storage quota the probe did not observe enforced | `workspace.storage-quota-unserved` (501) | existing |
    | a create refused by the namespace's ResourceQuota | `workspace.capacity`, `exec.capacity` (429, `exhausted`) | existing |
    | a member naming a Pod field | `request.schema-invalid` (422) | existing |

    The order is the daemon's existing check order. The new code, the widened `driver` (N7) and the
    neutral facts (N6) arrive in a successor bundle with a vector each, cut by
    `cargo xtask render-bundle`, its check added to `scripts/gate.sh` in the same change.
13. **Conformance, on a disposable kind cluster with its own kubeconfig only** (N11). The cluster is
    created by the lane with `kind create cluster --kubeconfig <lane file>`, as the `pod-lane` job
    does. The driver's credential is a namespaced ServiceAccount bound to exactly decision 3's Role,
    in a namespace labelled `pod-security.kubernetes.io/enforce: restricted` with the default-deny
    policy installed. No lane reads `~/.kube/config` or any ambient context. It must pass:
    - **shared, from the wire:** the clean-room runner of design 31, served over TLS with the lane's
      Identity stub as the chart lane is. Its core groups (machine, TLS, auth, workspace, recovery,
      cleanup) must pass. The exec, events and restart groups must pass where their facts hold.
      Sessions, resources and retention are `absent` with `fact-absent`, never passed;
    - **shared, from the vectors:** the route and refusal probes of
      `crates/substrate-daemon/tests/runtime_vectors.rs`, run against a daemon with this driver,
      selected by facts, with absent cases not counted;
    - **driver-specific, once each:** a same-named object of another deployment, and one with a
      foreign uid, are left untouched; with any one verb removed from the Role the dependent fact is
      absent and its refusal named; a `privileged` dry run admitted refuses startup; a running exec
      across a daemon restart reads `unknown` and its Pod is gone; a force-deleted Pod is not
      reported ended before its container is shown stopped; after cleanup, no labelled object
      remains in the namespace and the admin kubeconfig sees no cluster-scoped object the run added.

## Decisions for the operator

### N1 — where the daemon runs and how it authenticates

| option | what it does | what it costs |
|---|---|---|
| **A** design 28's loader: one operator-written, self-contained JSON kubeconfig naming the namespace, with every refusal design 28 lists | one hardened code path, already tested on the pod lane; the daemon can run in or out of the cluster | the token is as long-lived as the operator makes it |
| **B** in-cluster only, from the projected ServiceAccount token, re-read before each call | short-lived, kubelet-rotated tokens | a second loader, and a file path design 28 refuses (`tokenFile`); the lane must run the daemon in-cluster |
| **C** both | each deployment picks | two loaders to keep equally strict |

**Recommendation: A** for the first slice. B is a labelled later milestone for in-cluster hosting.

### N2 — how a daemon claims its namespace

| option | what it does | what it costs |
|---|---|---|
| **A** a coordination Lease in the namespace, holder `<deployment>/<nonce>` | two daemons cannot share a namespace and split one set of objects between two ledgers | three verbs on `leases`; a crashed holder blocks a successor for one Lease duration |
| **B** a namespace annotation | visible to an operator | patching a namespace is a cluster-scoped grant |
| **C** the operator's word | nothing | a second daemon on the same namespace corrupts both ledgers silently |

**Recommendation: A.**

### N3 — how workspace file operations reach the PVC

| option | what it does | what it costs |
|---|---|---|
| **A** a keeper Pod per workspace runs the executor; the daemon calls it through `pods/exec` on the API server | no Pod-network path from the daemon, no certificates; one keeper per workspace | `create` on `pods/exec` lets the credential run a command in any Pod of the namespace, which `create` on `pods` already allows; exec Pods are pinned to the keeper's node |
| **B** the keeper serves mutual TLS on the Pod network | no `pods/exec` grant | per-keeper certificates, an ingress rule in the default-deny policy, and Pod-network reach from the daemon |
| **C** a one-shot executor Pod per file operation | no long-lived Pod | seconds of Pod start per file read |

**Recommendation: A.** Whether two Pods on one node may mount one `ReadWriteOnce` claim depends on
the storage driver; the probe observes it, and without it exec facts are absent.

### N4 — where the executor and the exec image come from

| option | what it does | what it costs |
|---|---|---|
| **A** a fourth release image, `ghcr.io/beyond10x/b10x-substrate-executor`, signed as ADR 0006 signs the bundle; the operator declares it and one exec image, each by digest; no request names an image | the executor ships with the daemon that speaks to it, checked by a version handshake | one more release artifact and its preflight |
| **B** the executor inside the daemon image, run with another entrypoint | no new artifact | the daemon image is larger than the executor needs, and every keeper pulls it |
| **C** the request names the exec image | per-request toolchains | a wire field, and an image choice that is the product's, not Substrate's |

**Recommendation: A.**

### N5 — how "no egress" becomes a fact

| option | what it does | what it costs |
|---|---|---|
| **A** the operator installs a namespace-wide default-deny NetworkPolicy (`podSelector: {}`, both directions, no rule); the driver only reads it, binds its uid and spec into the snapshot, and the probe observes from inside a Pod that connections to the API server's cluster address, to cluster DNS and to one operator-declared outside address all fail | a fact from an observation, re-read before every dispatch | an observation at probe time, not a proof for all time; a plugin that stops enforcing between probes is unseen until the next one |
| **B** per-run policies, as design 28 | the adapter's existing code | the D173 dance for every run, and no stronger claim |
| **C** claim nothing and refuse every exec | honest | the driver serves no execution, so the track cannot close |

**Recommendation: A**, published as a new neutral property, `confinement.no-egress`, beside design
30's set. Its meaning: a connection from inside a run fails. Design 15 § 1 accepts the same
observation for containers.

### N6 — which facts the daemon admits an exec on

| option | what it does | what it costs |
|---|---|---|
| **A** take design 30's neutral facts, and admit an exec on either the four Linux facts or the neutral set: families `process-run`, `wait`, `output-read`, `limits`, and properties `whole-tree-kill` and `no-egress` | one admission rule, no released meaning changed | this ADR waits for design 30's acceptance and bundle |
| **B** this driver cuts its own neutral facts first | no wait | two designs naming the same properties |
| **C** publish the Linux facts from Kubernetes observations | no new facts | rewrites released meanings (invariant 6) |

**Recommendation: A.** Sequence design 30 first.

### N7 — what `driver` says

| option | what it does | what it costs |
|---|---|---|
| **A** `HostDriverKind` gains `kubernetes`; the schema's `const` becomes an enum in the successor bundle | provenance stays truthful: design 30 § 4.1 fixes `host` to mean this machine | an exhaustive `match` in a consumer stops compiling; the SDK's `Machine` has no `driver`, and mantle's `host` fixtures stay valid |
| **B** keep `host` | no wire change | contradicts the meaning design 30 fixes |
| **C** make `driver` optional | no new value | removes provenance an auditor reads |

**Recommendation: A.** Clients still choose by facts (invariant 4); design 31's runner already
records `driver` and selects on nothing but facts.

### N8 — an exec still running when the daemon restarts

| option | what it does | what it costs |
|---|---|---|
| **A** delete its Pod; the exec stays `unknown` | the host driver's contract and design 31's restart case hold unchanged | work that could have finished is ended |
| **B** re-attach: keep watching the Pod and end the exec when it ends | no work lost | the contract change design 31 § 4 names, with its own design and a moved restart case |

**Recommendation: A**, with B a labelled later milestone.

### N9 — when an unanswered create's absence becomes authoritative

| option | what it does | what it costs |
|---|---|---|
| **A** after acceptance plus the driver's 30 s client timeout plus the API server's request timeout (default 60 s, operator-stated when different), an absent object makes the operation `failed` with no resource | a client learns the answer within minutes | correct only while the stated request timeout is the server's |
| **B** never: absence stays `unknown` until a lease expires | no assumption about the server | an exec without a lease stays `unknown` forever |

**Recommendation: A.** Nothing is ever created again either way.

### N10 — separate stdout and stderr

| option | what it does | what it costs |
|---|---|---|
| **A** the executor, as PID 1, frames each chunk with its stream and length on the container log; the daemon reads `pods/log` and splits it | exact streams through the existing output route | the output bound must not exceed the kubelet's `containerLogMaxSize`, which the operator states, as design 28 takes `podPidsLimit` |
| **B** report the merged log as `stdout`, as the `pod` adapter does | no framing | `stderr` reads empty while the run wrote to it: an observation that is not true on the wire |

**Recommendation: A.** The driver publishes `exec.output-limit-bytes` no larger than the stated log
size, and the executor truncates and marks output past it, as the host driver does.

### N11 — the lane

| option | what it does | what it costs |
|---|---|---|
| **A** a workflow of its own, `namespace-lane.yml`, on pull requests touching the driver, the executor, `Dockerfile` or the wire crate, on pushes to `main` and on dispatch; `release.yml` refuses a tag without its success, as ADR 0046 K9 B does for the chart lane | `gate.yml` grows by zero | a third required release signal |
| **B** extend the `pod-lane` job | one cluster | every pull request pays an image build and the driver's cases |
| **C** a recorded local run, `Namespace-lane:` in the annotated tag | no hosted cluster time | evidence from a workstation, and a fourth tag record format |

**Recommendation: A**, falling back to C for any case a hosted runner cannot execute.

### N12 — where the code lives

| option | what it does | what it costs |
|---|---|---|
| **A** a crate `substrate-kubernetes` implementing `Driver`; the API client and kubeconfig loader move out of `pod.rs` into a module both use | `architecture/dependency-rules.md`'s "host / Docker / Kubernetes adapters ──▶ substrate driver ports" holds; one client to harden | a move of tested code before the slice |
| **B** inside `substrate-host` beside the pod adapter | no new crate | the host crate grows a second backend |

**Recommendation: A.** `cargo xtask check-packages` gains the crate in `SOURCE_PACKAGES`.

## Consequences

- **Sequence.** This ADR, then design 30's acceptance and bundle (N6), then the successor bundle of
  decision 12, then `story:kubernetes-namespace-workspace-and-exec-slice`. The story's existing
  dependencies stand: the node-bound profile, remote clean-room conformance and the driver-port
  story.
- **Wire.** One successor bundle: `driver` widened (N7), `confinement.no-egress` (N5),
  `exec.inline-capsule-unserved`, each with a vector. No route changes. Earlier bundles keep their
  bytes.
- **New surfaces.** The driver crate (N12), the executor binary and image (N4), the lane (N11) with
  a Rust runner (an `xtask` verb), and the release preflight for the new image and lane. AGENTS.md
  § *Releases* names them when they land.
- **Weaker than the host driver, by name.** No sealed secret slots, apertures, host roots, writable
  roots, capsules, sessions, working directory, scoped writes or resource usage in the first slice.
  A client that needs one is answered by its refusal. Each can return later only as its own decision
  with a probe.
- **Unobserved, to be observed on the lane before the slice relies on it:** that background garbage
  collection removes a PVC's dependent Pods under the `pvc-protection` finalizer; that `dryRun=All`
  runs Pod Security admission on the lane's API server; that kind's default network plugin enforces
  NetworkPolicy (if not, N5's fact is absent there and exec cases are `absent`, so the lane must
  install an enforcing plugin to pass the exec group); and that two Pods on one node can mount one
  `ReadWriteOnce` claim with the lane's storage class.
- **No atlas ADR.** The driver stays inside Substrate's ownership (the execution data plane and its
  drivers) and renames no wire-visible identifier. Atlas ADR 0109 governs adapters, which this is
  not. Atlas's O1 evidence is recorded when the track closes.

## What this does not decide

- Re-attaching runs across a restart (N8 B), sessions through `pods/attach`, capsules on a PVC,
  egress apertures, and resource usage from the metrics API.
- Workloads, images, volumes and endpoints as Kubernetes objects (design 01 § 8's later column).
- More than one namespace per daemon, scheduling across clusters, and product quotas.
- In-cluster token loading (N1 B), and how a deployment packages this driver: a chart profile beside
  ADR 0046's or its own.
