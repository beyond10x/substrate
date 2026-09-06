# Container-private host execution

Status: implementation design, 2026-09-06. Owned by
`story:container-private-exec-bootstrap`; this configures the existing host driver and does not
introduce a container driver, wire capability, contract bundle or caller-selected runtime.

## Evidence and boundary

The released image supplies the daemon but not the shell, socat or bubblewrap required by
`crates/substrate-host/src/probe.rs`. Its cgroup probe requires a process-free writable ancestor
of the daemon, and its process launcher already enforces namespaces, seccomp and per-exec limits.
The terminal must pass those probes without changing them.

A disposable Kubernetes container with private PID/mount/cgroup namespaces, no host paths and
finite CPU/memory limits demonstrated the following sequence: a bind-remount of its existing
`/sys/fs/cgroup` view, moving itself into `daemon/`, enabling cpu/memory/pids, delegating ownership,
and dropping to UID/GID 65532. It then wrote child CPU, memory, swap and process ceilings with no
permitted/effective/inheritable/ambient capabilities. The enclosing CPU and memory limits were
unchanged. The default AppArmor profile refused the mount; diagnostic AppArmor removal applied
only to that disposable probe and is not the deployed profile.

The kernel's [delegation rules](https://www.kernel.org/doc/html/latest/admin-guide/cgroup-v2.html)
and containerd's [default AppArmor profile](https://github.com/containerd/containerd/blob/v2.2.1/contrib/apparmor/template.go)
explain these observations. The implementation must verify the exact namespace and filesystem
posture again on every startup rather than treating the probe as a permanent host fact.

## Bootstrap

An explicit `substrate-container-exec` entrypoint runs as container PID 1 and UID 0. It accepts a
quota selection and normal daemon arguments, never a program, UID, filesystem or cgroup path.
It refuses an override of the cgroup root. The selected executable is the image's fixed ordinary
daemon or its existing byte-identical quota executable.

Before mutation, require `0::/` in the process cgroup, a cgroup2 mount rooted at `/` on the fixed
path, finite enclosing CPU/memory ceilings and the bootstrap process as the only direct member.
Reject ambiguous mount parsing, missing controllers, existing unexpected children and unsupported
privilege state. Require the existing nosuid/nodev/noexec mount constraints and preserve them.
Make only that mount writable using a bind-remount; never mount the host hierarchy, join another
namespace, or expose a host path. Compare enclosing resource ceilings before and after.

Create the fixed daemon child, move the bootstrap there and prove the root process-free. Enable
cpu/memory/pids, plus io when available for child resource observations, and delegate only the root directory, cgroup.procs, cgroup.subtree_control and
the daemon child's migration files to UID/GID 65532. Parent resource-control files retain root
ownership. The non-root daemon must be unable to raise its enclosing limits.

Startup requires only SYS_ADMIN, CHOWN, SETUID, SETGID and SETPCAP. Empty supplementary groups,
clear ambient/inheritable authority and remove every bounding capability except SYS_ADMIN for
the explicitly selected quota profile. Drop GID/UID to 65532, verify all four current capability
sets are empty, and exec the image's fixed non-setuid Tini init with the fixed daemon and
delegated root. Tini stays PID 1 without capabilities, forwards termination to the daemon and
reaps orphaned sandbox processes. Both init and daemon executables must pass the protected
root-owned regular-file check. The actual image journey exposed unreaped bubblewrap zombies
when the daemon alone was PID 1; terminating a cgroup without reaping those process records
does not satisfy the cleanup proof. The ordinary profile
also enables no_new_privs. The quota profile allows only the pre-existing file capability to
reacquire SYS_ADMIN; the trusted process launcher continues to clear privileges before bubblewrap.
Failure exits before a daemon listener starts; container teardown owns incomplete bootstrap state.

## Runtime packaging and AppArmor

Keep ordinary non-root startup and its image checks. Add a pinned, non-setuid bubblewrap build,
socat and a shell with basic file tools to the daemon runtime. No network credentials or mutable
tool download occurs at service startup. The MCP image remains separate.

AppArmor remains explicit on deployments using it. A versioned inherited profile permits private
cgroup bind-remount, transient tmpfs/proc/devpts, bind mounts, remounts and namespace-local control
writes required by the existing confinement probes. It grants no new cgroup filesystem or block
filesystem mount. The daemon launcher retains no_new_privs and clears capabilities before the
sandbox executable; sandbox mounting occurs after it creates its own user and mount namespaces.
A separate relaxing Px profile transition was considered and rejected: no_new_privs forbids
relaxing LSM constraints at exec. No permission to change profiles is introduced to bypass it.

This profile admits more mount setup than containerd's default deny-mount profile. The containment
floor remains the existing namespace, seccomp, cleared-capability and cgroup probes, which must
all pass. The application pod has no MAC_ADMIN, host paths or host namespaces. A separately administered,
immutable node setup installs the named profile; missing profiles prevent scheduling/startup.
No global AppArmor disablement or default-profile replacement belongs to this change.

The exact profile bytes and installation mechanism must pass parser and real-node negative checks
before deployment. Do not claim this design proves an LSM profile before those observations exist.

The default runtime's masked proc submounts prevent a less-privileged nested user namespace from
mounting its own proc filesystem. The bootstrap therefore requires the exact enforced AppArmor
label and exactly one private, non-propagating procfs mount before any mutation. It mounts a
fresh proc filesystem at the fixed /proc path while it
still has setup authority, preserving nosuid/nodev/noexec. This is the container's private PID
and mount namespace, never host procfs. The AppArmor profile replaces the removed path masks
with explicit denials for kernel memory, keys, timers, firmware, power controls and writes to
non-user sysctl trees and proc bus/fs/irq controls. Only the namespace-local user sysctl tree
remains writable for bubblewrap's non-nestable namespace setup. Missing enforcement refuses
startup; this does not request a privileged pod or a globally unmasked runtime configuration.

The outer container also requires an explicit seccomp allowlist: Docker and containerd defaults
refuse pivot_root even when SYS_ADMIN permits mount. Derive the ordinary syscall baseline from a
pinned Moby profile, resolve its architecture conditions for the packaged amd64 runtime, and omit
all capability-conditional allowances. Add only clone/unshare, mount/unmount, pivot_root/chroot,
namespace hostname setup and the existing quota calls. Keep the default action EPERM and the
existing argument-restricted ordinary rules. In particular, no module loading, bpf, perf, host
clock mutation or kernel-log authority is added. The child's existing seccomp filter remains an
additional inherited restriction. Install the immutable OCI seccomp file under the kubelet's
configured profile root and select it by an explicit Localhost reference. A missing file is a
startup refusal. The container bootstrap capability set does not grow for this setup.

The image also supplies a separate Rust node-profile installer. A deployment-owned DaemonSet runs
it with only MAC_ADMIN, a securityfs mount and the exact dedicated kubelet seccomp subdirectory;
it receives no Kubernetes token, host PID/network namespace or host root mount. It installs only
the image's compiled-in versioned bytes, atomically, and refuses an existing path with different
bytes or unsafe ownership/mode. An already loaded AppArmor name without a matching retained
source file is refused. It may reload the identical policy after reboot, verifies enforcement,
and supports read-only readiness checks plus a signal-aware hold mode. No uninstallation or
retirement occurs while an application might use the profiles. Application pods never receive
the installer's authority or mounts. This host prerequisite is an explicit private deployment
step, not something a user workspace can request.

## Verification and rollout

The published image must prove ordinary/quota startup compatibility, bootstrap refusal outside a
private finite delegation, final daemon/worker capabilities, unchanged enclosing ceilings,
non-root refusal to raise those ceilings, actual delegated host and wire conformance, PTY input,
resize, output, cancellation and whole-tree cleanup. The AppArmor check must deny an undeclared
filesystem mount while admitting the required setup. Existing seccomp/no-egress/non-nestable-userns tests
remain decisive. An independent security review precedes publication.

Generic chart composition selects this entrypoint only through an explicit opt-in, supplies an
empty writable temporary directory and a configured local security profile, and preserves current
durable state and workspace quota volumes. The private deployment installs the node prerequisite,
then selects the immutable runtime/chart and a bounded no-network terminal profile. Its rollback
restores the prior image/startup/profile selection while retaining the same durable volumes.
Existing workspaces are preserved; active test terminals close before rollout or rollback.

The final product acceptance observes real terminal output over its browser WebSocket, plus
editable Files and both Agent replies. No terminal profile is advertised until the execution
runtime has been proved. Credential recovery is independently owned by Connectors and its consumer.
