# Design 34: the Kubernetes namespace driver, implemented

Status: accepted for implementation, 2026-10-29, with § 14's answers (Q1 B, Q2 A with the O11
observation, Q3 A; `approval-record:design-34-accepted`). Owned by `story:kubernetes-namespace-workspace-and-exec-slice`
(`epic:kubernetes-deployment-and-driver`). It implements
[ADR 0047](../../adr/0047-a-kubernetes-namespace-driver-serves-workspaces-on-pvcs-and-execs-as-pods.md),
accepted 2026-10-29 with N1 to N12 as recommended. Invariant 8 requires this design before code.
It needs one successor bundle (§ 11), which needs its own design section before it is cut.

ADR 0047 decides *what* the driver may do. This note decides *how*: crates, the trait
implementation, the ledger records, the probe, the executor protocol, the lane, and the order of
work. Where ADR 0047 is silent and the choice is reversible, this note takes it (§ 13). § 14 holds
only questions the ADR does not settle.

## 1. What exists

| piece | where | what this design takes from it |
|---|---|---|
| `Driver` trait, `DriverError`, `DispatchOutcome`, `Listed` | `crates/substrate-host/src/lib.rs` | the port the driver implements; `App.driver` is `Arc<dyn Driver>` (`crates/substrate-daemon/src/app/service.rs`) |
| host driver construction | `crates/substrate-daemon/src/runtime.rs` (`HostDriver::open(host_config)`) | the one place a second driver is selected |
| exec admission | `crates/substrate-daemon/src/app/operations.rs`: snapshot check, then field checks, then the four Linux facts as the last check | where N6 widens the final check |
| restart reconciliation | `reconcile_provisional_workspaces`, `reconcile_provisional_execs` (`service.rs`); `reconcile_after_restart` (`crates/substrate-store/src/recovery.rs`) | the loop the driver answers through `observe_workspace` and `observe_exec` |
| the `pod` adapter's client | `crates/substrate-host/src/confinement/pod.rs`: `parse_cluster`, `settings`, `Api`, `connector`, `PodTrace`, `watch_event`, `replay`, `pod_end`, `settle`, `never_bound` | the kubeconfig loader, the HTTPS client and the gone-Pod rule of [design 28](28-pod-confinement-adapter.md) § *A gone Pod* |
| guarded workspace I/O | `crates/substrate-host/src/fs.rs` (`GuardedFilesystem`); its only crate imports are `DriverError` and `Listed` | the executor's file operations (ADR 0047 decision 5) |
| environment rule | `crates/substrate-host/src/environment.rs` (`check`) and `baseline_value` in `process.rs` | the exact environment a run receives |
| pinned cluster tooling | `pod-lane` in `.github/workflows/gate.yml`, `.github/workflows/chart-lane.yml` | kind `v0.33.0`, `kindest/node:v1.37.0`, the busybox digest of `crates/substrate-host/tests/ess_confinement_pod.rs` |
| driver port fence | `crates/substrate-daemon/tests/driver_port.rs` | the daemon names only the port from `substrate_host` |

Design 30 is still proposed. ADR 0047 N6 sequences its acceptance and bundle before this driver
serves an exec (§ 11, § 12).

## 2. Crate layout and the client extraction

N12 A puts the driver in `substrate-kubernetes` and the client in "a module both use". One crate
cannot hold both: the `pod` adapter in `substrate-host` would depend on it for the client, and the
driver depends on `substrate-host` for `Driver`. Cargo refuses that cycle. The executor (N4) must
also run inside an exec image the operator chooses, so it cannot link `substrate-host`'s Git,
curl and OpenSSL stack. Four packages follow:

| directory | package | depends on (workspace) | holds |
|---|---|---|---|
| `crates/substrate-kubernetes-api` | `b10x-substrate-kubernetes-api` | none | design 28's kubeconfig loader and refusals; the HTTPS client (`Api`), bounded bodies, paging, metadata lists, watch streams, `DELETE` with `preconditions.uid`, `dryRun=All`, `pods/log`, `pods/exec` over WebSocket `v5.channel.k8s.io`, `Lease` read and update, `SelfSubjectRulesReview`; the gone-Pod evidence (`PodTrace`, `watch_event`, `replay`, `never_bound`) |
| `crates/substrate-guarded` | `b10x-substrate-guarded` | `substrate-wire` | `fs.rs` moved whole; `DriverError`, `DriverErrorClass` and `Listed` moved with it and re-exported by `substrate-host`, so every existing path compiles; the environment rule (`check`, `baseline_value`) |
| `crates/substrate-executor` | `b10x-substrate-executor`, binary `substrate-executor` | `substrate-wire`, `substrate-guarded` | the protocol types and frame codec (lib); the executor (bin) |
| `crates/substrate-kubernetes` | `b10x-substrate-kubernetes` | `substrate-host`, `substrate-kubernetes-api`, `substrate-executor` (lib), `substrate-wire` | `NamespaceDriver: Driver` |

`substrate-host` gains edges to `substrate-kubernetes-api` and `substrate-guarded`.
`substrate-daemon` gains an edge to `substrate-kubernetes`. The extraction is a move: `pod.rs` keeps
its constants, its rendering and its run task, and calls the client. Its golden manifest
(`crates/substrate-host/tests/golden/pod-manifest.json`), its unit tests and the pod lane stay
byte-identical in what they assert.

`pods/exec` uses `tokio-tungstenite`, which `Cargo.lock` already carries for the SDK. No new
third-party package, so `THIRD_PARTY_LICENSES.html` changes only if a feature does.

`cargo xtask check-packages` gains:

- the four packages in `SOURCE_PACKAGES` (`xtask/src/packages.rs`), each with README, inherited
  SPDX metadata and a documentation target. `substrate-kubernetes-api` and `substrate-guarded`
  name `website/docs/concepts/adapters.md`, and the other two name `website/docs/guides/deployment.md`,
  which gains its section in U10;
- a `cargo tree --locked -p b10x-substrate-executor -e normal` rule refusing any line naming
  `b10x-substrate-host`, `-store`, `-daemon`, `-kubernetes-api`, `rustls` or `gix`;
- a refusal of the `fake` feature of `b10x-substrate-kubernetes-api` on any `[dependencies]` edge
  (§ 10.1). It is legal only under `[dev-dependencies]`.

`crates/substrate-daemon/tests/driver_port.rs` stays as it is: `NamespaceDriver` is named only in
`runtime.rs`, the composition root, as `HostDriver` is. Its allowlist gains the `BackendLedger`
port of § 4.

The executor is built static for `x86_64-unknown-linux-musl` (§ 13 D2). `rust-toolchain.toml` adds
that target, and a `Dockerfile` stage `executor-builder` installs `musl-tools`. The target
`executor` is `FROM scratch` with `/substrate-executor` and the licence notices.

## 3. What the driver serves

The daemon selects the driver with `--driver kubernetes --kubernetes-config <file>` (design 02:
configuration, never a request). `--driver` defaults to `host`. The host-only flags (`--workspaces`,
`--cgroup-root`, `--bubblewrap`, host-process flags) conflict with `kubernetes` in `clap`.
`--deployment` is required with `kubernetes`.

The config file is design 28's self-contained JSON kubeconfig, through the moved loader with every
refusal design 28 lists (N1 A). The driver reads one context extension,
`b10x-substrate/namespace-driver`, a closed object. An unknown member is refused at open.

| member | meaning | required |
|---|---|---|
| `storage_class` | the PVC storage class | yes |
| `workspace_size_bytes` | every PVC's size, 1 MiB to 1 TiB | yes |
| `executor_image`, `exec_image` | `…@sha256:<64 lowercase hex>`; a tag is refused | yes |
| `default_deny_policy` | name of the operator's NetworkPolicy (N5) | yes |
| `egress_probe_address` | `ip:port` outside the cluster that the no-egress probe dials | yes |
| `pod_pids_limit` | the kubelet's `podPidsLimit`, stated as in design 28 | yes |
| `container_log_max_bytes` | the kubelet's `containerLogMaxSize`, stated (N10) | yes |
| `run_as_user` | numeric uid, default 65532 | no |
| `api_request_timeout_seconds` | the API server's request timeout, default 60 (N9) | no |
| `max_current_workspaces`, `max_current_execs` | published bounds, defaults 64 and 32 | no |
| `probe_interval_seconds` | probe period and `valid_until` window, default 600, 60 to 3600 | no |

### Routes

| route family | first slice | later |
|---|---|---|
| `machine.get`, `operation.get`, `event.list`, `event.stream`, `reconciliation.snapshot.*`, `*.lease.renew` | served (daemon-side) | — |
| `workspace.create` (empty source), `.get`, `.destroy` | served: PVC and keeper | request-sized PVCs, once `workspace.storage-quota` can be observed (§ 6) |
| `workspace.file.read`, `.write`, `.delete`; `workspace.file.read-v2`, `.tree.read-v2`, `.file.replace-v2`, `.file.edit-v2`, `.file.patch-v2` | served: one executor call each through `pods/exec` | — |
| `workspace.adopt` | refused (§ 14 Q3) | an operator PVC as an adopted workspace, its own decision |
| `workspace.git.*` | `workspace.git-unserved` (501), the trait default | Git on a PVC |
| `exec.start`, `.get`, `.output.get`, `.signal`, `.retire` | served: one Pod per exec | re-attach across restart (N8 B) |
| `session.*` | `session.unserved` / `session.pty-unserved` (501), the trait defaults | `pods/attach` sessions |
| `metrics.get`, `metrics.stream` | absent `exec.resource-usage`; the existing refusal | metrics API usage |
| `process.*` | `process.host-unserved` (501) | never: the host-process family is the host's |

## 4. Durable records: the backend ledger

ADR 0047 decision 4 says "the ledger row holds the name before the API call" and "the uid the
create answered is committed as the backend identity". The store has no column for either:
`execs.cgroup` is wire-visible (`AppliedConfinement.cgroup`) and must not carry a Pod uid. So:

- **Store.** A table `backend_objects (deployment, resource_kind, resource_id, role, name,
  operation_digest, uid NULL, created_at, settled_at NULL)`, unique on `(deployment, name)`.
  `resource_kind` is `workspace`, `exec` or `probe`. `role` is `pvc`, `keeper` or `pod`.
  It is an additive migration in `crates/substrate-store/src/schema.rs`.
- **Names are reserved by the daemon, inside the transaction that commits `accepted`.** A new
  trait method `fn backend_names(&self, kind, id) -> Vec<BackendName>` defaults to empty, so the host
  driver writes nothing and changes nothing. The daemon computes `operation_digest =
  sha256(subject ‖ operation id)` (length-prefixed). The driver never sees the subject.
- **Port.** `trait BackendLedger` beside `Driver` in `substrate-host`, implemented by the daemon over
  the store and handed to the driver at open: `recorded(kind, id)`, `bind_uid(name, uid)` (refuses
  a different uid already bound: a collision), `settle(name)` (the object read absent; the row is
  closed), `reserve_probe(names)` and `unsettled()`. Each call is one SQLite transaction, committed
  before it returns.
- **Workspace root name** is `workspace_root_identity(id)` = `sbx-ws-<h>`, already committed before
  dispatch by the existing path, so the PVC name needs no new mechanism.

### Names, labels and hashing

`<h>` is the first 26 characters of lowercase RFC 4648 base32 (no padding) of SHA-256 over
`"substrate-kubernetes-name\0" ‖ u32be(len) ‖ deployment ‖ u32be(len) ‖ resource id`. That is
130 bits, and `sbx-ws-<h>-keeper` is 39 characters, within a DNS-1123 label. Probe objects are
`sbx-pr-<nonce>-{pvc,keeper,pod}`, with a 16-character base32 nonce.

| key | value |
|---|---|
| label `app.kubernetes.io/managed-by` | `b10x-substrate` |
| label `b10x-substrate/deployment` | 26-character base32 of SHA-256 of the deployment |
| label `b10x-substrate/workspace` or `b10x-substrate/exec` or `b10x-substrate/probe` | `<h>` or the nonce |
| label `b10x-substrate/role` | `pvc`, `keeper`, `pod` |
| annotation `b10x-substrate/operation` | `sha256:<hex>` of `operation_digest` (probe: of the nonce) |

An object is **ours** when its name has a ledger row, its deployment label is ours and either its
uid equals the bound uid, or no uid is bound and its operation annotation equals the row's. The
second case adopts the uid of a create whose answer was lost. Anything else is a collision
(decision 4): logged, counted as `substrate_kubernetes_collisions_total` on the operational listener
(an addition to ADR 0045's closed catalogue, operator-only), never modified, and the row stays
`unknown`.

## 5. Ledger states and restart reconciliation, per resource

"Bound" means `bind_uid` committed. "Shown stopped" is design 28's gone-Pod rule, moved into the
client crate.

| resource | before the call | the calls | after | restart reconciliation |
|---|---|---|---|---|
| workspace create | op `accepted`; workspace `creating`; rows `pvc`, `keeper` | POST PVC; bind; POST keeper (ownerReference to the PVC uid, `controller: false`); bind; `pods/exec` handshake (§ 7.2) | `ready` within the create bound (60 s), else `OutcomeUnknown` and the provisional loop completes it | PVC and keeper ours and the handshake passes: `ready`. PVC ours, keeper row unbound and absent: still `unknown`; past the N9 bound the operation is `failed`, the PVC is deleted with its uid precondition, and nothing is created again |
| file write, delete, replace, edit, patch | op `accepted` | one `pods/exec` call | the observed version | an unanswered call is followed by one `stat` call: if the content digest equals the request's, that observation is returned; otherwise `Failed` `kubernetes.executor-unanswered` (retriable). Nothing is resent |
| exec start | op `accepted`; exec row; row `pod`; admitted snapshot | Lease check and binding re-read (§ 6.3); POST Pod; bind | running observation | Pod ours and container shown `terminated`: read the log, commit the exit, delete the Pod. Ours and running: delete (grace 1 s), stay `unknown` (N8 A). Unbound and absent past the N9 bound: `failed` with no resource (§ 5.1) |
| signal | the signal intent | `DELETE` with grace 5 s (`term`) or 1 s (`kill`) and `preconditions.uid` | the end, once shown stopped | an intent without a shown end is re-delivered as the same delete; the exec stays `unknown` until shown |
| workspace destroy | `destroying` (design 03 V1 decision 8) | delete every exec Pod of the workspace; delete the keeper; confirm stopped; delete the PVC | `destroyed` once each reads absent; `WorkspaceDestroyProgress::Pending` until then | resumed by `reconcile_destroying_workspaces` |
| probe objects | rows `probe` via `reserve_probe` | create, observe, delete (§ 6.2) | settled when absent | every unsettled probe row is deleted under the uid rule before a new probe starts |
| garbage | — | each maintenance round deletes ours-and-present objects of rows whose resource is terminal, retired or destroyed | settled when absent | the same |

### 5.1 Dispatch absence, told apart from physical absence

Today `NotFound` on an exec after restart means only "no process now" (`mark_exec_physically_absent`
keeps `unknown`). Under N9 an exec whose Pod was **never stored** ends `failed`. The driver returns
`NotFound` in both cases. The daemon tells them apart by the backend ledger. If the exec has a `pod`
row with no uid bound, absence past the bound is dispatch absence (`complete_dispatch_absence` with
kind `exec`). A bound uid means physical absence, as today. The host driver writes no rows, so its
behaviour is unchanged. The N9 bound is acceptance + 30 s client timeout +
`api_request_timeout_seconds`, read from the API server's `Date` header against the row's
`created_at`.

### 5.2 Watching

After reconciliation the driver lists its Pods (label `managed-by` and deployment, metadata only, 100
per page, 4 MiB per page, at most 64 pages) and watches from the list's `resourceVersion`. A 410, an
`ERROR` event or a dropped stream relists and re-reads every non-terminal exec by name. A terminal
observation needs the container `terminated` in a Pod with `restartPolicy: Never` and the bound uid.
Absence, a `Failed` phase and a watch gap never count as termination.

## 6. Startup, the probe and the facts

### 6.1 Open sequence

Each step's failure leaves the driver unopened. The daemon serves its listeners with every driver
fact absent, and `/readyz` (ADR 0045) reports the reason code. These codes are log and readiness
reasons, not wire refusals.

1. Load the kubeconfig and extension: `kubernetes.config-invalid`.
2. `SelfSubjectRulesReview` for the namespace. It is not a stored object, so it creates no
   cluster-scoped resource. Allowed: decision 3's table plus the fixed set every authenticated
   subject holds in a conformant cluster (`system:basic-user`, `system:discovery`,
   `system:public-info-viewer`), pinned in code and checked by O8. Any other rule, a `*`, or any verb
   on `secrets`: `kubernetes.rbac-excess`. `incomplete: true` is logged and counted. Decision 3
   refuses only what is visible.
3. Take the Lease `b10x-substrate-driver`, holder `<deployment>/<nonce>`, `leaseDurationSeconds: 30`.
   A Lease is taken over only when its `resourceVersion` has not changed for 30 s on this process's
   monotonic clock (client-go's rule, immune to clock skew). It is renewed every 10 s and **held**
   while the last successful renewal is under 20 s old. Held by another deployment:
   `kubernetes.lease-foreign`. Held by a live instance: `kubernetes.lease-held`. Both are retried
   every 10 s.
4. Read `default_deny_policy`. It must be `podSelector: {}`, `policyTypes` both, no rules:
   `kubernetes.policy-not-default-deny`.
5. `dryRun=All` create of a Pod with `privileged: true`. If it is admitted:
   `kubernetes.pod-security-unenforced`. This depends on O2.
6. Reconcile (§ 5), then list and watch (§ 5.2).
7. Probe (§ 6.2). Its keeper performs the executor handshake: `kubernetes.executor-handshake-failed`.

A lost Lease while serving makes every fact absent and refuses a start `exec.capability-stale` (422)
until the Lease is retaken (decision 2). Reconciliation pauses. Retaking it runs steps 4 to 7 again.

### 6.2 The probe

Repeated every `probe_interval_seconds`. `valid_until` is the next run plus 60 s. A failed or late
probe makes the facts it would have renewed absent, never stale. One PVC, one keeper and one exec
Pod, created like a workspace and an exec but with probe rows, then deleted under the uid rule:

| observation | fact |
|---|---|
| `substrate-executor fs check` in the keeper: `openat2` with `RESOLVE_BENEATH|RESOLVE_NO_SYMLINKS|RESOLVE_NO_MAGICLINKS|RESOLVE_NO_XDEV`, a symlink escape refused, `renameat2` replacement | `workspace.guarded-io`, `workspace.openat2-beneath`, `workspace.atomic-replace` |
| the probe Pod runs the executor's `probe-child`, which writes its argv as JSON; the argv holds `$(HOME)`, `$$`, a space and a non-ASCII byte; byte-exact | `exec.argv-only` |
| the Pod ends, its log frames decode, and its `applied` record matches the rendered memory and CPU | `confinement.family.process-run`, `.process-watch`, `.output-read`, `.wait`, `.limits`, `.audit`, `.workspace` (design 30 § 4.3) |
| `probe-child` forks a grandchild that writes a heartbeat to the PVC every 100 ms and exits; after the container is shown `terminated`, the keeper's `fs stat` sees the heartbeat unchanged for 2 s | `confinement.whole-tree-kill` |
| from inside: TCP to `KUBERNETES_SERVICE_HOST:443`, a DNS query to `/etc/resolv.conf`'s nameserver, TCP to `egress_probe_address`; each must fail within 3 s | `confinement.no-egress` (N5) |
| — | never: `exec.namespaces`, `exec.cgroup-limits`, `exec.cgroup-kill`, `exec.no-egress` |

Declared bounds are published as configuration, as the host publishes them: `workspace.max-current`,
`workspace.max-file-bytes`, `workspace.read-limit-bytes`, `workspace.list-limit-items`,
`exec.max-current`, `exec.signals = [term, kill]`, and `exec.output-limit-bytes` (§ 7.3).

**`workspace.storage-quota` stays absent.** The wire fact states `max_bytes` *and* `max_inodes`
(`StorageQuotaFacts`), and a PVC's filesystem fixes its inode count by size. The probe can observe
`ENOSPC` past the size, but not a requested inode bound. A request quota is therefore
`workspace.storage-quota-unserved` (501), as ADR 0047 decision 12 requires for a quota not observed
enforced, and every PVC is `workspace_size_bytes`.

### 6.3 Backend identity and the snapshot

The snapshot digest (design 02 V1 decision 5, design 30 § 4.6) takes, beside the facts: the API
server URL and the SHA-256 of its CA data, the namespace, the Lease holder, the policy's uid,
`resourceVersion` and SHA-256 of its canonical `spec`, both image digests and the storage class. A
change moves `config_generation`. Before each Pod create, `start_exec` checks that the Lease is held,
re-reads the policy (one `GET`) and compares the binding with the admitted snapshot. A mismatch is
`NotDispatched(exec.capability-stale)` (422), recorded as refused.

### 6.4 Admission

N6 A widens the last check in `operations.rs`. An exec is admitted on the four Linux facts **or** on
`confinement.family.process-run`, `.wait`, `.output-read`, `.limits`, `confinement.whole-tree-kill`
and `confinement.no-egress`. Otherwise `exec.sandbox-unavailable` (501), unchanged. The new capsule
check comes **after** it, so the host driver's answers are unchanged: on the portable lane a capsule
still hears `exec.sandbox-unavailable` first.

## 7. The executor

One static binary, three roles. Every role refuses an unknown subcommand and exits 2.

### 7.1 Version handshake

`substrate-executor version` writes one JSON line `{"protocol":"substrate-executor/1","version":"<crate
version>","source":"<git sha or \"unknown\">"}`. The driver requires `protocol` equal and `version`
equal to its own. A daemon talks only to the executor of its own release (N4 A).

### 7.2 Keeper and file operations (N3 A)

The keeper Pod runs the executor image. Its PID 1 is `substrate-executor keeper`, which reaps
children and exits on `TERM`. It mounts the PVC at `/workspace`. It is rendered like an exec Pod
(§ 7.3) without the init container. The daemon calls `pods/exec` on container `keeper` with
`command=/substrate-executor&command=fs`, `stdin`, `stdout`, `stderr`, and `v5.channel.k8s.io`.

- **Request (stdin, then close channel 255):** `u32be` header length (≤ 64 KiB), a closed JSON
  header `{"op": "read"|"write"|"delete"|"read-v2"|"tree-v2"|"replace-v2"|"edit-v2"|"patch-v2"|"stat"|"check", ...}`
  with the wire crate's own query and input types, then `u64be` content length (≤
  `workspace.max-file-bytes`) and the content.
- **Response (stdout):** the same framing. The header is `{"ok": <wire observation>}` or `{"error":
  {"class","code","message","address","retriable"}}`. The error is the `DriverError` the guarded code
  returned. Its `code` maps back to the `&'static str` through a closed table built from the
  constants the guarded crate uses. An unknown code is `Failed` `kubernetes.executor-protocol`.
- **Bounds:** one call per operation, 30 s, response ≤ header 64 KiB + `workspace.read-limit-bytes`.
  Channel 3's status must be `Success`. Anything else is `Failed`. stderr is drained and discarded.
- No shell performs file I/O (story acceptance). The guarded root is `/workspace`, the PVC itself.

### 7.3 Exec Pods and stream tagging (N10 A)

The exec Pod is design 28's rendering with these differences:

| field | value |
|---|---|
| `metadata.name` | `sbx-ex-<h>` |
| `ownerReferences` | the workspace PVC by uid, `controller: false`, no `blockOwnerDeletion` |
| `affinity.nodeAffinity` | required `kubernetes.io/hostname In [<keeper spec.nodeName>]` |
| init container `executor` | `executor_image`; `substrate-executor install /x/substrate-executor`; writes an `emptyDir` (`sizeLimit` 64 MiB) |
| container `run` | `exec_image`; `command` `["/.substrate/substrate-executor", "run", <spec parts>]`; **no `env`**; `/workspace` (the PVC, the only writable mount) and `/.substrate` (the `emptyDir`, `readOnly: true`) |
| `terminationGracePeriodSeconds` | 5 |
| `resources` | requests equal limits: memory, CPU at design 28's rate, `ephemeral-storage` = 64 MiB + 2 × `container_log_max_bytes` |

**The run spec** is a closed JSON object `{argv, env, memory_bytes, cpu_millicores, output_bytes}`,
base64url-encoded and split into parts of at most 96 KiB (Linux refuses one argument over
`MAX_ARG_STRLEN`, 128 KiB). base64url has no `$`, so kubelet's `$(VAR)` expansion cannot touch it.
`env` is the final set the driver computed with the host rule (`allow` names at `baseline_value`, then
`set`). The executor clears its own environment, which holds kubelet's `KUBERNETES_*` and the image's
`PATH`, and gives the child exactly `env`.

**Run:**

1. Read `memory.max` and `cpu.max` from `/sys/fs/cgroup`, and `pids.max` (see O6, § 14 Q2). Write
   the `applied` record. A value that differs from the spec means the argv is **not** started: the
   executor writes `{"refused":"limits-unapplied"}` and exits 125. The driver records the exec
   `unknown`, counts it, and fabricates no exit.
2. Spawn the argv in a new process group with stdout and stderr as two pipes, and stdin `/dev/null`.
3. Drain both pipes continuously, so the child never blocks on output. Each chunk of at most
   6144 bytes becomes one log line `b10x1 o <base64>` or `b10x1 e <base64>`. Lines stay below the
   CRI log line limit (16 KiB), so the kubelet never splits one. Past `output_bytes` per stream the
   executor writes `b10x1 c {"truncated":"o"|"e"}` once and discards the rest, as the host driver
   truncates.
4. On `TERM` it forwards `TERM` to the child's process group.
5. When the direct child ends it writes `b10x1 c {"exit":{"code":n}}` or `{"exit":{"signal":"term"|"kill"|...}}`
   and exits with the child's code, or 128 + the signal. As PID 1 its exit tears down the PID
   namespace, which is the whole-tree kill the probe observes.

Only the executor writes to the container's stdout and stderr. The child's descriptors are pipes, so
a child cannot forge a frame.

**Reading output.** `pods/log?container=run&limitBytes=<cap>`, tried five times (design 28). Lines are
decoded in order, and any line that is not a frame is a protocol failure, never output. The exit comes
from the API's `terminated.exitCode`. The `exit` record adds the signal name only when it agrees with
that code. `exec.output-limit-bytes` is the largest L with `2 × ceil(L / 6144) × 8202 + 65536 ≤
container_log_max_bytes`, capped at 1 MiB. It is therefore never more than the kubelet keeps in one
log file.

**Signals.** `term` deletes with grace 5 s and `kill` with grace 1 s, each with `preconditions.uid`.
`int` is refused (§ 14 Q3). The end is committed only once the container is shown stopped, as in
design 28.

## 8. Named refusals

The wire refusals, in the daemon's existing check order. Everything here except the rows marked
**new** exists in bundle `0.24.0` or in the code paths cited.

| request | answer | where |
|---|---|---|
| a member naming a Pod field, namespace, ServiceAccount, node, runtime class or host path | `request.schema-invalid` (422) | `deny_unknown_fields` |
| start naming another snapshot; Lease lost; binding moved | `exec.capability-stale` (422) | daemon; driver before the Pod create |
| `aperture` or `apertures` | `exec.egress-apertures-unserved` (501) | `check_egress_aperture` |
| `secret_slots` | `exec.secret-slots-unserved` (501) | `check_secret_slots` |
| `read_only_files` | `exec.read-only-files-unserved` (501) | `check_read_only_files` |
| a host or Git writable root | `exec.writable-roots-host-unserved`, `exec.writable-roots-git-unserved` (501) | `check_writable_roots` |
| `workdir` | `exec.workdir-unserved` (501) | `check_workdir` |
| `read_only_roots` | `exec.sandbox-unavailable` (501) | `check_read_only_roots` |
| `measurements` | `exec.metrics-unserved` (501) | daemon |
| `scratch` | `exec.scratch-quota-unserved` (501) | daemon |
| scoped `workspace_access` | `exec.workspace-scoped-write-unserved` (501) | existing check |
| neither confinement set proved | `exec.sandbox-unavailable` (501) | widened final check (§ 6.4) |
| an inline capsule | `exec.inline-capsule-unserved` (501), **new** | after the final check |
| `signal: int` | **new**, § 14 Q3 | driver |
| a raw-pipe or PTY session | `session.unserved`, `session.pty-unserved` (501) | trait defaults |
| `process.*` | `process.host-unserved` (501) | existing |
| a non-empty workspace source | `workspace.source-unserved` (501) | driver, as `HostDriver::create_workspace` answers |
| Git v2 routes | `workspace.git-unserved` (501) | trait defaults |
| a workspace storage quota | `workspace.storage-quota-unserved` (501) | driver (§ 6.2) |
| a create the namespace's ResourceQuota refuses | `workspace.capacity`, `exec.capacity` (429, `exhausted`) | driver: a 403 whose Status message starts with `exceeded quota:` (O7); any other 403 is `Failed` `kubernetes.api-failed` |
| `workspace.adopt` | § 14 Q3 | driver |

Failures (500, class `failed`) carry codes as the host driver's `exec.spawn-failed` does:
`kubernetes.api-failed`, `kubernetes.executor-unanswered` and `kubernetes.executor-protocol`. The
successor-bundle section confirms whether the error schema constrains failure codes.

## 9. Security envelope, clause by clause

- **Subject:** unchanged. The cluster sees only `operation_digest`.
- **Argv-only, environment cleared and shaped:** § 7.3, proved by `exec.argv-only`.
- **No egress:** the operator's default-deny policy is bound into the snapshot and observed from inside
  (N5). It is weaker than the host driver's namespace by being an observation at probe time (§ 14 Q1).
- **No nested user namespace, read-only root, no privilege:** Pod Security `restricted` enforced
  (startup step 5); `drop: [ALL]`, `RuntimeDefault` seccomp, `allowPrivilegeEscalation: false`.
- **Bounds:** memory and CPU are read back. Processes per § 14 Q2. The wall bound is
  `activeDeadlineSeconds`, plus deletion 60 s after it (design 28).
- **Whole-tree kill:** the PID namespace teardown, observed.
- **Guarded I/O:** the host driver's own `openat2` code, observed in the keeper.
- **No token in any Pod:** `automountServiceAccountToken: false`. The keeper holds none either.
- **Credentials:** the kubeconfig token never appears in an error, a log line or a record (design 28).

## 10. Tests

### 10.1 The fake API: what runs in seconds, in gate step 1

`b10x-substrate-kubernetes-api` has a module `fake`, compiled under `cfg(any(test, feature =
"fake"))`. The feature is fenced to dev-dependencies (§ 2). It is an in-process HTTPS server (an
`rcgen` CA) serving the namespace subset the driver uses, from an in-memory store with real uids and
`resourceVersion`s:

- pods, PVCs, NetworkPolicies, Leases: create, get, list (paged, metadata), watch (chunked, with
  410 for an expired version and `ERROR` events), delete with `gracePeriodSeconds` and
  `preconditions.uid` (409 on mismatch), `dryRun=All`, and a switchable Pod Security admission;
- `SelfSubjectRulesReview` answering configured rules; ResourceQuota as a counted 403
  `exceeded quota:`;
- `pods/log` with `limitBytes`, and `pods/exec` over WebSocket v5, which runs the **real**
  `substrate-executor fs` against a temporary directory as `/workspace`;
- a fake kubelet the test steps: bind a node, run the exec container by spawning the real
  `substrate-executor run` with the rendered spec and a test cgroup directory (a hidden
  `--cgroup-root`), capture its stdout as the log, then write `terminated`;
- a fault plan: drop the answer after storing, 504, delay past the client timeout, force-delete
  without `terminated`, a same-named object of another deployment, a foreign-uid replacement, and
  Lease contention.

The fake cannot confine, so on it the probe correctly publishes no `confinement.no-egress` and no
`whole-tree-kill`. Exec-path tests open the driver through a crate-private `#[cfg(test)]` constructor
that takes a probe outcome. The shipped open path has no way to inject facts. Each case runs once, in
seconds. What the fake cannot show (kubelet, scheduler, Pod Security, the network plugin, storage) is
the lane's.

### 10.2 The kind lane (N11 A)

**Workflow `.github/workflows/namespace-lane.yml`.** It runs on pull requests touching
`crates/substrate-kubernetes*/**`, `crates/substrate-executor/**`, `crates/substrate-guarded/**`,
`crates/substrate-wire/**`, `crates/substrate-daemon/**`, `Dockerfile`, `Cargo.lock`,
`rust-toolchain.toml`, `xtask/src/namespace_lane*.rs` or the workflow itself; on pushes to `main`;
and on dispatch. `contents: read` only.

| job | what runs | where |
|---|---|---|
| `static` | `cargo xtask namespace-lane check`: renders keeper, exec and probe objects from fixed inputs against goldens; asserts that the Role the lane installs equals decision 3's table, that no rendered kind is cluster-scoped, and that every object carries the four keys of § 4 | runner, seconds |
| `cluster` | pinned kind with one control-plane and one worker node (`podPidsLimit: 1024`, `containerLogMaxSize: 10Mi`); a pinned `registry:2` by digest as kind's local registry; the `executor` image built and pushed, so it has a manifest digest; the exec image is the busybox digest the pod lane pins; then `cargo xtask namespace-lane run --admin-kubeconfig $RUNNER_TEMP/admin.json` | runner and kind |

`namespace-lane run` is Rust and uses the extracted client, never `kubectl`. It reads only the file
it is given, never `KUBECONFIG` or `~/.kube`, and refuses a kubeconfig whose server is not
`https://127.0.0.1:<port>`, kind's loopback endpoint. In order, it:

1. **provisions as admin:** the namespace labelled `pod-security.kubernetes.io/enforce: restricted`;
   the ServiceAccount, decision 3's Role and its binding; the default-deny policy; a ResourceQuota
   (`count/persistentvolumeclaims: 4`, `pods: 8`); a 1 h token in the driver's kubeconfig. It records
   the cluster-scoped inventory;
2. **observes O1 to O10 (§ 10.3)** with the admin and driver credentials, and writes each result into
   the job summary;
3. starts the daemon on the runner, `--driver kubernetes`, over TLS with the lane's in-process
   Identity stub, as the chart lane does;
4. runs the [design 31](31-remote-clean-room-conformance-runner.md) runner, built with `-p`, with
   `--restart-hook` killing (`KILL`) and restarting the daemon the verb supervises. Core groups must
   pass. Exec, events and restart must pass where their facts hold. Sessions, resources and retention
   must be `absent` with `fact-absent`;
5. runs `crates/substrate-daemon/tests/runtime_vectors.rs` with `SUBSTRATE_VECTORS_KUBERNETES_CONFIG`.
   This is a third lane beside portable and delegated: cases selected by facts, absent cases not
   counted, the executed count pinned as `NAMESPACE_CASES`;
6. runs the driver-specific cases once each (ADR 0047 decision 13): a same-named object of another
   deployment and a foreign-uid object are left untouched; with each Role verb removed in turn, the
   dependent fact is absent and its refusal named; an admitted `privileged` dry run refuses startup (a
   second namespace without the label); a running exec across a restart reads `unknown` and its Pod is
   gone; a force-deleted Pod is not reported ended before its container is shown stopped; a quota
   exhaustion is `workspace.capacity` and `exec.capacity`;
7. destroys everything through the daemon, then requires as admin: no object labelled
   `app.kubernetes.io/managed-by: b10x-substrate` in the namespace, and a cluster-scoped inventory
   equal to step 1's.

Fallback C (a `Namespace-lane:` tag record) applies only to a case the hosted runner cannot execute,
named in the verb's inventory. None is expected. The first run's duration is measured on its pull
request and recorded in the story. Each case runs once, with no repetition (AGENTS.md § *The gate*).

### 10.3 The lane's first observations

ADR 0047 § *Consequences* lists four unobserved behaviours. They are O1 to O4, run in U5 with busybox
and the client before any driver code relies on them. O5 to O10 are what this design adds.

| id | observation | if it does not hold |
|---|---|---|
| O1 | deleting a PVC under `pvc-protection` with a Pod owning-referenced to it: background GC removes the Pod, and the PVC goes once no Pod uses it | the driver's own Pods-first order (decision 4) is the only cleanup; the backstop is dropped from the claim |
| O2 | `dryRun=All` of a `privileged` Pod is refused by Pod Security admission in the `restricted` namespace, and admitted in an unlabelled one | startup step 5 cannot detect a missing level; the lane fails and the step is redesigned |
| O3 | kind's default plugin enforces the default-deny policy: the three probe connections fail | the lane installs a pinned enforcing plugin; without one, `confinement.no-egress` is absent and exec cases are `absent` |
| O4 | two Pods on one node mount one `ReadWriteOnce` claim of the `standard` class | exec facts are absent on that class (N3), and the lane fails until a class that allows it is used |
| O5 | `openat2` with `RESOLVE_BENEATH` is allowed by containerd's `RuntimeDefault` seccomp profile | the workspace facts are absent and every file route refuses; the executor gains no fallback |
| O6 | what `pids.max` reads inside a container under `podPidsLimit: 1024` | § 14 Q2 |
| O7 | a create over the ResourceQuota answers 403 with a Status message starting `exceeded quota:` | capacity is unrecognisable and stays `kubernetes.api-failed` |
| O8 | the rules review of the driver's ServiceAccount is decision 3's table plus the fixed default set | the pinned default set is corrected from the observation |
| O9 | base64 frame lines of 8202 bytes round-trip through `pods/log` byte-exactly, both streams interleaved | the chunk size halves |
| O10 | `pods/exec` accepts `v5.channel.k8s.io` and its close channel on v1.37 | `v4.channel.k8s.io`, with stdin closed by the request frame's own length |
| O11 | an exec Pod under `podPidsLimit` spawns processes until `fork` fails; the lane records the count reached against `pod_pids_limit`, and the count does not exceed it (§ 14 Q2) | the stated bound does not hold: the driver ships no `limits` fact, and the result goes to the coordinating session before anything else |

### 10.4 Release

`ghcr.io/beyond10x/b10x-substrate-executor:<version>` from the `executor` target, keyless-signed,
verified and anonymously read back before anything is announced (ADR 0006's pattern, with
`GITHUB_TOKEN` and no secret). `release.yml` refuses a tag whose commit has no successful
`namespace-lane.yml`, as it does for `gate.yml` (ADR 0046 K9 B); that lane asserts O11, so the
process bound is observed once per release on the real kubelet path. `xtask/tests/release_workflow.rs`
gains both refusals. AGENTS.md § *Releases* names the image and the condition.

## 11. Wire: one successor bundle

ADR 0047 decision 12 and § *Consequences* require one successor bundle after design 30's. It is the
next free version when that story starts: **`0.26.0`** if design 30 is cut as `0.25.0`. It adds no
route. It carries:

- `driver`: the schema's `{"const": "host"}` becomes `{"enum": ["host", "kubernetes"]}`, and
  `HostDriverKind` gains `Kubernetes` (N7 A);
- the optional fact `confinement.no-egress` (N5 A);
- `exec.inline-capsule-unserved` (501) with a vector;
- whatever § 14 Q3 adds, each with a vector.

Invariant 6 requires that bundle to have **its own design section before it is cut**: the
compatibility block, the exact route counts, each vector and the constant it is held to, and the
`ADVERTISED_CONTRACT_BUNDLE` move. That section is U7's first deliverable. This note fixes only the
content above. `cargo xtask render-bundle` cuts it. `scripts/gate.sh` gains it in `check-bundles` and
`check-json` in the same change, and design 31's runner moves its pin with it.

## 12. Implementation slices

One wave unit each, at most four per wave. Every unit ends with `bash scripts/gate.sh` green. The
tests named are the unit's own, each run once.

**Wave 1: foundations, no behaviour change**

- **U1 client extraction.** `substrate-kubernetes-api` from `pod.rs`; WebSocket `pods/exec`,
  preconditioned delete, `dryRun`, Lease and rules review added; the `fake` module and its feature
  fence. Tests: client cases against the fake (paging bound, 410 relist, precondition 409, lost
  answer, credential never in an error); `pod.rs` unit tests and the golden manifest unchanged; the
  pod lane green.
- **U2 guarded extraction.** `substrate-guarded` with `fs.rs`, `DriverError`, `Listed` and the
  environment rule; re-exports from `substrate-host`. Tests: the moved fs tests; `driver_port`
  unchanged; `cargo xtask check-packages`.
- **U3 backend ledger.** The `backend_objects` migration, `BackendLedger`, `backend_names` (default
  empty), reservation in the `accepted` transaction, and the dispatch-absence branch of § 5.1. Tests:
  store cases (reserve, bind, refuse a second uid, settle); a daemon case showing the host driver
  writes no row and answers exactly as before.

**Wave 2: the executor, the cluster facts, the driver core**

- **U4 executor.** Crate, binary, protocol and frame codec; static musl; `executor` Dockerfile
  target; the `tree` rule in `check-packages`. Tests: header and frame round trips; every `fs` op
  against a temp root, symlink escape refused; argv byte-exact including `$`; environment exactly the
  spec; interleaved streams tagged; truncation records; the `limits-unapplied` refusal with a test
  cgroup root; version handshake.
- **U5 lane skeleton and O1 to O4, O6 to O8, O10.** `namespace-lane.yml`, `cargo xtask namespace-lane
  check|observe`, provisioning, local registry. Tests: the verb's kubeconfig refusals (non-loopback
  server, ambient `KUBECONFIG` ignored); the static goldens. Results recorded in the story before U8
  and U9 start.
- **U6 driver core.** `substrate-kubernetes`: extension parsing, names and labels, ownership predicate,
  Lease, rules review, policy check, dry-run step, snapshot binding. Tests on the fake: each startup
  refusal; Lease takeover only after 30 s unchanged; held lost after 20 s without renewal; foreign
  deployment and foreign uid untouched.

**Wave 3: the bundle and the two resource paths**

- **U7 successor bundle.** Its design section first (§ 11), then the cut. Tests: `check-bundle` for
  the new version, a vector each, the runtime-vector route probes.
- **U8 workspace path.** PVC and keeper create, handshake, every served file route over `pods/exec`,
  lost-answer recovery by digest, destroy with `Pending`. Tests on the fake: each § 5 row for
  workspaces and files, including a crash between PVC and keeper and the N9 bound.
- **U9 exec path.** Pod rendering (goldens), start with binding re-read, watch and relist, log decode,
  signals, gone-Pod evidence, garbage rounds. Tests on the fake: each § 5 row for execs, force delete,
  410 mid-run, a forged frame from the child impossible (child writes on its own stdout) and a
  non-frame line refused. U8 and U9 share `lib.rs`'s `Driver` impl and nothing else. They meet there,
  and the wave plan says so.

**Wave 4: serving**

- **U10 probe, facts, daemon wiring.** The probe and its repeat, `--driver kubernetes`, N6 admission,
  the capsule refusal after the final check, the deployment guide section. Tests: probe facts on the
  fake (no `no-egress` there); admission cases for both fact sets; the host lane answers unchanged.
- **U11 restart reconciliation.** § 5 restart column end to end on the fake with a real daemon
  restart. Tests: a running exec becomes `unknown` and its Pod is gone; a terminated one keeps its exit
  and output; an unbound create past the bound is `failed`; probe leftovers are cleaned.
- **U12 release preflight.** The executor image publication and the `namespace-lane.yml` condition in
  `release.yml`. Tests: `xtask/tests/release_workflow.rs` refusals.

**Wave 5: the lane in full**

- **U13 namespace lane run.** Steps 3 to 7 of § 10.2, O5 and O9, the `NAMESPACE_CASES` lane in
  `runtime_vectors.rs`, the cleanup assertions; STATUS.md, CHANGELOG.md and AGENTS.md § *The gate*.
  Tests: the lane itself on its pull request, with the first run's duration recorded.

## 13. Decisions taken here

Each is reversible and inside ADR 0047.

| id | decision | why |
|---|---|---|
| D1 | four packages (§ 2) | a single crate is a Cargo cycle; the executor must not link the host stack |
| D2 | the executor is static musl | it runs inside an image the operator chooses, whose libc is unknown |
| D3 | a `backend_objects` table, names reserved by the daemon in the `accepted` transaction | durable-before-call is the daemon's guarantee, not the driver's; no wire column is misused |
| D4 | the run spec as base64url argument parts | no `$` for kubelet to expand; `MAX_ARG_STRLEN` |
| D5 | ASCII frame lines under 16 KiB | CRI line splitting and binary safety |
| D6 | Lease 30 s, renew 10 s, held under 20 s, takeover on unchanged `resourceVersion` | skew-free, client-go's rule |
| D7 | fake API behind a dev-only feature, fenced by `check-packages` | one fake for two crates without a test-support package |
| D8 | `workspace.storage-quota` absent; PVCs at the operator's size | a PVC cannot honour a requested inode bound |
| D9 | the lane uses two nodes and a local registry | node pinning needs a wrong node to avoid; Pods need manifest digests |
| D10 | an unanswered file call is resolved once by `stat` and digest, never resent | no fabricated "not written" |
| D11 | workspace create waits 60 s for the handshake, then `OutcomeUnknown` | first-consumer binding and pulls can be slow; the provisional loop finishes it |
| D12 | dispatch absence is recognised by an unbound `pod` row | N9's `failed` without changing the host driver's `unknown` |

## 14. Decisions for the operator

Only questions ADR 0047 does not settle.

**Answered** by the operator on 2026-10-29, verbatim: "accept all if it s the right and not the
cheapest low effort choice". Read against that condition by the coordinating session:

- **Q1: B.** The more thorough option: one read-only verb, and every policy change moves
  `config_generation`. ADR 0047 decision 3's table is amended with it.
- **Q2: A, with O11** (§ 10.3). A alone states a bound without observing it. The lane spawns
  processes in an exec Pod until `fork` fails and records the count against `pod_pids_limit`, and
  the release asserts it. At runtime `limits` still publishes the operator's value; the guide names
  O11 as its proof. If O11 shows the bound does not hold, A does not ship.
- **Q3: A.** B calls a valid request malformed.
- C of Q2 stays rejected: a per-uid count is the wrong bound.

### Q1 — additive NetworkPolicies

NetworkPolicies are additive allows. With `get` only (decision 3), another policy in the namespace
that allows egress is invisible to the driver until the next probe sees a connection succeed.

| option | what | cost |
|---|---|---|
| **A** keep decision 3: `get` on the named policy | no RBAC change | an allowing policy added between probes is unseen for up to `probe_interval_seconds` |
| **B** add `list` on `networkpolicies` and bind the digest of the namespace's whole policy set into the snapshot | any added or changed policy moves `config_generation`, and a start is `exec.capability-stale` before its Pod | one read-only verb, an amendment to ADR 0047 decision 3's table |

**Recommendation: B.**

### Q2 — the process bound, if O6 shows `pids.max` as `max`

kubelet's `podPidsLimit` is set on the Pod's cgroup. The container's own cgroup, which is all the
executor can see under a cgroup namespace, is expected to read `max`. ADR 0047 decision 6 assumed
the executor reads it back.

| option | what | cost |
|---|---|---|
| **A** take `pod_pids_limit` on the operator's word, as design 28 does, and publish `limits` from memory and CPU read back | execs are served | one bound is stated, not observed |
| **B** treat `limits` as unproved | honest | under N6 no exec is admitted; the track cannot close |
| **C** `RLIMIT_NPROC` in the executor | an enforced number | it counts per uid across the node, not per Pod: the wrong bound |

**Recommendation: A**, recorded in the guide.

### Q3 — refusals the ADR's bundle list lacks

`exec.signals` publishes `term` and `kill` only, and the wire has no named refusal for `int`.
`workspace.adopt` has no fact, and a driver with no operator directories can only answer "not found".

| option | what | cost |
|---|---|---|
| **A** add `exec.signal-unserved` (501) and `workspace.adopt-unserved` (501) to the successor bundle, each with a vector | invariant 3's named refusal for both | two more codes in the bundle |
| **B** reuse: `int` gets `request.schema-invalid` (422); adopt gets its existing not-found refusal | no new codes | a valid request is called malformed, and a capability gap is called absence |

**Recommendation: A.**

## 15. Not decided here

Everything ADR 0047 leaves out (§ *What this does not decide*), plus: chart packaging for this driver,
in-cluster hosting of the daemon (N1 B), image pull credentials (images must be pullable by the
nodes without a Pod secret), and a keeper lost while serving. In the first slice that case fails
every file route with `kubernetes.api-failed`, and the keeper is never recreated.
