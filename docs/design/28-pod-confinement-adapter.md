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
| context extension `b10x-substrate/pod` | `run_as_user` (default 65532) and `pod_pids_limit` |

Refused at open, by name, because each would change who the adapter is or whom it trusts:
`insecure-skip-tls-verify: true`, `proxy-url`, `certificate-authority`, `client-certificate`,
`client-key` and `tokenFile` (file paths: the configuration is one self-contained file), `exec` and
`auth-provider` (programs and plugins), `username` and `password`, and every impersonation member
(`as`, `as-uid`, `as-groups`, `as-user-extra`). A user with neither a client certificate nor a
token is refused.

Open then lists at most one Pod in the namespace. A cluster that does not answer within 10 seconds,
or answers anything but 200, does not open (`confinement.adapter-unserved`). The credentials never
appear in an error, a record or a log line.

`pod_pids_limit` is the operator's statement of the kubelet's `podPidsLimit` on every node that may
run the Pod. The Pod API has no per-Pod process bound and a namespaced client cannot read the
kubelet's, so this is the one bound the adapter takes on the operator's word; without it, no
`processes` bound is admitted.

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
| `spec.volumes` | one `emptyDir`, `workspace`; never a `hostPath` |
| `securityContext.privileged` | `false` |
| `securityContext.runAsNonRoot`, `runAsUser`, `runAsGroup` | `true`, the operator's numeric uid twice |
| `securityContext.readOnlyRootFilesystem` | `true` |
| `securityContext.allowPrivilegeEscalation` | `false` |
| `securityContext.capabilities` | `drop: [ALL]`, nothing added |
| `securityContext.seccompProfile` | `RuntimeDefault` |
| container `image` | the profile's image, by digest only |
| container `command` and `args` | the argv, split at its first element; no shell |
| container `env` | the command's environment plus `SUBSTRATE_CONFINEMENT_LOCAL=refuse` |
| container `resources` | requests equal to limits, from the profile's bounds |
| `spec.activeDeadlineSeconds` | the wall bound, rounded up to whole seconds |

`SUBSTRATE_CONFINEMENT_LOCAL=refuse` is atlas ADR 0109 F3 A: inside `pod` the host's policy fact
`local_permitted` is false, so a Substrate embedded in the run refuses `local`.

The adapter creates two namespaced objects per run and no cluster-scoped resource: the Pod and a
NetworkPolicy of the same name selecting it by the label `b10x-substrate/run`, with
`policyTypes: [Ingress, Egress]` and no rule. Both carry
`app.kubernetes.io/managed-by: b10x-substrate`. The policy is created first and deleted last. The
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
| `process_bounds.processes` with no `pod_pids_limit`, or below it | `confinement.pod-processes-unserved` (501) |
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
- processes: the operator's `pod_pids_limit`, which must be at or below the profile's bound.

## A run

`start` creates the NetworkPolicy, then the Pod; a Pod refused by the API deletes the policy. One
task per run polls the Pod. When it ends (`Succeeded` or `Failed`), the task reads the container's
log once, at most the output bound plus one byte, then deletes the Pod and the policy and confirms
both are gone before `wait` returns. A run whose container ended `OOMKilled` reports
`memory_oom_kills: 1` in its termination record (`PodAdapter::termination`) and its exit code.

The kubelet keeps one log per container, interleaving stdout and stderr, and the API returns that
one stream. `output_read` under `pod` reports it as `stdout`, with `stderr` empty.

`signal: Kill` reads the log, then deletes the Pod with a grace period of 0; `signal: Term` deletes
it with 5 seconds, so the container receives `SIGTERM` and then `SIGKILL`. The run ends with that
signal. The policy is deleted after the Pod in every case: after `wait`, after a kill and after a
deadline.

## Not decided here

Restart reconciliation, durable-before-API dispatch, watch gaps, quotas, server-side apply and
finalizers belong to the namespace driver (`story:kubernetes-namespace-driver-entry-gate`). An
embedding process that dies mid-run leaves the run's two objects, labelled for an operator to find;
`activeDeadlineSeconds` still ends the container.
