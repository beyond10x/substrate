---
format: aep.planning-md/1
id: review-result:adversary-waveb-u3-pass-1
kind: review-result
status: active
title: Adversary pass 1, wave B unit u3 (host confinement pair)
relations:
- reviews: story:seccomp-denies-af-vsock
revision: 1
---
# Adversary pass 1 — wave B unit u3, host confinement pair

Worktree `wt-ec9fa2b38e4d` at `06484e9`, base `0c858f0`.

```
unit: u3
verdict: red
cases: executed 96→97, red 1
origin: introduced 0, pre-existing 1, undecided 0
needs-coordinator: yes
```

`git diff --stat` is empty; the only change is one untracked test file,
`crates/substrate-host/tests/qrtr_family_confinement.rs`. No implementation file touched.

## A second live escape, measured

`AF_QIPCRTR` (family 42) is permitted by the confinement seccomp profile and **is not confined by a
network namespace**. Measured at the bwrap layer on this host, with the `qrtr` module loaded:

| observation | result |
|---|---|
| confined sandbox (netns `4026534314`) ↔ host process (init netns `4026531840`) | datagrams exchanged **bidirectionally** |
| two mutually-isolated sandboxes (netns `4026534396` ↔ `4026534475`) | datagram exchanged |
| `AF_INET` to a host loopback service over the same boundary | **refused** |

So the namespace confines `AF_INET` and does not confine `AF_QIPCRTR`.

Inside a real admitted exec, `socket(AF_QIPCRTR, SOCK_DGRAM, 0)` returns a live descriptor:

```
test a_confined_process_cannot_open_an_af_qipcrtr_socket ...
panicked at crates/substrate-host/tests/qrtr_family_confinement.rs:176:5:
a confined process opened an AF_QIPCRTR socket; no network namespace confines qrtr, so on a host
with the qrtr transport that socket reaches other domains (host and sibling sandboxes, measured at
the bwrap layer): "QRTR-OPENED 3\n"
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out
```

The case runs a "present" exec first (`python3 -c print('py-ok')`, exit 0) so a bare failure cannot
be an absent-interpreter artefact. It early-returns without `SUBSTRATE_VECTORS_CGROUP_ROOT`, so the
release package gate stays green at 97 and the red appears only in the real sandbox.

Delegated lane with `--no-fail-fast`: `passed=96 failed=1 executed=97`, every other test green.

**What reaches it:** the exec path itself — `start_exec` runs the client's own argv inside the
sandbox, so any client-supplied command opens the socket. Production-node reachability depends on
the `qrtr` module being loaded there; **nothing found** confirming it on the EKS nodes `STATUS.md`
names. That is the same host-dependency caveat the unit accepted for `AF_VSOCK`, which needs a vsock
transport.

**Origin `pre-existing`:** `git show 0c858f0:.../seccomp.rs` denies only `AF_UNIX`, so
`AF_QIPCRTR` opened at base too. The unit's diff did not create the hole, but it shipped a
`FAMILY_POLICY` document asserting a family stance that misses it. `FAMILY_POLICY`'s own doc names
`AF_ALG` as "the next it would examine" — and `AF_ALG` is refused by the kernel inside the sandbox
(`ESOCKTNOSUPPORT`) and is not a cross-domain channel, while `AF_QIPCRTR` is an actual live escape
ranking ahead of it. The survey that produced the table did not empirically enumerate reachable
families.

**Named fix:** `jump(AF_QIPCRTR, 0, 1); RET_K deny` in `filters`, plus a `FAMILY_POLICY` row with
`denied=true`.

## Attacked and could not break

- **The `AF_VSOCK` denial holds for the family.** The filter matches family at BPF offset 16
  regardless of type or protocol, so `SOCK_STREAM`, `SOCK_DGRAM`, `SOCK_SEQPACKET` and every protocol
  all get `EACCES`. No type-specific hole.
- **`AF_VSOCK` by another route.** `/dev/vsock` is not a `connect()` route without an `AF_VSOCK`
  socket; the exec builder `env_clear`s and passes only controlled fds, so no pre-opened vsock fd
  crosses in.
- **The x32 bypass.** `normalize_syscall_number` masks `X32_SYSCALL_BIT` before the number compare,
  so x32 `socket` maps onto the native family jumps. No x32-only family hole.
- **`memory.oom.group` reaches every exec.** A single `Cgroup::create` closure writes it
  unconditionally; pty and plain execs both route through it; no alternate cgroup-limit writer omits
  it.
- **The 0.6.0 confinement floor is unchanged** — `--unshare-user --disable-userns` and the three
  assertions over it.
- **Unmeasured-OOM naming.** `unmeasured_oom_kills` reads `memory.events` before `reconcile_cgroup`
  removes the directory; `memory_exhausted` fires from either source.
- **The one-CPU `cpu.max` clamp reasoning verified correct** — `capability.json:38-56` is
  `additionalProperties: false` and the bundle is frozen, so publishing it needs a successor bundle
  and an ADR. Not re-reported, per the brief.

## Self-reported charter deviation

The adversary wrote `/tmp/x_before.txt` and `/tmp/x_after.txt` (throwaway test-name diffs), against
the standing rule that nothing goes in `/tmp`. Both removed. Reported by the agent itself.

```findings
- file: crates/substrate-host/src/seccomp.rs
  line: 90
  category: judgement
  severity: blocker
  verdict: needs-revision
  origin: pre-existing
  message: >-
    The confinement seccomp profile permits AF_QIPCRTR (family 42), a socket family the network
    namespace does not confine. Measured at the bwrap layer on this host with the qrtr module loaded,
    a confined sandbox exchanged datagrams bidirectionally with the host netns and with a second
    isolated sandbox, while AF_INET over the same boundary was refused, and
    crates/substrate-host/tests/qrtr_family_confinement.rs:176 shows socket(AF_QIPCRTR,SOCK_DGRAM,0)
    returns a live fd inside a real admitted exec instead of EACCES. The unit's FAMILY_POLICY names
    AF_ALG as the next family to examine, but AF_QIPCRTR is a live escape ranking ahead of it. Fix is
    one jump against libc::AF_QIPCRTR in filters plus a denied FAMILY_POLICY row. Production-node
    reachability depends on the qrtr module being loaded there, which is not shown.
```
