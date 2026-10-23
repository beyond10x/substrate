---
status: accepted
date: 2026-10-23
---

# ADR 0039: confined sockets are an argument allowlist

Amends the confinement seccomp profile (`crates/substrate-host/src/seccomp.rs`) recorded under the
safety envelope in `AGENTS.md`. Story:
`story:a-confined-process-cannot-mutate-host-global-kernel-state`.

## Context

The profile refused ten socket families by number and let every other `socket(2)` through with
any type and protocol. Three families stayed allowed because shipped capability needs them:
`AF_INET` and `AF_INET6` (the egress aperture is a TCP listener in the child's network namespace,
and ordinary programs resolve names) and `AF_NETLINK` (glibc reaches `NETLINK_ROUTE` for
`getaddrinfo` and interface enumeration).

Allowing a family with *any* protocol let a confined process ask the kernel for a protocol it does
not have built in, and the kernel answers that request by loading a module into the host's single,
un-namespaced module table, where it stays after the sandbox exits. The `FamilyPolicy.host_state`
rows for these three families recorded this as an accepted residual. One row, `AF_NETLINK`, was
reasoned from `modules.alias` rather than measured.

## Decision

1. **`socket(2)` is an allowlist of (family, protocol) pairs.** Everything not listed is refused
   with `EACCES`, the errno every existing denial of this profile returns:

   | family | allowed protocol argument |
   |---|---|
   | `AF_INET` | `0`, `IPPROTO_TCP`, `IPPROTO_UDP` |
   | `AF_INET6` | `0`, `IPPROTO_TCP`, `IPPROTO_UDP` |
   | `AF_NETLINK` | `NETLINK_ROUTE` (`0`) |

   ICMP and ICMPv6 are not allowed: no shipped path inside the sandbox opens them. The egress
   helper and forwarder that open `AF_INET` stream sockets run outside the child's seccomp program
   (`crates/substrate-host/src/egress.rs`, the `setns` helper). A family absent from the table,
   including every family `DENIED_FAMILIES` named before, is refused.
2. **`socket(AF_UNIX)` stays refused, and `socketpair(2)` admits `AF_UNIX` only as
   `SOCK_STREAM` or `SOCK_SEQPACKET`** (the type compared under the `0xf` mask). Every other type
   is refused with `EACCES`. Before this ADR only `SOCK_DGRAM` was refused, but the kernel creates
   an `AF_UNIX` `SOCK_RAW` pair as a datagram pair, so refusing by one excluded type was not a
   datagram refusal. `socketpair(2)` for any other family is refused with `EACCES` too. It never
   yields a usable pair for those families, and it creates sockets the same way `socket(2)` does.
3. **`ioctl(TIOCSETD)` is refused with `EACCES`.** Pty sessions (ADR 0019) give the child a
   terminal; nothing the runtime ships changes its line discipline.
4. **Arguments are compared on their low 32 bits**, which is the width the kernel reads for the
   family, protocol and ioctl request. **On x86_64, every syscall whose number carries the x32
   bit (`0x40000000`) is refused with `EACCES` before any other check.** The profile used to mask
   that bit off and compare the native number, but some x32 entries are compat numbers of their
   own: x32 `ioctl` is 514, not 16, so it walked past the `TIOCSETD` check. Nothing the runtime
   ships is x32.
5. **`FamilyPolicy.host_state` stays answered for every surveyed family**, and the allowed rows now
   say what the allowlist closes.

## Proof

One delegated case per trigger (`crates/substrate-host/tests/kernel_module_triggers.rs`). The
probe is the test executable itself, copied into the workspace and run inside an admitted exec with
one argument naming the trigger. For each, the call answers `EACCES` inside the sandbox, and the
host's `/proc/modules` read before and after the exec shows no new module:

| trigger | call |
|---|---|
| SCTP over `AF_INET` | `socket(AF_INET, SOCK_STREAM, IPPROTO_SCTP)` |
| a non-`ROUTE` netlink protocol | `socket(AF_NETLINK, SOCK_RAW, NETLINK_ISCSI)` |
| line discipline | `ioctl(<pty>, TIOCSETD, N_HDLC)` |

The refusal is the standing assertion. A module diff alone passes once a module is already
resident, because the load happens at most once per boot. Each case also checks that an allowed
call in the same probe still succeeds (TCP socket, `NETLINK_ROUTE` socket, opening the pty), so an
exec that ran nothing cannot pass.

## Residual

Other kernel autoload paths are not claimed closed by this ADR.

## Consequences

- A confined workload that opened SCTP, raw-protocol, ICMP or non-route netlink sockets now
  receives `EACCES`. That is a capability reduction, recorded here.
- A new family or protocol is a change to the allowlist and to this ADR, not a fall-through.
