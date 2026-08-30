# Spike: egress enforcement mechanism

**Status:** spike result · **Date:** 2026-08-30

Answers what [design 10](10-destination-bound-egress.md) § 4 left open and
[ADR 0013](../../adr/0013-egress-apertures-are-declared-by-the-operator.md) deferred. Every claim is
a command run on one host and its real output, from a prototype outside this repository.

## 1 Question

Can an unprivileged substrate daemon give a confined child TCP reach to exactly one pinned
`host:port` and nothing else — **without CAP_NET_ADMIN in the host network namespace and without
running as root**? Root is disqualified by construction: `crates/substrate-host/src/probe.rs:49-51`
withholds `exec` when `effective_uid() == 0`.

## 2 Host

| Fact | Value |
|---|---|
| Kernel, distro | `6.6.144-3-MANJARO`, Manjaro Linux `BUILD_ID=rolling` |
| Prototype daemon identity | `uid=1000`, never root, no `CAP_NET_ADMIN` |
| `bwrap --version` | `bubblewrap 0.11.2` |
| `kernel.unprivileged_userns_clone` | `1` |
| `kernel.apparmor_restrict_unprivileged_userns` | **absent** (no such file) |

`bwrap --help` confirms design 10 § 4: `--userns FD`, `--userns2 FD`, `--pidns FD`, `--block-fd FD`,
`--info-fd FD` — and **no `--netns`**.

## 3 What was tried

### 3.1 Option (c) — per-run forwarder in the sandbox netns · **works**

**A sibling cannot `setns` the obvious way.** bwrap nests a *second* user namespace, so the child's
`ns/user` is not the one owning the netns, and capabilities flow only to descendants. Joining the
**owning** userns — reachable only through `ioctl(netns_fd, NS_GET_USERNS)` — does work, because its
owner uid is the daemon's own and the daemon sits in its parent userns:

```console
$ nsenter --user=/proc/<child>/ns/user --net=/proc/<child>/ns/net --preserve-credentials ...
setns(3, CLONE_NEWUSER) = 0
setns(4, CLONE_NEWNET)  = -1 EPERM (Operation not permitted)
child ns/user            : user:[4026533899]
netns owner userns       : user:[4026533826]     # ioctl(netns_fd, NS_GET_USERNS)
owner uid of that userns : 1000
$ # …and against that owning userns instead:
setns(owner_userns, CLONE_NEWUSER) = 0 errno=0
setns(netns, CLONE_NEWNET)         = 0 errno=0
now in netns: net:[4026533831] uid: 0 CapEff: 000001ffffffffff
```

No netlink write is needed: bwrap maps the caller's uid and already brings `lo` up. An unprivileged
userns *can* do it anyway — `unshare -Ur --net … 'ip link set lo up'`, then `BIND+CONNECT OK on
('127.0.0.1', 18080)`; without a uid map `CapEff` is `0` and `ip` gets `{error=-EPERM}`.

**The child reaches the pinned destination and nothing else** — one run, five connects, aperture
pinned to `example.com` resolved once to `172.66.147.243:80`:

| Probe | Result |
|---|---|
| aperture, `127.0.0.1:18080` | **REACHED** — `HTTP/1.1 200 OK`, 88.4 ms |
| second destination `1.1.1.1:443` | REFUSED, errno 101 `Network is unreachable` |
| the pinned IP `172.66.147.243:80` **direct** | REFUSED, errno 101 `Network is unreachable` |
| host loopback, second port `127.0.0.1:19999` | REFUSED, errno 111 `Connection refused` |
| host LAN address `<lan>:22` | REFUSED, errno 101 `Network is unreachable` |

The floor is unchanged: inside the sandbox `ip -o link show` lists `lo` only, `ip route show` is
empty, and `ss -ltn` shows exactly one listener — the aperture.

**One long-lived process suffices.** `setns(CLONE_NEWUSER)` from a thread returns `EINVAL` (errno
22), from a single-threaded process `0`. So a short-lived helper enters the netns, creates the
listening socket *there*, hands the descriptor back over `SCM_RIGHTS` and exits — logging
`handed-back-listener=('127.0.0.1', 18080) host-netns=net:[4026531840] setup_ms=1.6` — while the
forwarder never leaves the host netns and dials out from it.

**TLS crosses intact; the sandbox cannot verify it.** Pinned to `example.com:443` the child got
`"verify=off": {"handshake": "OK", "tls": "TLSv1.3", "first": "HTTP/1.1 200 OK"}` — handshake and
SNI are byte-transparent — against `"verify=on": {"handshake": "FAIL", "err":
"SSLCertVerificationError: … unable to get local issuer certificate"}` with
`"ca_bundle_present": false`: the sandbox has no `/etc` (design 10 § 2) and so no CA bundle.

### 3.2 Option (b) — descriptor at the child · not pursued, one fact recorded

(c) works and design 10 § 4 already rates (b) a poor consumer fit. Worth keeping: **bwrap passes an
arbitrary inherited descriptor through** — a socketpair at fd 4 appeared in the sandbox as
`4 -> socket:[32298073]`. (b)'s blocker is the vendor process, not bubblewrap.

### 3.3 Option (a) — veth + nftables · **refuted for this daemon**

Four attempts as uid 1000; the last closes the only unprivileged-looking path there was:

```console
$ ip link add veth-spike0 type veth peer name veth-spike1
RTNETLINK answers: Operation not permitted
$ nft list ruleset
Operation not permitted (you must be root)
$ ip link set veth1 netns <host pid>          # from inside an unprivileged netns
openat("/proc/<host pid>/ns/net", O_RDONLY) = -1 EACCES (Permission denied)
$ RTM_SETLINK(veth1, IFLA_NET_NS_FD=<retained host netns fd>)
netlink error -1 (Operation not permitted)
```

(a) needs `CAP_NET_ADMIN` in the host netns: a privileged helper with its own trust boundary.

## 4 Verdict

**ADR 0013 should use mechanism (c)** — as design 10 § 4 recommended, now with evidence. The daemon
stays unprivileged, the kernel floor is untouched, and the child speaks a loopback base URL, which is
what `codex` and `claude` accept. Two details are load-bearing and both fail silently:

- **`ioctl(netns_fd, NS_GET_USERNS)`, not `/proc/<pid>/ns/user`.** Joining the child's own user
  namespace returns `EPERM` and reads like kernel policy when it is an addressing mistake.
- **The netns comes from bwrap's `--info-fd` `child-pid`, never the spawned pid.**
  `/proc/<bwrap pid>/ns/net` is `net:[4026531840]`, the **host** namespace, and
  `crates/substrate-host/src/process.rs:393` uses `child.id()` — the bwrap pid. Using it here binds
  the aperture on host loopback, exposed to everything on the machine.

## 5 What it costs per run

| | Measured |
|---|---|
| Long-lived processes added | **1** forwarder, plus a helper that exits after `setup_ms=1.6` |
| Forwarder resident set (Python prototype) | ~12 MB; a Rust forwarder should be well below |
| TTFB direct from the host netns, n=40 | median **1.07 ms**, min 0.41, p95 8.01 |
| TTFB through the aperture, same destination, n=40 | median **4.82 ms**, min 1.19, p95 9.94 |
| Added critical-path syscalls | `openat` + `ioctl` + 2 × `setns` + `sendmsg`/`recvmsg` |
| Host requirements | unprivileged user namespaces — already required for bwrap. No sysctl change, no `CAP_NET_ADMIN`, no root, no nftables |

One extra loopback hop; the p95 ranges overlap, and the internet destination cost 88.4 ms end to end.

## 6 What the implementing story must still prove

| # | Must prove |
|---|---|
| 1 | **The forwarder dies with the run.** Observed failure to copy: after the sandbox exited, `forwarder alive? yes`, still holding 2 sockets pinning the dead netns. Join the run's cgroup **before** opening the listening socket, so `cgroup.kill` reaps it |
| 2 | **The listener is created at the existing spawn barrier** `--block-fd` (`crates/substrate-host/src/process.rs:947-948`, `:398-402`) — never before the aperture exists or after it is gone |
| 3 | **The probe exercises the mechanism** in a throwaway sandbox at startup and publishes `exec.egress-apertures` only then, never the destination's liveness (design 10 § 9 decision 6) |
| 4 | **A CA bundle decision.** § 3.1 shows TLS verification failing for want of `/etc`; an HTTPS endpoint needs a read-only CA bundle mount or the aperture unlocks nothing. Design 10 § 9 decision 2 covered the `/etc/hosts` half only. **Answered: a generated per-run snapshot of an operator-configured anchor — [design 10 § 11](10-destination-bound-egress.md#11-what-shipped-and-what-did-not)** |
| 5 | **The delegated-lane vectors** of design 10 § 7 — `declared-aperture-is-reachable` and `undeclared-destination-is-unreachable` as one run with two connects in the § 3.1 shape, plus `applied-aperture-is-observed` |
| 6 | **Byte accounting lives in the forwarder** — the only place that sees the bytes, and what `exec.aperture-byte-limit` refuses on |
| 7 | **The aperture pins a TCP tuple, not an HTTP identity.** A child may send any `Host` header; the bytes still go to the pinned address |

## 7 What could not be tested here and needs a different host

| Untested | Why it matters |
|---|---|
| Hosts restricting unprivileged user namespaces — `unprivileged_userns_clone=0`, `max_user_namespaces=0`, AppArmor's `apparmor_restrict_unprivileged_userns` (absent here) | The mechanism inherits bubblewrap's existing requirement, so such a host already has no `exec`; confirm the refusal is that one and not a new shape |
| SELinux-enforcing hosts | `setns` and the `SCM_RIGHTS` handback are both policy-visible |
| Kernels older than 4.9 | `NS_GET_USERNS` does not exist there and the mechanism has no fallback |
| The delegated cgroup lane | Forwarder-in-cgroup and whole-tree kill were not exercised; § 6 row 1 is the case to write |
| IPv6 destinations and a real model endpoint | Only IPv4 and `example.com` were used |

## 8 What the implementation proved, and where

`story:destination-bound-egress` landed on 2026-08-30. Row by row against § 6, and honest about the
one lane this host cannot run:

| § 6 | Proved by | Executed here? |
|---|---|---|
| 1 forwarder dies with the run | the forwarder writes `0` to the run's `cgroup.procs` before the listening socket exists, and `InstalledAperture::drop` kills it besides | **no** — the cgroup half needs a delegated subtree |
| 2 listener at the `--block-fd` barrier | installed between `cgroup.attach_tree` and `release_barrier` (`crates/substrate-host/src/process.rs`) | yes — `egress::tests` open a real sandbox at that barrier |
| 3 probe exercises the mechanism | `egress::mechanism_is_provable` runs a throwaway sandbox and connects through it from inside | yes |
| 4 CA bundle | § 7 above | yes, as a file-generation case; no TLS handshake was run |
| 5 delegated-lane vectors | `check_confined_apertures` in `crates/substrate-daemon/tests/runtime_vectors.rs` | **no** — absent without `SUBSTRATE_VECTORS_CGROUP_ROOT` |
| 6 byte accounting in the forwarder | a shared page the relays `fetch_add` into; asserted non-zero in `applied_aperture_is_observed` | yes |
| 7 a TCP tuple, not an HTTP identity | the relay dials a `sockaddr_in` fixed before the fork; nothing a child sends can reach it | yes, by construction — no `Host`-header case was written |

**Trap 2 bites, and a test catches it.** Pointing the install at `child.id()` — the bubblewrap pid,
whose `ns/net` is the host namespace — makes
`egress::tests::the_mechanism_is_proven_in_a_throwaway_sandbox` fail: the aperture is bound on host
loopback, and nothing inside the sandbox can reach it.

**Trap 1 did not reproduce for the sandbox argv used here, and the implementation takes the safe
route anyway.** Swapping `ioctl(netns_fd, NS_GET_USERNS)` for `/proc/<child>/ns/user` left all nine
cases green, so bubblewrap did *not* nest a second user namespace for the throwaway argv — fewer
binds than a real run. `NS_GET_USERNS` returns the owning namespace in both cases and
`/proc/<pid>/ns/user` only sometimes does, so the ioctl stays: this is a trap that would appear on
some argv and not others, which is exactly the kind that ships.

**One trap this spike did not name, found while implementing.** `std::io::pipe` sets `O_CLOEXEC`, so
a sandbox handed `--info-fd` or `--block-fd` from one reports **nothing** and blocks **forever** —
and both failures are silent, because a harness that reads no `child-pid` looks exactly like a
harness that decided not to run. The first version of these tests "passed" in 0.00 s while opening no
sandbox at all. `pipe2` with no flags is what the daemon's own barrier already uses, and is what the
probe and the cases use now; releasing that barrier is a **written byte**, never a close, because
a close only releases the sandbox if every forked copy of the write end is gone.
