# The `pod` confinement adapter: one Kubernetes Pod per run

Status: accepted for implementation, 2026-10-27. Owned by `story:pod-adapter`. It changes no wire
byte and no contract bundle: `pod` is an adapter behind substrate-host's confinement API
([ADR 0044](../../adr/0044-confinement-is-a-contract-family-served-by-declared-adapters.md)
decisions 7 and 8), not the Kubernetes namespace driver of `epic:kubernetes-deployment-and-driver`,
which stays a separate, later piece.

Invariant 8 asks for a design before code. This note takes the authority rules of
`story:kubernetes-namespace-driver-entry-gate` that apply to one Pod, fixes the Pod the adapter
renders, and lists every refusal.

## What the adapter serves and proves

`pod` serves `process_run`, `process_watch`, `output_read`, `wait`, `workspace`, `limits` and
`audit`, and proves `image_digest_pinned` (`spec/confinement/confinement.yaml`, the declared set
above `OpenAdapter`). It does not serve `filesystem_view`: a Pod reaches no host path. It serves
neither network family: a NetworkPolicy is enforced by the cluster's network plugin, which the
API does not let a client prove, so the adapter writes a default-deny policy and claims nothing
from it. It proves no `whole_tree_kill`, IPC or `private_tmpdir` fact.

## Opening: the operator's cluster configuration

`PodRequest.cluster` names one file, written by the operator, in the JSON form of a kubeconfig
(`kubectl config view --raw --minify --flatten -o json` writes it). The adapter reads its current
context and nothing else from the environment. Absent, unreadable, larger than 1 MiB, not JSON or
incomplete, the adapter does not open: `confinement.adapter-unserved` (501) with the reason.

| kubeconfig member | used as |
|---|---|
| context `namespace` | the only namespace the adapter touches; required, a DNS-1123 label |
| cluster `server` | an `https` URL; any other scheme is refused |
| cluster `certificate-authority-data` | the only trust roots; required |
| cluster `tls-server-name` | the name the server certificate is verified against, when present |
| user `client-certificate-data` and `client-key-data` | TLS client authentication |
| user `token` | a bearer token, sent only to that server |
| context extension `b10x-substrate/pod` | `run_as_user` (default 65532), `pod_pids_limit` and `ephemeral_storage_bytes` (default 1 GiB) |

Refused at open, by name, because each would change who the adapter is or whom it trusts:
`insecure-skip-tls-verify: true`, `proxy-url`, `certificate-authority`, `client-certificate`,
`client-key` and `tokenFile` (file paths: the configuration is one self-contained file), `exec` and
`auth-provider` (programs and plugins), `username` and `password`, and every impersonation member
(`as`, `as-uid`, `as-groups`, `as-user-extra`). A user with neither a client certificate nor a
token is refused.

Open then lists at most one Pod in the namespace. A cluster that does not answer within 10 seconds,
or answers anything but 200, does not open (`confinement.adapter-unserved`). Open next reads the
log of `substrate-log-access-probe`, a Pod no run is named: the API answers 404 to a credential that
may read `pods/log` and 403 to one that may not, and without that access no run's output could be
read, so a 401 or 403 does not open either, nor does any answer but 200, 400 or 404. The
credentials never appear in an error, a record or a log line.

Open last sweeps the namespace (`approval-record:pod-orphan-policy-sweep-d124`). It lists the
NetworkPolicies labelled `app.kubernetes.io/managed-by: b10x-substrate` and deletes one only when
its name is the run its `b10x-substrate/run` label names, the Pod of that name does not exist, in
any phase, and the API server's clock (its `Date`) puts the policy's `creationTimestamp` at least
90 seconds back: twice the 30-second API timeout plus 30 seconds. `start` creates the policy, waits
up to 30 seconds for that answer, then creates the Pod and waits up to 30 seconds more, and the API
server stamps a `creationTimestamp` in whole seconds before admission and storage, possibly on
another member than the one whose `Date` the sweep reads; a younger policy may fence a run another
adapter is still starting. A policy without that label, or whose name is not its run label, is never
touched. The list is read in pages of 100 with `continue`, each page bounded at 4 MiB, at most 64
pages; every page is read before anything is deleted. A list longer than that, or a sweep the API
refuses or does not answer, does not open (`confinement.adapter-unserved`).

`pod_pids_limit` is the operator's statement of the kubelet's `podPidsLimit` on every node that may
run the Pod. The Pod API has no per-Pod process bound and a namespaced client cannot read the
kubelet's, so this is the one bound the adapter takes on the operator's word. Every run has a
`processes` bound, the default one included, so without `pod_pids_limit` no profile is admitted.

`ephemeral_storage_bytes`, from 1 MiB to 1 TiB, is the workspace `emptyDir`'s `sizeLimit` and the
container's `ephemeral-storage` request and limit; it is 1 GiB when the operator states none.

## Authority: what a request cannot choose

The `ConfinementProfile` has no member for any of these. The renderer writes each as a fixed value,
so no profile can reach it, and the golden manifest
(`crates/substrate-host/tests/golden/pod-manifest.json`) is the check.

| Pod field | rendered as |
|---|---|
| `metadata.namespace` | the operator's context namespace |
| `metadata.name` | `substrate-run-<ulid>`, minted by the adapter |
| `spec.serviceAccountName` | not set: the namespace's `default`, with no token mounted |
| `spec.automountServiceAccountToken` | `false` |
| `spec.nodeName`, `spec.nodeSelector`, `spec.affinity`, `spec.tolerations` | not set |
| `spec.runtimeClassName` | not set |
| `spec.hostNetwork`, `spec.hostPID`, `spec.hostIPC` | `false` |
| `spec.enableServiceLinks` | `false` |
| `spec.restartPolicy` | `Never` |
| `spec.volumes` | one `emptyDir`, `workspace`, with `sizeLimit` `ephemeral_storage_bytes`; never a `hostPath` |
| `securityContext.privileged` | `false` |
| `securityContext.runAsNonRoot`, `runAsUser`, `runAsGroup` | `true`, the operator's numeric uid twice |
| `securityContext.readOnlyRootFilesystem` | `true` |
| `securityContext.allowPrivilegeEscalation` | `false` |
| `securityContext.capabilities` | `drop: [ALL]`, nothing added |
| `securityContext.seccompProfile` | `RuntimeDefault` |
| container `image` | the profile's image, by digest only |
| container `command` and `args` | the argv, split at its first element, every `$` written `$$`; no shell |
| container `env` | the command's environment, every `$` in a value written `$$`, plus `SUBSTRATE_CONFINEMENT_LOCAL=refuse` |
| container `resources` | requests equal to limits, from the profile's bounds and `ephemeral_storage_bytes` |
| `spec.activeDeadlineSeconds` | the wall bound, rounded up to whole seconds |

`SUBSTRATE_CONFINEMENT_LOCAL=refuse` is atlas ADR 0109 F3 A: inside `pod` the host's policy fact
`local_permitted` is false, so a Substrate embedded in the run refuses `local`.

The adapter creates two namespaced objects per run and no cluster-scoped resource: the Pod and a
NetworkPolicy of the same name selecting it by the label `b10x-substrate/run`, with
`policyTypes: [Ingress, Egress]` and no rule. Both carry
`app.kubernetes.io/managed-by: b10x-substrate`. The policy is created first and deleted last. The
Pod carries one `ownerReference`, to that policy by the `uid` its create answered, with
`controller: false` and no `blockOwnerDeletion`, so the garbage collector removes a Pod whose
policy is gone and the Role needs nothing more; `render` shows the Pod without it, because the uid
exists only once the policy is created. The
minimal Role is `create`, `get`, `list`, `delete` on `pods` and `networkpolicies`, and `get` on
`pods/log`; the CI lane runs with exactly that Role and the namespace labelled
`pod-security.kubernetes.io/enforce: restricted`.

## Refusals: profile and command fields

Each is refused before any API call. Admission refusals write a `profile_refused` audit record.

| field set through the profile or command | refusal |
|---|---|
| `ingress_routes`, `rate_bounds` | `confinement.reserved-field-unserved` (501) |
| `inner` | `confinement.adapter-unserved` (501) |
| `image` absent, by tag, or not `@sha256:` and 64 lowercase hex | `confinement.image-unpinned` (422) |
| `writable_roots`, `read_only_roots`, `read_only_files` (a `hostPath`) | `confinement.family-unserved` naming `filesystem_view` (501) |
| `workspace` naming a host directory (a `hostPath`) | `confinement.family-unserved` naming `filesystem_view` (501) |
| `network: Egress` | `confinement.family-unserved` naming `network_egress` (501) |
| `network: Isolated` (a promise the API cannot prove) | `confinement.family-unserved` naming `network_egress` (501) |
| `sealed_paths`, `exec_subpaths`, `private_tmpdir`, `ipc.socket_allowlist`, `ipc.signals_contained` | `confinement.fact-unserved` naming the fact (501) |
| `secret_slots` (a Pod spec is stored in the cluster) | `exec.secret-slots-unserved` (501) |
| `workdir` not relative, or with `..` | `exec.workdir-invalid` (422) |
| any profile when the operator declared no `pod_pids_limit` (every run has a processes bound, the default included); `process_bounds.processes` below it | `confinement.pod-processes-unserved` (501) |
| a bound outside the linux driver's ranges, or CPU under 1 millicore | `exec.limit-unserved` (429) |
| `io_bounds.output_bytes` of 0 or above 1 MiB | `exec.limit-unserved` (429) |
| an empty argv, or one above 256 KiB | `exec.argv-invalid` (422) |
| an environment name not `[A-Z][A-Z0-9_]*`, a secret-like name, `SUBSTRATE_CONFINEMENT_LOCAL`, a value above 4096 bytes or with NUL, more than 64 entries | `exec.environment-refused` (422) |
| `signal: Int` (the API delivers no signal) | `confinement.pod-signal-unserved` (501) |

`local_reason` is ignored: `pod` is not declared insecure.

## Bounds

Without `process_bounds` the run gets the linux driver's defaults: 600 s wall, 1 GiB, 600 s of CPU.
With them:

- memory: `memory_bytes` as both request and limit;
- CPU: a rate, `cpu_millis / ceil(timeout_ms / 1000)` millicores, as both request and limit. With
  the wall bound, cumulative CPU time stays at or below `cpu_millis`; a rate under 1 millicore is
  refused, never rounded up;
- wall: `activeDeadlineSeconds`; the adapter also deletes a Pod not ended 60 s after the wall
  bound, which covers a Pod that never started (unschedulable, image not pulled);
- processes: the operator's `pod_pids_limit`, which must be at or below the profile's bound; a
  profile without `process_bounds` runs under it.

## A run

`start` creates the NetworkPolicy, then the Pod. A Pod create that fails, refused or with its
answer lost after the API may have stored the Pod (a dropped connection, a 504, the client's
timeout), deletes the Pod by name and confirms it is gone before it deletes the policy; when the
Pod's removal is not confirmed the policy stays and the start fails
`confinement.pod-api-failed` naming the Pod. A policy create answered without a `uid` removes the
policy and fails the start the same way. Once the Pod is stored, `start` reads the policy back: a
404, any other failed read, or a policy with another `uid` removes the Pod the same way and fails
the start `confinement.pod-api-failed`, so a run whose policy another adapter's sweep removed in
between never starts; a removal after the read-back leaves the Pod to the garbage collector through
its `ownerReference`. One task per run polls the Pod. When the API shows
the container terminated, the task reads the container's log, at most the output bound plus one
byte, then deletes the Pod and, once the Pod is confirmed gone, the policy, before `wait` reports
the run ended. A Pod is never force-deleted: every deletion leaves the kubelet a grace of at least
1 second, so a Pod gone after the adapter's own deletion is the kubelet's confirmation that its
container stopped. A Pod's removal not confirmed within 2 minutes keeps its policy and reports the
run `Unknown` while the task keeps trying; so does an API that has not answered for 2 minutes,
whatever the removal is doing, and a Pod someone else deleted. After `Unknown` the task deletes and
reads the Pod again, the pause between tries doubling from 500 ms to 30 seconds, until the API
confirms the Pod gone, then removes the policy the same way; the run still reads `Unknown`. The
task lives as long as the process that holds the adapter; the next open's sweep removes what it
leaves. The adapter tracks at most 128 runs and makes room by forgetting runs that are over, oldest
first; a run whose Pod's removal the API has not confirmed is not over for that count, so at 128 a
start is refused `exec.tracking-capacity` and the removal tasks never outnumber the cap.

`wait` and `observe` carry `memory_oom_kills` once the API has shown the container's terminated
state: 1 for a container that ended `OOMKilled`, 0 otherwise, absent when it was not shown.

The kubelet keeps one log per container, interleaving stdout and stderr, and the API returns that
one stream. `output_read` under `pod` reports it as `stdout`, with `stderr` empty. A log read is
tried five times; a log the API does not return is `confinement.pod-api-failed`, never an empty
output. Only a container the API has not shown started reads as empty.

`signal: Kill` reads the log, then deletes the Pod with a grace period of 1 second; `signal: Term`
with 5 seconds; the kubelet sends `SIGTERM` and then `SIGKILL`. The run ends with that signal once
the API shows the container terminated or the Pod gone; a kubelet that has not confirmed it within
60 seconds leaves the run `Unknown`, because its container may still run. A Pod not ended 60 s
after the wall bound is deleted the same way and ends as a `Kill`. The policy is deleted after the
Pod in every case.

## Not decided here

Restart reconciliation, durable-before-API dispatch, watch gaps, quotas, server-side apply and
finalizers belong to the namespace driver (`story:kubernetes-namespace-driver-entry-gate`). An
embedding process that dies mid-run leaves the run's two objects, labelled for an operator to find;
`activeDeadlineSeconds` still ends the container. Once the Pod is gone, the next open's sweep
removes the policy; reconciling the run itself stays with the namespace driver.
