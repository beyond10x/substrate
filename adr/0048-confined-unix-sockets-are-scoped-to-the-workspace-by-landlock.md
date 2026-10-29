---
status: proposed
date: 2026-10-29
---

# ADR 0048: confined Unix sockets are scoped to the workspace by Landlock

Amends [ADR 0015](0015-declared-host-roots-carry-no-host-ipc.md) and decision 2 of
[ADR 0039](0039-confined-sockets-are-an-argument-allowlist.md). Story:
`story:confined-unix-sockets-scoped-to-the-workspace`.

## Context

The operator decided on 2026-10-29 that a confined process may create Unix-domain (`AF_UNIX`)
sockets, provided `bind` and `connect` stay inside the workspace, so that tmux, screen and language
servers work in a sandbox. A host service whose socket is visible through a read-only mount must
still be unreachable.

Today the seccomp profile (`crates/substrate-host/src/seccomp.rs`, `filters`) refuses
`socket(AF_UNIX, …)` with `EACCES` and admits `socketpair(AF_UNIX)` only as `SOCK_STREAM` or
`SOCK_SEQPACKET`. ADR 0015 gives the reason: a read-only bind mount does not stop `connect(2)` to a
socket file inside it. The confinement-floor probe (`probe_bubblewrap` in
`crates/substrate-host/src/probe.rs`) asserts that the `socket(2)` call itself is refused before
`exec.no-egress` is published.

So the question is which kernel mechanism can limit the pathname sockets a confined process may
bind and connect to, without privilege. The daemon runs unprivileged (`probe` withholds `exec` for
uid 0), so a mechanism that needs a capability in the initial user namespace is not available.

## Evidence

Measured on 2026-10-29 on the workstation that wrote this ADR: kernel `6.18.49`, active LSMs
`capability,landlock,lockdown,yama,bpf,apparmor` (`/sys/kernel/security/lsm`), bubblewrap 0.12.0.
The probe was a throwaway script that calls the Landlock syscalls directly and runs each case in a
forked child. "Here" marks a claim observed on that machine. "Docs" marks a claim taken only from
the cited source.

| # | Claim | Source | Status |
|---|---|---|---|
| E1 | A read-only bind mount and a fresh network namespace do not stop `connect` to a pathname socket: `bwrap --unshare-user --disable-userns --unshare-net … --ro-bind <dir> /runtime/sentinel -- socat - UNIX-CONNECT:/runtime/sentinel/host.sock` without the seccomp profile exits 0. | [unix(7)](https://man7.org/linux/man-pages/man7/unix.7.html), § *Pathname socket ownership and permissions*: connecting needs write permission on the socket, and nothing there involves the mount's read-only flag. | Here |
| E2 | The network namespace isolates the abstract socket namespace and not the pathname one. A child in a new user and network namespace gets `ECONNREFUSED` for an abstract name its parent listens on, and connects to the parent's pathname socket. | [network_namespaces(7)](https://man7.org/linux/man-pages/man7/network_namespaces.7.html): "network namespaces isolate the UNIX domain abstract socket namespace". | Here |
| E3 | `LANDLOCK_ACCESS_FS_MAKE_SOCK` confines `bind(2)` of a pathname socket. With it handled and allowed only beneath one directory, `bind` there succeeds and `bind` in a sibling directory is `EACCES`. Landlock ABI 1, Linux 5.13. | [Landlock userspace API](https://docs.kernel.org/userspace-api/landlock.html): "Create (or rename or link) a UNIX domain socket". | Here |
| E4 | No Landlock right up to ABI 7 controls `connect` to a pathname socket. A domain that handles all sixteen ABI 7 filesystem rights and allows nothing beneath the socket's directory still connects to it. | [Landlock, Linux 6.18](https://docs.kernel.org/6.18/userspace-api/landlock.html) describes no such right. | Here |
| E5 | `LANDLOCK_SCOPE_ABSTRACT_UNIX_SOCKET` refuses `connect` to an abstract socket created outside the domain with `EPERM`. It admits an abstract socket created inside the domain and does not affect pathname sockets. Landlock ABI 6, Linux 6.12: commit [`21d52e295ad2`](https://git.kernel.org/pub/scm/linux/kernel/git/torvalds/linux.git/commit/?id=21d52e295ad2afc76bbd105da82a003b96f6ac77), merged by [`e1b061b444fb`](https://git.kernel.org/pub/scm/linux/kernel/git/torvalds/linux.git/commit/?id=e1b061b444fb01c237838f0d8238653afe6a8094) (`landlock-6.12-rc1`). | [Landlock userspace API](https://docs.kernel.org/userspace-api/landlock.html), § *Scope flags*; absent from the [6.11 page](https://docs.kernel.org/6.11/userspace-api/landlock.html). | Here (behaviour); docs (version) |
| E6 | `LANDLOCK_ACCESS_FS_RESOLVE_UNIX` controls the lookup of a pathname socket in `connect(2)`, and in `sendmsg(2)` with an explicit address. Access is granted when the socket's path is beneath a rule that allows the right, or when the socket was created in the same domain. A denial is `EACCES`. Landlock ABI 9, Linux 7.1: commit [`ae97330d1bd6`](https://git.kernel.org/pub/scm/linux/kernel/git/torvalds/linux.git/commit/?id=ae97330d1bd6a97646c2842d117577236cb40913), "Hook into the path lookup in unix_find_bsd()", merged by [`b8f82cb0d84d`](https://git.kernel.org/pub/scm/linux/kernel/git/torvalds/linux.git/commit/?id=b8f82cb0d84d00c04cdbdce42f67df71b8507e8b) (`landlock-7.1-rc1`). | [Landlock, Linux 7.1](https://docs.kernel.org/7.1/userspace-api/landlock.html) documents it; [Linux 7.0](https://docs.kernel.org/7.0/userspace-api/landlock.html) does not. | Docs |
| E7 | This workstation does not have E6. `landlock_create_ruleset(NULL, 0, LANDLOCK_CREATE_RULESET_VERSION)` answers ABI 7, and a ruleset that handles bit 16, the bit after `LANDLOCK_ACCESS_FS_IOCTL_DEV`, is `EINVAL`. | Same as E6. | Here |
| E8 | A Landlock domain that handles any filesystem right stops bubblewrap from building its sandbox. Under a domain handling only `MAKE_SOCK`, bubblewrap exits 1 with `Failed to make / slave: Operation not permitted`, and without the domain the same argv exits 0. The ruleset must therefore be applied after bubblewrap's mounts, inside the sandbox. | [Landlock userspace API](https://docs.kernel.org/userspace-api/landlock.html), § *Current limitations*, *Filesystem topology modification*. | Here |
| E9 | Seccomp user notification cannot make this decision safely. A supervisor that reads the `sockaddr` from the target's memory races the target, and the mechanism "must not be used to make security policy decisions about the system call". | [seccomp_unotify(2)](https://man7.org/linux/man-pages/man2/seccomp_unotify.2.html), § *Design goals; use of SECCOMP_USER_NOTIF_FLAG_CONTINUE* and § *Caveats regarding /proc/tid/mem*. | Docs |
| E10 | The cgroup `connect_unix` and `sendmsg_unix` BPF hooks (Linux 6.7, merge [`d2dc885b8c9d`](https://git.kernel.org/pub/scm/linux/kernel/git/torvalds/linux.git/commit/?id=d2dc885b8c9ddb6fc374d93a87f8f2d1b97d2caf)) need a BPF program loaded by a privileged caller. Here `kernel.unprivileged_bpf_disabled` is `2`. | [`unprivileged_bpf_disabled`](https://docs.kernel.org/admin-guide/sysctl/kernel.html#unprivileged-bpf-disabled) | Here (sysctl value); docs (meaning) |

Further options were rejected by reasoning, not by measurement:

- **An AppArmor or SELinux policy** that mediates Unix sockets is host policy, loaded by the
  operator with privilege. It is not something an unprivileged daemon applies per exec or can
  prove per run.
- **Hiding socket files.** Overmounting or removing the sockets found in a declared root at start
  does not hold, because a declared root is a live view: a host service can bind a new socket in
  it after the start.

**Finding.** On Linux before 7.1 (Landlock ABI below 9), no unprivileged mechanism confines
`connect(2)` to a pathname socket. That includes this workstation's `6.18` line. On Linux 7.1 and
later, `RESOLVE_UNIX` does (E6), `MAKE_SOCK` confines `bind` (E3), and the abstract namespace is
closed twice: by the network namespace (E2) and by the abstract scope (E5).

## Decision

1. **A start opts in.** An exec start, and a session start (design 27 publishes exec fields on it),
   may name `unix_sockets: "workspace"`. This is a closed string with one member. A start that does
   not name it runs today's profile byte for byte, and the floor probe, `exec.no-egress` and every
   existing client keep their meaning.
2. **The seccomp profile for an opted-in start admits `socket(AF_UNIX)`** only with type
   `SOCK_STREAM` or `SOCK_SEQPACKET`, compared under the `0xf` mask as ADR 0039 compares
   `socketpair`, and protocol `0`. Datagram and every other type stay `EACCES`. The `socketpair`
   rule, the x32 refusal and the `io_uring_setup` refusal do not change. A stream or seqpacket
   socket resolves a path only in `connect`, so `connect` is the one lookup left to confine.
3. **A Landlock domain is applied inside the sandbox**, after bubblewrap's mounts (E8) and before
   the workload's `execve`, by a launcher that Substrate owns. The launcher is bound read-only by
   descriptor at a reserved mount, which joins the Substrate-owned mounts ADR 0015 forbids a
   declared root to shadow. Its ruleset:
   - handles `LANDLOCK_ACCESS_FS_MAKE_SOCK | LANDLOCK_ACCESS_FS_RESOLVE_UNIX` and no other
     filesystem right, so the rest of the filesystem answers exactly as it does today;
   - adds one `LANDLOCK_RULE_PATH_BENEATH` rule on `/workspace` that allows both rights;
   - sets `scoped = LANDLOCK_SCOPE_ABSTRACT_UNIX_SOCKET`;
   - requires ABI 9 or later.

   The launcher fails closed: a Landlock call that does not succeed ends the run before the
   workload executes, and the exec is recorded as `exec.unix-sockets-unserved`.
4. **A second probe proves the mechanism, and only that probe publishes the new fact.** It runs
   only when the `exec` floor holds and the kernel answers ABI 9 or later. In one throwaway sandbox
   with the opted-in profile and the launcher, it must observe every one of these:
   - a stream `socket(AF_UNIX)` succeeds;
   - `connect` to the read-only-mounted sentinel host socket of ADR 0015 is `EACCES`;
   - `connect` through a symlink beneath `/workspace` that points at that sentinel is `EACCES`;
   - `bind` outside `/workspace` (in `/tmp`) is `EACCES`;
   - `connect` to an abstract name the daemon listens on is refused;
   - `bind`, `listen` and `connect` beneath `/workspace` succeed.

   Only then does the daemon publish **`exec.unix-sockets`** with `{"scope": "workspace"}`. That
   fact is never published without `exec.no-egress`. `exec.no-egress` still comes from the
   unchanged floor probe and still describes every start that does not opt in. The opted-in
   profile's host-socket unreachability is what the second probe proves.
5. **A missing guarantee is a named refusal (invariant 3).** A start that names `unix_sockets`
   where the fact is absent is refused `exec.unix-sockets-unserved` (501) before dispatch and
   recorded as refused. Absence covers a kernel below 7.1, Landlock not in the active LSM list, a
   failed probe, and every adapter that does not publish the fact (macOS, ADR 0037; the Kubernetes
   namespace driver, ADR 0047). No adapter admits `AF_UNIX` without this fact.
6. **The applied exec record states `unix_sockets: "workspace"`** for an opted-in run, so the record
   says which profile ran.

### Amendments

- **ADR 0015.** "Every confined process runs under a verified seccomp program that denies creation
  of `AF_UNIX`/`AF_LOCAL` sockets" now holds for every start that does not opt in. For an opted-in
  start, the denial becomes the workspace scope of decision 3. The proof that a mounted host socket
  is unreachable becomes decision 4's probe, which reuses ADR 0015's sentinel. ADR 0015's
  consequence, that a mounted socket cannot become undeclared host authority, holds for both.
- **ADR 0039, decision 2.** "`socket(AF_UNIX)` stays refused" now holds unless the start opts in.
  The `socketpair` half is unchanged.

### Blocker on this kernel line

This workstation runs `6.18.49` with Landlock ABI 7 (E7). On it, decision 4's probe does not run,
`exec.unix-sockets` is absent, and every opted-in start is refused `exec.unix-sockets-unserved`. The
delegated-lane cases that prove the scope need a Linux 7.1 or later host. On an older host they are
absent, never reported as passed. Nothing in this ADR is weakened to make the 6.18 line serve it.

## Wire impact

This is a contract change. The next successor bundle after `0.25.0` publishes it. This ADR does not
cut that bundle, and no code serves the field before the bundle exists (invariant 8). The bundle
publishes:

- the optional `unix_sockets` start member on the exec and session starts;
- the optional `exec.unix-sockets` fact (`{"scope": "workspace"}`);
- the applied record member `unix_sockets`;
- the refusal `exec.unix-sockets-unserved` (501), with a refusal vector.

Each is held to a `substrate_wire` constant by `cargo xtask check-bundle`. The change is additive:
no route and no existing member changes.

## Residual

- A socket bound beneath `/workspace` is reachable by other execs that share the workspace (that is
  how a second exec attaches to a tmux server) and by host processes that can open the workspace
  directory.
- A host service that binds its own socket inside a workspace directory becomes reachable from a
  confined process. Placing it there is the operator's act. The confined process cannot place one
  there: a link or rename from a read-only root into the workspace crosses mounts.
- The `RESOLVE_UNIX` semantics in E6 are documentation-only until decision 4's probe has run on a
  Linux 7.1 or later host.

## Consequences

- tmux, screen and language servers work in an opted-in sandbox when their sockets live beneath
  `/workspace`. tmux's default socket directory is beneath `/tmp` (tmux(1), `TMUX_TMPDIR`), so
  tmux needs `TMUX_TMPDIR` set beneath `/workspace`. `/tmp` and datagram sockets are not admitted;
  admitting either is an amendment to this ADR, not a fall-through.
- A new probe and a launcher are added to the confinement path. Neither runs for a start that does
  not opt in.
- On hosts below Linux 7.1 the capability is absent and refused by name.
