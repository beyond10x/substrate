---
title: Confinement adapters
description: What each of the six confinement adapters enforces, the families and facts it declares, what it refuses by name, what it needs, and when to choose it.
---

# Six adapters, one contract

An adapter is one way to run a program under the confinement contract: a Linux sandbox, a macOS
Seatbelt profile, a systemd scope, a container or a Kubernetes Pod. Every adapter answers the same
contract. When it opens, it declares which contract families it serves and which facts it proved.
A profile that needs anything else is refused by name, before anything runs. Nothing falls back to
another adapter, and nothing is quietly dropped.

All six adapters run real programs since 0.12.0. The [contracts overview](../reference/contracts.md)
defines the families and facts; [choose a confinement profile](../guides/confinement-profiles.md)
shows the Rust API that opens an adapter and admits a profile.

## Pick an adapter

| You need | Choose | Because |
|---|---|---|
| a Linux sandbox that bounds the whole process tree | `linux` | namespaces, seccomp and cgroup v2, each proved at open |
| a confined build on a Mac | `macos` | a Seatbelt profile that writes only the workspace |
| bounds around a run on a desktop without a delegated cgroup | `systemd` around `linux` | the user manager's scope becomes `linux`'s cgroup root |
| a run inside a pinned image | `oci` | one Docker or podman container per run, no network |
| a run on a Kubernetes cluster | `pod` | one Pod per run beside a default-deny NetworkPolicy |
| no confinement, knowingly | `local`, or `systemd` around `local` for bounds | declared insecure, with a reason on every observation |

Prefer an adapter that does not prove `declared_insecure`. Choose by the facts an adapter
declares, never by its name or the operating system; the
[profile guide](../guides/confinement-profiles.md#choose-an-adapter-by-its-facts) shows the check.

## Compare adapters

`TRUE` means the adapter always declares it once it opens. `UNKNOWN` means it declares it only
when proved by the open-time probe on this host, or by the operator's configuration. `FALSE`
means it never does. `cargo xtask check-docs` holds both tables to the declared sets in the specification's
scenarios.

### Families by adapter

| Family | local | linux | macos | systemd (local) | systemd (linux) | oci | pod |
|---|---|---|---|---|---|---|---|
| `process_run` | TRUE | UNKNOWN | TRUE | TRUE | UNKNOWN | TRUE | TRUE |
| `process_watch` | TRUE | UNKNOWN | TRUE | TRUE | UNKNOWN | TRUE | TRUE |
| `output_read` | TRUE | UNKNOWN | TRUE | TRUE | UNKNOWN | TRUE | TRUE |
| `wait` | TRUE | UNKNOWN | TRUE | TRUE | UNKNOWN | TRUE | TRUE |
| `workspace` | TRUE | UNKNOWN | TRUE | TRUE | UNKNOWN | TRUE | TRUE |
| `filesystem_view` | FALSE | UNKNOWN | TRUE | FALSE | UNKNOWN | TRUE | FALSE |
| `network_egress` | FALSE | UNKNOWN | FALSE | FALSE | FALSE | FALSE | FALSE |
| `network_ingress` | FALSE | FALSE | FALSE | FALSE | FALSE | FALSE | FALSE |
| `limits` | FALSE | UNKNOWN | FALSE | TRUE | TRUE | UNKNOWN | TRUE |
| `audit` | TRUE | TRUE | TRUE | TRUE | TRUE | TRUE | TRUE |

### Facts by adapter

| Fact | local | linux | macos | systemd (local) | systemd (linux) | oci | pod |
|---|---|---|---|---|---|---|---|
| `declared_insecure` | TRUE | FALSE | FALSE | TRUE | FALSE | FALSE | FALSE |
| `paths_are_host_paths` | TRUE | FALSE | TRUE | TRUE | FALSE | FALSE | FALSE |
| `whole_tree_kill` | FALSE | UNKNOWN | FALSE | TRUE | TRUE | FALSE | FALSE |
| `local_permitted` | UNKNOWN | FALSE | FALSE | UNKNOWN | FALSE | FALSE | FALSE |
| `ipc_socket_allowlist` | FALSE | UNKNOWN | FALSE | FALSE | UNKNOWN | FALSE | FALSE |
| `ipc_signals_contained` | FALSE | UNKNOWN | TRUE | FALSE | UNKNOWN | FALSE | FALSE |
| `sealed_paths` | FALSE | FALSE | UNKNOWN | FALSE | FALSE | FALSE | FALSE |
| `exec_subpaths` | FALSE | FALSE | UNKNOWN | FALSE | FALSE | FALSE | FALSE |
| `private_tmpdir` | FALSE | FALSE | UNKNOWN | FALSE | FALSE | FALSE | FALSE |
| `host_network` | TRUE | FALSE | FALSE | TRUE | FALSE | FALSE | FALSE |
| `image_digest_pinned` | FALSE | FALSE | FALSE | FALSE | FALSE | TRUE | TRUE |
| `stdin_pipe` | TRUE | UNKNOWN | TRUE | TRUE | UNKNOWN | UNKNOWN | FALSE |

Every refusal in the sections below comes after the contract's own order (a reserved field, an
adapter that did not open, a missing reason, an unpinned image, the host's policy, then the first
missing family and the first missing fact), which the
[profile guide](../guides/confinement-profiles.md#refusals) lists.

## local

An unconfined host process, always declared insecure.

- **Enforces.** No confinement. A start still holds the environment to the host driver's closed,
  non-secret shape (`exec.environment-refused`), refuses a working directory reached through a
  symbolic link (`exec.workdir-missing`), and rechecks the adapter's own proof. A signal reaches the
  run's process group; the leader stays unreaped until its output closes, so its group id cannot
  be reused.
- **Declares.** `process_run`, `process_watch`, `output_read`, `wait`, `workspace` (a host
  directory) and `audit`; the facts `declared_insecure`, `paths_are_host_paths` and
  `host_network`, and `local_permitted` unless the host forbids it.
- **Refuses by name.** A profile without a reason (`confinement.local-reason-missing`); any profile
  when the host forbids `local` (`confinement.local-refused`); every filesystem view, network
  policy and bound (`confinement.family-unserved`); a profile that requires a fact it does not
  prove, such as `private_tmpdir` (`confinement.fact-unserved`); sealed secret slots
  (`exec.secret-slots-unserved`).
- **Needs.** Nothing: it opens on every host, and only by name. The operator forbids it by setting
  `SUBSTRATE_CONFINEMENT_LOCAL` to any value other than `permit`; an embedder can only tighten
  that, with `LocalPolicy::Refuse`.
- **Choose it when** the run must not be confined, such as a trusted tool on a developer machine,
  and the reason is worth recording on every observation.

## linux

The host driver behind the contract: bubblewrap, namespaces, cgroup v2, seccomp and `openat2`.
[Confinement and refusal](./confinement.md) describes what the driver enforces.

- **Enforces.** An argument vector rather than a shell string, a cleared and shaped environment, a
  writable `/workspace`, read-only system inputs and declared roots, no usable network interface
  unless an operator-declared aperture is admitted, a seccomp filter on new Unix-domain sockets and
  `io_uring_setup`, PID, memory and CPU bounds in its own cgroup, and whole-cgroup termination.
- **Declares.** Each family only when the driver's probe proved what it rests on:
  `process_run`, `process_watch` and `wait` on the confinement floor (argv-only exec and
  namespaces), `output_read` with an output limit, `workspace` with guarded I/O,
  `filesystem_view` with read-only roots and files, `network_egress` with a declared egress
  aperture, `limits` with cgroup limits on processes, memory and CPU, and `audit` always. Facts:
  `whole_tree_kill` with cgroup kill, `ipc_socket_allowlist` on the floor, and
  `ipc_signals_contained` on the floor with a pid namespace.
- **Refuses by name.** A profile that requires `paths_are_host_paths`, `sealed_paths`,
  `exec_subpaths`, `private_tmpdir` or `host_network` (`confinement.fact-unserved`); a working
  directory the host has not proved (`exec.workdir-unserved`); a writable root the operator did
  not declare (`exec.writable-roots-host-unserved`); an egress aperture the host did not declare
  (`exec.aperture-undeclared`); a workspace that is not a directory directly beneath the driver's
  workspace root (`workspace.path-escape`); secret slots the host does not serve
  (`exec.secret-slots-unserved`, `exec.secret-slot-unknown`).
- **Needs.** Linux, a bubblewrap binary that can set up the run's namespaces, a delegated cgroup v2
  root, `close_range`, and an embedder that does not run as uid 0. The open-time probe needs all
  of them. Without any one, `linux` serves only `workspace` and `audit`, proves none of its facts
  and runs nothing.
- **Choose it when** the run must be confined on Linux and its whole process tree bounded and
  killed.

## macos

A host process under a Seatbelt profile, run through `/usr/bin/sandbox-exec`.

- **Enforces.** The workspace is the only writable location, the declared roots are read-only and
  visible only at their host paths, the program itself is the only executable unless exec
  subpaths are admitted, and the run's environment is all it sees. `signal` reaches only the same
  sandbox.
- **Declares.** `process_run`, `process_watch`, `output_read`, `wait`, `workspace`,
  `filesystem_view` and `audit`; the facts `paths_are_host_paths` and `ipc_signals_contained`.
  The build policy adds three facts, each only when a Seatbelt probe at open saw it enforced:
  `sealed_paths` (paths inside the workspace, and the directories above them, stay unwritable),
  `exec_subpaths` (programs beneath a declared directory may run: a workspace-relative path such as
  `target`, a declared root, or a system or developer directory) and `private_tmpdir` (each run
  gets its own `TMPDIR`, removed before `wait` returns).
- **Refuses by name.** `limits`, `network_egress` and `network_ingress`
  (`confinement.family-unserved`): Seatbelt has no pids or memory bound and no egress forwarder.
  `whole_tree_kill` (`confinement.fact-unserved`): a child can leave the process group with
  `setsid`. A writable root outside the workspace (`exec.writable-roots-host-unserved`), a single
  read-only file (`exec.read-only-files-unserved`), a read-only root mounted away from its host
  path (`exec.read-only-root-invalid`), any `DYLD_*` variable (`exec.environment-refused`), a
  sealed path or exec subpath with a symbolic-link component
  (`exec.sandbox-path-unrepresentable`), an absolute exec subpath outside the admitted directories
  (`exec.sandbox-exec-subpath-undeclared`), a program the policy does not list
  (`exec.sandbox-program-unlisted`), and a render of a profile with `private_tmpdir`
  (`confinement.render-unserved`), because only a start creates that directory.
- **Needs.** macOS with `/usr/bin/sandbox-exec`. Open fails closed as
  `confinement.adapter-unserved` off macOS or when the probe does not show a deny-default profile
  enforced.
- **Choose it when** a build or test runs on a Mac: `cargo test --offline` of a crate with a
  vendored dependency and a build script runs under it on the `macos-15` CI job.

## systemd

A transient `Delegate=yes` scope from the user's systemd manager, as the bounds provider around
`local` or `linux`. Opening it moves the embedding process into the scope once; every `systemd`
adapter the process opens shares it.

- **Enforces.** Around `local`: one child cgroup per run with `pids.max`, `memory.max` with no
  swap, `memory.oom.group`, the timeout and the cumulative CPU budget, and `cgroup.kill`. Around
  `linux`: the scope is the host driver's delegated cgroup root, so `linux` enforces its own
  bounds. The scope carries the nearest process and memory bound of the cgroup the process left.
- **Declares.** Around `local`: `local`'s families plus `limits`, and `local`'s facts plus
  `whole_tree_kill`. The run stays declared insecure and needs a reason, because a run with the
  embedder's uid can raise its own `pids.max` or leave its cgroup. Around `linux`: exactly what
  `linux` proved inside the scope.
- **Refuses by name.** Around `local`, an environment that names `DBUS_SESSION_BUS_ADDRESS`,
  `DBUS_STARTER_ADDRESS` or `XDG_RUNTIME_DIR` (`exec.environment-refused`); a requested `inner`
  other than the one it opened around (`confinement.adapter-unserved`); a second `systemd` around
  `linux` while one is open (`confinement.adapter-unserved`).
- **Needs.** A reachable user manager: `DBUS_SESSION_BUS_ADDRESS` set to one `unix:path=` address,
  `systemctl` and `busctl`, and `systemctl --user` answering. Open is refused
  `confinement.adapter-unserved`, before anything moves, without them, or when the scope lacks
  `cpu`, `memory` and `pids`, `cgroup.kill` or the enclosing bounds, or when `linux` did not prove
  `limits` and `whole_tree_kill` inside it.
- **Choose it when** a desktop session has no delegated cgroup root of its own. Use `inner: linux`
  for a bounded, confined run; `inner: local` bounds the tree but does not confine it.

## oci

One Docker or podman container per run, the image named by digest only.

- **Enforces.** `--pull never`, `--network none`, `--ipc none`, `--cap-drop ALL`,
  `--security-opt no-new-privileges`, a read-only root with a private `/tmp`, the embedder's own
  uid and gid (never 0), the runtime's default seccomp profile, `--init` so the command is not
  PID 1, and the profile's bounds, or the defaults, as `--pids-limit`, `--memory`,
  `--memory-swap` and `--cpus`. The workspace is bound at `/workspace`, declared roots and files
  read-only. The container is removed after `wait`, a kill or a timeout.
- **Declares.** `process_run`, `process_watch`, `output_read`, `wait`, `workspace`,
  `filesystem_view` and `audit`, `limits` when the runtime reports memory, swap, pids and CPU quota
  control, and the fact `image_digest_pinned`. It does not prove `ipc_socket_allowlist`: a Unix
  socket beneath a bound root is reachable from the container.
- **Refuses by name.**
  - An image that is not `name@sha256:` and 64 lowercase hex characters
    (`confinement.image-unpinned`), and a pinned image the runtime does not hold
    (`confinement.image-absent`).
  - `network_egress` (`confinement.family-unserved`).
  - A bound host path that is, or holds, a socket the runtime is reached through
    (`confinement.runtime-socket-exposed`), and a path the runtime's `--mount` value cannot carry
    exactly, holding `,`, `"`, `:`, a line break or NUL (`confinement.path-unrepresentable`).
  - At admission, a profile when the runtime reports no pids, memory, swap and CPU control, or
    whose process bounds, output bound (1 byte to 1 MiB) or CPU share over the wall bound fall
    outside what the runtime accepts (`exec.limit-unserved`).
  - A workspace that is not a directory directly beneath the operator's workspace root
    (`workspace.path-escape`).
- **Needs.** A runtime program (`docker`, `podman` or an absolute path) whose `info` answers and
  reports seccomp, and an embedder that does not run as uid 0. Open fails closed as
  `confinement.adapter-unserved` otherwise. On a rootful host the runtime's socket is
  root-equivalent: the adapter's authority is the operator's runtime.
- **Choose it when** the run needs a toolchain in a pinned image, no network, and nothing of the
  host but its workspace and declared roots.

## pod

One Kubernetes Pod per run, beside a default-deny NetworkPolicy, in the namespace the operator's
cluster configuration names. The adapter speaks to the API server itself; it runs no `kubectl`.

- **Enforces.** `runAsNonRoot` with the operator's uid (65532 when none is named), the `RuntimeDefault` seccomp profile, no
  privilege escalation, a read-only root filesystem, every capability dropped, no host network,
  PID or IPC namespace, no service account token, requests equal to limits, an `emptyDir`
  workspace at `/workspace` with a 1 GiB size and ephemeral-storage bound unless the operator
  states another, and `activeDeadlineSeconds` from the wall bound. The argv and environment pass
  literally: every `$` is written `$$`. The Pod is never force-deleted, and its policy is deleted
  only once the Pod is gone and its container shown terminated; otherwise the run reads `Unknown`.
- **Declares.** `process_run`, `process_watch`, `output_read`, `wait`, `workspace`, `limits` and
  `audit`, and the fact `image_digest_pinned`. It serves no `filesystem_view`, because a Pod
  reaches no host path, and neither network family, because a NetworkPolicy's enforcement depends
  on the cluster's network plugin, which a client cannot prove. Its observation adds
  `memory_oom_kills` from the container's terminated state.
- **Refuses by name.** An unpinned image (`confinement.image-unpinned`); a host workspace path,
  writable or read-only roots and files (`confinement.family-unserved` naming `filesystem_view`);
  a network policy (`confinement.family-unserved` naming `network_egress`); a processes bound the
  cluster does not hold (`confinement.pod-processes-unserved`); an interrupt signal, since the API
  delivers only termination (`confinement.pod-signal-unserved`); an argv over 256 KiB or holding a
  NUL (`exec.argv-invalid`); a request the API refused or failed (`confinement.pod-api-failed`).
- **Needs.** The JSON form of a kubeconfig whose current context names a namespace, a cluster and a
  user. The optional `b10x-substrate/pod` context extension sets `run_as_user`, `pod_pids_limit` and
  `ephemeral_storage_bytes`; without `pod_pids_limit` no profile is admitted. The credential must
  read `pods/log` and watch `pods`. Open is refused `confinement.adapter-unserved` without a configuration, with an
  unusable one, when the cluster does not answer, or without log access.
- **Choose it when** the run belongs on a cluster rather than a host, and needs no host path.
