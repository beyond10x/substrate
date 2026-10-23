use std::fs::File;
use std::io::{Seek as _, SeekFrom, Write as _};
use std::os::fd::{AsRawFd as _, RawFd};

use crate::DriverError;

const ALLOW: u32 = 0x7fff_0000;
const ERRNO: u32 = 0x0005_0000;
const LD_W_ABS: u16 = 0x20;
const ALU_AND_K: u16 = 0x54;
const JMP_JEQ_K: u16 = 0x15;
const RET_K: u16 = 0x06;
const SOCKET_TYPE_MASK: u32 = 0x0f;

/// `AF_QIPCRTR`, the Qualcomm IPC router, which this crate's `libc` does not name on this target.
///
/// Its `AF_MAX` is 43, so its table predates this family, `AF_SMC` and `AF_MCTP` alike. The number
/// is the kernel's `include/linux/socket.h` value and is part of a stable ABI: a family number is
/// never reassigned, because every `socket(2)` ever compiled against it would change meaning.
#[cfg(test)]
const AF_QIPCRTR: libc::c_int = 42;

/// The three further families this crate's `libc` does not name, for the same reason.
///
/// The survey measured all three opening inside the confinement argv, so they are spelled out
/// here rather than waited for.
#[cfg(test)]
const AF_KCM: libc::c_int = 41;
#[cfg(test)]
const AF_SMC: libc::c_int = 43;
#[cfg(test)]
const AF_MCTP: libc::c_int = 45;

/// **The `socket(2)` allowlist of ADR 0039, and the one list its BPF chain is generated from.**
///
/// A family absent from this list is refused, whatever its type and protocol. A listed family is
/// admitted only with one of its listed protocol arguments; any other protocol is refused. Each
/// listed family is allowed because shipped capability needs it: `AF_INET` and `AF_INET6` carry
/// the egress aperture and name resolution, and `AF_NETLINK`'s `NETLINK_ROUTE` is what glibc
/// reaches for `getaddrinfo` and interface enumeration. ICMP and `ICMPv6` are not listed: no
/// shipped path inside the sandbox opens them (the egress helper and forwarder run outside this
/// program).
///
/// Written out once and turned into jumps by `assemble`, because hand-counted classic-BPF offsets
/// are where an off-by-one hides, and an off-by-one in a refusal chain silently *allows*.
/// `seccomp::tests` asserts the allowed half of `FAMILY_POLICY` names exactly these families.
const ALLOWED_SOCKETS: [(&str, libc::c_int, &[libc::c_int]); 3] = [
    (
        "AF_INET",
        libc::AF_INET,
        &[0, libc::IPPROTO_TCP, libc::IPPROTO_UDP],
    ),
    (
        "AF_INET6",
        libc::AF_INET6,
        &[0, libc::IPPROTO_TCP, libc::IPPROTO_UDP],
    ),
    ("AF_NETLINK", libc::AF_NETLINK, &[libc::NETLINK_ROUTE]),
];

/// Offsets of the syscall arguments in `struct seccomp_data`. Each comparison reads the low 32
/// bits, which is the width the kernel reads for a family, a protocol, a socket type and an
/// ioctl request (both supported targets are little-endian).
const ARGUMENT_0: u32 = 16;
const ARGUMENT_1: u32 = 24;
const ARGUMENT_2: u32 = 32;

#[cfg(target_arch = "x86_64")]
const X32_SYSCALL_BIT: u32 = 0x4000_0000;

#[cfg(target_arch = "x86_64")]
const AUDIT_ARCH: u32 = 0xc000_003e;
#[cfg(target_arch = "aarch64")]
const AUDIT_ARCH: u32 = 0xc000_00b7;
#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
compile_error!("the confinement seccomp profile has no audited architecture for this target");

/// A jump target in the program `filters` builds, resolved to an offset by `assemble`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Label {
    /// The instruction after this one.
    Next,
    /// The shared `RET` that refuses with the profile's errno.
    Deny,
    /// The shared `RET` that allows the call.
    Allow,
    /// The argument checks for `socket(2)`.
    Socket,
    /// The protocol check for the `ALLOWED_SOCKETS` entry at this index.
    Protocols(usize),
    /// The argument checks for `socketpair(2)`.
    Socketpair,
    /// The argument check for `ioctl(2)`.
    Ioctl,
}

/// One instruction of the program before its jump offsets are resolved.
enum Op {
    Statement(u16, u32),
    Jump { value: u32, yes: Label, no: Label },
    Mark(Label),
}

fn number(value: libc::c_long) -> u32 {
    u32::try_from(value).expect("a syscall number fits the 32-bit seccomp_data field")
}

fn word(value: libc::c_int) -> u32 {
    // Reinterpreting the bits is the point: the kernel compares the argument's low word.
    value.cast_unsigned()
}

pub(crate) fn profile() -> Result<File, DriverError> {
    let deny = ERRNO | u32::try_from(libc::EACCES).unwrap_or(13);
    let filters = filters(deny);
    let mut file = tempfile::tempfile().map_err(|error| failed(&error))?;
    // SAFETY: `filters` is a contiguous array of the kernel ABI type for this slice's lifetime.
    let bytes = unsafe {
        std::slice::from_raw_parts(
            filters.as_ptr().cast::<u8>(),
            std::mem::size_of_val(filters.as_slice()),
        )
    };
    file.write_all(bytes)
        .and_then(|()| file.seek(SeekFrom::Start(0)).map(|_| ()))
        .map_err(|error| failed(&error))?;
    clear_cloexec(file.as_raw_fd())?;
    Ok(file)
}

/// The confinement program.
///
/// In order: refuse a foreign architecture; refuse `io_uring_setup`; send `socket`, `socketpair`
/// and `ioctl` to their argument checks; allow everything else. The argument checks are:
///
/// * `socket`: the family must be in `ALLOWED_SOCKETS` and the protocol one of its listed values
///   (ADR 0039). `AF_UNIX` is not listed, so it stays refused for the filesystem reason
///   `FAMILY_POLICY` records.
/// * `socketpair`: `AF_UNIX` only, and not as a datagram pair. A Unix datagram socketpair is not
///   confined to its peer: `connect(2)` can retarget either endpoint to a mounted host socket.
///   Connection-oriented pairs remain available for process-local IPC. No other family yields a
///   usable pair, and each creates its sockets the way `socket(2)` does (ADR 0039).
/// * `ioctl`: `TIOCSETD` is refused; no shipped path changes a terminal's line discipline.
fn filters(deny: u32) -> Vec<libc::sock_filter> {
    let mut program = vec![
        Op::Statement(LD_W_ABS, 4),
        Op::Jump {
            value: AUDIT_ARCH,
            yes: Label::Next,
            no: Label::Deny,
        },
        Op::Statement(LD_W_ABS, 0),
    ];
    // x32 uses the x86_64 audit architecture with an ABI bit in the syscall number. Compare the
    // underlying call so an x32 `socket`, `socketpair` or `ioctl` cannot skip the native numbers.
    #[cfg(target_arch = "x86_64")]
    program.push(Op::Statement(ALU_AND_K, !X32_SYSCALL_BIT));
    for (syscall, target) in [
        (libc::SYS_io_uring_setup, Label::Deny),
        (libc::SYS_socket, Label::Socket),
        (libc::SYS_socketpair, Label::Socketpair),
        (libc::SYS_ioctl, Label::Ioctl),
    ] {
        program.push(Op::Jump {
            value: number(syscall),
            yes: target,
            no: Label::Next,
        });
    }
    program.push(Op::Statement(RET_K, ALLOW));

    program.push(Op::Mark(Label::Socket));
    program.push(Op::Statement(LD_W_ABS, ARGUMENT_0));
    let last = ALLOWED_SOCKETS.len() - 1;
    for (index, (_, family, _)) in ALLOWED_SOCKETS.iter().enumerate() {
        program.push(Op::Jump {
            value: word(*family),
            yes: Label::Protocols(index),
            no: if index == last {
                Label::Deny
            } else {
                Label::Next
            },
        });
    }
    for (index, (_, _, protocols)) in ALLOWED_SOCKETS.iter().enumerate() {
        program.push(Op::Mark(Label::Protocols(index)));
        program.push(Op::Statement(LD_W_ABS, ARGUMENT_2));
        let last = protocols.len() - 1;
        for (position, protocol) in protocols.iter().enumerate() {
            program.push(Op::Jump {
                value: word(*protocol),
                yes: Label::Allow,
                no: if position == last {
                    Label::Deny
                } else {
                    Label::Next
                },
            });
        }
    }

    program.extend([
        Op::Mark(Label::Socketpair),
        Op::Statement(LD_W_ABS, ARGUMENT_0),
        Op::Jump {
            value: word(libc::AF_UNIX),
            yes: Label::Next,
            no: Label::Deny,
        },
        Op::Statement(LD_W_ABS, ARGUMENT_1),
        Op::Statement(ALU_AND_K, SOCKET_TYPE_MASK),
        Op::Jump {
            value: word(libc::SOCK_DGRAM),
            yes: Label::Deny,
            no: Label::Allow,
        },
        Op::Mark(Label::Ioctl),
        Op::Statement(LD_W_ABS, ARGUMENT_1),
        Op::Jump {
            value: u32::try_from(libc::TIOCSETD).expect("TIOCSETD fits the 32-bit request word"),
            yes: Label::Deny,
            no: Label::Allow,
        },
        Op::Mark(Label::Deny),
        Op::Statement(RET_K, deny),
        Op::Mark(Label::Allow),
        Op::Statement(RET_K, ALLOW),
    ]);
    assemble(&program)
}

/// Resolves every label to a forward jump offset and emits the classic-BPF program.
///
/// Classic BPF jumps are counts of instructions to skip and can only go forward. A label that is
/// behind its jump, missing, or more than 255 instructions ahead is a bug in `filters`, so it
/// panics rather than emitting a program whose refusal lands somewhere else.
fn assemble(program: &[Op]) -> Vec<libc::sock_filter> {
    let mut positions = Vec::new();
    let mut at = 0_usize;
    for op in program {
        match op {
            Op::Mark(label) => positions.push((*label, at)),
            Op::Statement(..) | Op::Jump { .. } => at += 1,
        }
    }
    let target = |label: Label, from: usize| -> u8 {
        let to = if label == Label::Next {
            from + 1
        } else {
            positions
                .iter()
                .find(|(marked, _)| *marked == label)
                .unwrap_or_else(|| panic!("seccomp label {label:?} is never marked"))
                .1
        };
        let skip = to
            .checked_sub(from + 1)
            .unwrap_or_else(|| panic!("seccomp label {label:?} lies behind its jump"));
        u8::try_from(skip).expect("a seccomp jump fits one classic-BPF offset")
    };
    let mut filters = Vec::with_capacity(at);
    for op in program {
        match op {
            Op::Mark(_) => {}
            Op::Statement(code, value) => filters.push(libc::sock_filter {
                code: *code,
                jt: 0,
                jf: 0,
                k: *value,
            }),
            Op::Jump { value, yes, no } => {
                let from = filters.len();
                filters.push(libc::sock_filter {
                    code: JMP_JEQ_K,
                    jt: target(*yes, from),
                    jf: target(*no, from),
                    k: *value,
                });
            }
        }
    }
    filters
}

/// One socket family's recorded stance, from the survey below.
#[cfg(test)]
pub(crate) struct FamilyPolicy {
    /// The family's name, for the message a failing case prints.
    pub name: &'static str,
    /// `AF_*`.
    pub family: libc::c_int,
    /// The socket type the survey observed opening inside the confinement argv.
    pub socket_type: libc::c_int,
    /// Whether this profile refuses the family.
    pub denied: bool,
    /// **Question one:** can data cross from this sandbox to another domain through it?
    pub reason: &'static str,
    /// **Question two:** does opening or binding it mutate state *outside* the sandbox?
    ///
    /// A separate field because it is a separate question, and because collapsing the two is the
    /// mistake this field exists to prevent: `AF_ALG` answered question one cleanly and was
    /// recorded allowed on that answer alone.
    pub host_state: &'static str,
}

/// **The measured stance of this profile on every socket family reachable from a confined
/// process**, on two questions, with the reason for each.
///
/// ### Why there are two questions
///
/// The first version of this table asked only *can a datagram cross to a sibling sandbox?* Every
/// autoloading family passed that question clean, `AF_ALG` among them, and it was recorded allowed
/// with the words "not a channel to another domain: nothing sent through it reaches any other
/// process". That was true about **data** and false about the family, and an adversary found it:
/// a `bind(2)` on `AF_ALG` names an algorithm and the kernel `request_module`s the backing
/// implementation into the host's single global module table, with the kernel's own privilege.
/// The confined process **influences** state outside its sandbox — attacker-chosen kernel code,
/// resident after the sandbox exits — and that state is **observable** across the boundary,
/// because `/proc/modules` is not namespaced. Reproduced here before this row changed: a confined
/// exec over this crate's own argv bound algorithms whose modules were absent and loaded
/// `aegis128`, `aegis128_aesni`, `chacha20poly1305`, `crypto_null`, `geniv`, `keywrap` and `seqiv`
/// onto the host, and a mutually-isolated sibling sandbox then listed all seven.
///
/// So each row answers both: `reason` for the data channel, `host_state` for the mutation.
///
/// ### How the list was produced
///
/// Every family `0..46` was tried with each of `SOCK_STREAM`, `SOCK_DGRAM`, `SOCK_RAW`,
/// `SOCK_RDM`, `SOCK_SEQPACKET`, `SOCK_DCCP` and `SOCK_PACKET` from inside the confinement argv on
/// this host. Twelve opened. Each was then put to two-isolated-sandboxes for question one, and to
/// a `/proc/modules` before-and-after for question two.
///
/// ### What question two turned up, and the part that is uncomfortable
///
/// `request_module` on `net-pf-<family>` — and on `net-pf-<family>-proto-<protocol>` — is a
/// property of `socket(2)` itself, not of a few exotic families. **`AF_INET` does it too**, and
/// that is measured, not feared: `socket(AF_INET, SOCK_STREAM, IPPROTO_SCTP)` from inside this
/// argv loaded `sctp` onto the host, where it stayed. `AF_INET` itself cannot be refused — the
/// egress aperture is a TCP listener in the child's own namespace (`egress::install_all`) and every
/// ordinary program resolves names — so ADR 0039 closes it on the protocol argument instead:
/// `ALLOWED_SOCKETS` admits each allowed family only with the protocols the runtime needs. Other
/// kernel autoload paths are not claimed closed by that ADR. The families that are denied below
/// are the ones where the same channel costs nothing to close, because the survey also measured
/// that a confined process can do nothing with them: an empty network namespace has no interface
/// for `AF_RDS` or `AF_SMC`, no session for `AF_PPPOX`, no MCTP interface, and no TCP socket for
/// `AF_KCM` to attach.
///
/// ### What this list still does not claim
///
/// It is complete for *this host's kernel with the modules it has loaded*. That is exactly why
/// `process.rs::no_socket_family_opens_inside_a_confined_exec_without_a_recorded_decision`
/// re-runs the enumeration inside a real admitted exec on every delegated run.
#[cfg(test)]
pub(crate) const FAMILY_POLICY: [FamilyPolicy; 13] = [
    FamilyPolicy {
        name: "AF_UNIX",
        family: libc::AF_UNIX,
        socket_type: libc::SOCK_STREAM,
        denied: true,
        reason: "the network namespace confines it — a name bound in one sandbox answered \
                 ECONNREFUSED from another — but the *filesystem* is the address space that \
                 matters here, and connect(2) can retarget a Unix socket at a host socket mounted \
                 into the sandbox",
        host_state: "none here: `unix` is built into this kernel, so nothing is loaded. On a \
                     kernel that builds it modular the `net-pf-1` alias would load it; denied for \
                     the filesystem reason either way",
    },
    FamilyPolicy {
        name: "AF_INET",
        family: libc::AF_INET,
        socket_type: libc::SOCK_DGRAM,
        denied: false,
        reason: "measured confined: a datagram sent to 127.0.0.1:34567 from one sandbox never \
                 reached a socket bound to it in another",
        host_state: "closed for the protocol argument by ADR 0039. Measured before it: \
                     socket(AF_INET, SOCK_STREAM, IPPROTO_SCTP) from inside this argv loaded \
                     `sctp` onto the host and it persisted after the sandbox exited. The family \
                     stays allowed for the egress aperture and name resolution, but only with \
                     protocol 0, IPPROTO_TCP or IPPROTO_UDP; any other protocol is refused EACCES \
                     before the kernel looks it up",
    },
    FamilyPolicy {
        name: "AF_INET6",
        family: libc::AF_INET6,
        socket_type: libc::SOCK_DGRAM,
        denied: false,
        reason: "measured confined, the same way as AF_INET, over [::1]:34567",
        host_state: "closed for the protocol argument by ADR 0039, as for AF_INET: the \
                     IPPROTO_SCTP probe opened over AF_INET6 as well before it, and now only \
                     protocol 0, IPPROTO_TCP and IPPROTO_UDP are admitted",
    },
    FamilyPolicy {
        name: "AF_NETLINK",
        family: libc::AF_NETLINK,
        socket_type: libc::SOCK_DGRAM,
        denied: false,
        reason: "measured confined: a unicast to a netlink pid bound in another sandbox was \
                 refused EPERM and delivered nothing; netlink address spaces are per network \
                 namespace and the child's own is empty",
        host_state: "closed for the protocol argument by ADR 0039: only NETLINK_ROUTE is \
                     admitted, which the kernel always has registered, so no other netlink \
                     protocol can be asked for. The family stays allowed because glibc reaches \
                     NETLINK_ROUTE for getaddrinfo and interface enumeration. The delegated case \
                     in tests/kernel_module_triggers.rs proves a non-ROUTE protocol is refused \
                     inside an admitted exec and leaves /proc/modules unchanged",
    },
    FamilyPolicy {
        name: "AF_RDS",
        family: libc::AF_RDS,
        socket_type: libc::SOCK_SEQPACKET,
        denied: true,
        reason: "no data channel: RDS rides addresses on network interfaces and the child's \
                 namespace has only its own loopback, so no address outside it could be \
                 constructed",
        host_state: "mutates it: `rds` is a module on this host and the net-pf-21 alias autoloads \
                     it on socket(2). It is resident here and the survey that opened the family \
                     is what put it there. Denied because closing that costs nothing — the \
                     measurement above is that a confined process can do nothing with it",
    },
    FamilyPolicy {
        name: "AF_PPPOX",
        family: libc::AF_PPPOX,
        socket_type: libc::SOCK_DGRAM,
        denied: true,
        reason: "no data channel: a PPPoX socket carries nothing until it is connected to a \
                 session on an interface, and the child's namespace has none",
        host_state: "mutates it: `pppox` is a module here and net-pf-24 autoloads it. Denied for \
                     AF_RDS's reason — an unusable family that loads host kernel code on request",
    },
    FamilyPolicy {
        name: "AF_ALG",
        family: libc::AF_ALG,
        socket_type: libc::SOCK_SEQPACKET,
        denied: true,
        reason: "no *data* channel — nothing sent through it reaches another process, which is \
                 what the refuted version of this row recorded and stopped at",
        host_state: "**the finding.** bind(2) names an algorithm and the kernel request_modules \
                     the implementation into the host's global module table with its own \
                     privilege, so --disable-userns does not stop it. Measured: a confined exec \
                     loaded aegis128, aegis128_aesni, chacha20poly1305, crypto_null, geniv, \
                     keywrap and seqiv, they persisted past sandbox exit, and an isolated sibling \
                     read all seven from the shared /proc/modules. Attacker-chosen kernel code \
                     plus a cross-sandbox observation channel",
    },
    FamilyPolicy {
        name: "AF_VSOCK",
        family: libc::AF_VSOCK,
        socket_type: libc::SOCK_STREAM,
        denied: true,
        reason: "vsock is not confined by a network namespace, so on a virtual-machine host a CID \
                 reaches the hypervisor side from inside --unshare-net (review finding 7)",
        host_state: "mutates it too: net-pf-40 resolves to the vsock transport modules and they \
                     are resident here. Denied for the hypervisor reason regardless",
    },
    FamilyPolicy {
        name: "AF_KCM",
        family: AF_KCM,
        socket_type: libc::SOCK_DGRAM,
        denied: true,
        reason: "no data channel: a kernel connection multiplexor carries nothing until a TCP \
                 socket is attached, and any such socket is already inside the child's namespace",
        host_state: "mutates it: `kcm` is a module here and net-pf-41 autoloads it. Denied for \
                     AF_RDS's reason",
    },
    FamilyPolicy {
        name: "AF_QIPCRTR",
        family: AF_QIPCRTR,
        socket_type: libc::SOCK_DGRAM,
        denied: true,
        reason: "measured **unconfined**: two mutually-isolated sandboxes, each with its own \
                 fresh network namespace, exchanged a datagram over it. The Qualcomm IPC router \
                 has no per-namespace address space, so a confined process reaches the host and \
                 every sibling sandbox",
        host_state: "mutates it as well: `qrtr` is a module here and net-pf-42 autoloads it",
    },
    FamilyPolicy {
        name: "AF_SMC",
        family: AF_SMC,
        socket_type: libc::SOCK_STREAM,
        denied: true,
        reason: "no data channel: SMC falls back to and rides TCP over the interfaces of the \
                 network namespace it is in, which is the child's own empty one",
        host_state: "mutates it: `smc` is a module here and net-pf-43 autoloads it. Denied for \
                     AF_RDS's reason",
    },
    FamilyPolicy {
        name: "AF_MCTP",
        family: AF_MCTP,
        socket_type: libc::SOCK_DGRAM,
        denied: true,
        reason: "no data channel: bind answered EACCES and sendto EINVAL inside the sandbox, and \
                 MCTP routing is per network namespace with no MCTP interface in the child's",
        host_state: "none *here* — measured: opening it loaded nothing, because this kernel \
                     builds mctp in. Denied anyway: on a kernel that builds it modular net-pf-45 \
                     autoloads it, and \"it happens to be built in on this host\" is exactly the \
                     host-dependent reasoning that produced two findings already",
    },
    FamilyPolicy {
        name: "AF_PACKET",
        family: libc::AF_PACKET,
        socket_type: libc::SOCK_DGRAM,
        denied: true,
        reason: "no data channel, and measured not reachable at all: every socket type answered \
                 EPERM inside the confinement argv, because packet sockets need CAP_NET_RAW",
        host_state: "none here — measured: the EPERM run loaded nothing, this kernel builds \
                     af_packet in. Denied because where it is modular net-pf-17 loads it *before* \
                     packet_create applies the CAP_NET_RAW check, so the refusal a confined \
                     process receives would arrive with the module already resident",
    },
];

fn failed(error: &std::io::Error) -> DriverError {
    DriverError::failed(
        "exec.sandbox-unavailable",
        format!("seccomp profile: {error}"),
    )
}

fn clear_cloexec(fd: RawFd) -> Result<(), DriverError> {
    // SAFETY: fcntl reads and updates flags on an owned descriptor.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) } != 0 {
        return Err(failed(&std::io::Error::last_os_error()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{ALLOWED_SOCKETS, ERRNO, FAMILY_POLICY, filters};

    #[cfg(target_arch = "x86_64")]
    const X32_SYSCALL_BIT: libc::c_long = 0x4000_0000;

    #[test]
    fn unix_stream_socketpair_remains_available_for_process_local_ipc() {
        assert_profile_result(unix_stream_socketpair_succeeds);
    }

    #[test]
    fn unix_datagram_socketpair_cannot_be_repurposed_for_host_ipc() {
        assert_profile_result(unix_datagram_socketpair_is_denied);
    }

    #[cfg(target_arch = "x86_64")]
    #[test]
    fn x32_socket_syscall_cannot_bypass_the_unix_socket_refusal() {
        assert_profile_result(x32_unix_socket_is_denied);
    }

    /// The same claim for the x32 ABI, over **every** family the table denies rather than the one
    /// that happened to have a case.
    ///
    /// Driven off `FAMILY_POLICY` because that is what makes it a check on the class instead of
    /// on an instance: a family added to the table cannot arrive with a native refusal and an x32
    /// hole, and nobody has to remember to write a second case beside the first.
    /// `x32_socket_syscall_cannot_bypass_the_unix_socket_refusal` above states the same thing for
    /// `AF_UNIX` in the plainest form and is kept for the message it fails with.
    #[cfg(target_arch = "x86_64")]
    #[test]
    fn no_denied_family_can_be_reached_through_the_x32_socket_syscall() {
        let mut checked = 0;
        for policy in FAMILY_POLICY {
            if !policy.denied {
                continue;
            }
            checked += 1;
            assert!(
                profile_child_passes(&|| {
                    // SAFETY: the child issues one raw syscall and closes nothing.
                    unsafe { x32_family_is_denied(policy.family, policy.socket_type) }
                }),
                "{} is refused natively and still reachable through the x32 socket syscall; it \
                 is recorded denied because {}",
                policy.name,
                policy.reason
            );
        }
        assert_eq!(
            checked,
            FAMILY_POLICY.len() - ALLOWED_SOCKETS.len(),
            "this case covers the denied families by walking the survey, so a survey that denies \
             a different number covers a different set than the one it reports"
        );
    }

    /// Whether the ABI-tagged `socket` number is refused for `family`.
    ///
    /// The raw syscall and not `libc::socket`, for `x32_unix_socket_is_denied`'s reason: seccomp
    /// answers before a kernel without x32 dispatch needs to understand the number.
    #[cfg(target_arch = "x86_64")]
    unsafe fn x32_family_is_denied(family: libc::c_int, socket_type: libc::c_int) -> bool {
        // SAFETY: the raw syscall receives the documented socket arguments.
        let result = unsafe {
            libc::syscall(
                libc::SYS_socket | X32_SYSCALL_BIT,
                family,
                socket_type | libc::SOCK_CLOEXEC,
                0,
            )
        };
        result == -1 && std::io::Error::last_os_error().raw_os_error() == Some(libc::EACCES)
    }

    /// The acceptance of `story:seccomp-denies-af-vsock` at the profile, and the whole survey
    /// with it.
    ///
    /// One forked child per family so a failure names the family and quotes the reason it was
    /// recorded under, rather than reporting that some socket call somewhere answered wrongly.
    /// Each row is driven with the socket type the survey saw open inside the confinement argv:
    /// `AF_ALG` opens as `SOCK_SEQPACKET` and nothing else, and a table driven with one type for
    /// every family would have exercised a shape the kernel refuses for its own reasons.
    #[test]
    fn every_surveyed_socket_family_answers_the_way_the_profile_recorded_it() {
        for policy in FAMILY_POLICY {
            assert!(
                profile_child_passes(&|| {
                    // SAFETY: the child calls socket and close on its own return value only.
                    unsafe {
                        family_answer_matches(policy.family, policy.socket_type, policy.denied)
                    }
                }),
                "{} did not answer the way this profile records it: it is {} because {}",
                policy.name,
                if policy.denied {
                    "recorded denied"
                } else {
                    "recorded allowed"
                },
                policy.reason
            );
        }
    }

    /// The survey may not shrink, its two halves may not disagree, and **no row may answer only
    /// one of the two questions**.
    ///
    /// The last clause is the one that matters. `AF_ALG` was recorded allowed with a `reason`
    /// that answered "does data cross?" and nothing that answered "does this mutate state outside
    /// the sandbox?", and the row read as a complete decision because there was nowhere for the
    /// second answer to be missing from. Now there is.
    #[test]
    fn every_surveyed_row_answers_both_questions_and_the_filter_is_built_from_the_same_list() {
        assert_eq!(
            FAMILY_POLICY.len(),
            13,
            "the survey measured 13 families; a shorter table is a narrower claim than the one \
             this profile makes"
        );
        for policy in FAMILY_POLICY {
            assert!(
                !policy.reason.is_empty(),
                "{} records no answer to the data-channel question",
                policy.name
            );
            assert!(
                !policy.host_state.is_empty(),
                "{} records no answer to the host-state question, which is the exact shape that \
                 let AF_ALG be recorded allowed",
                policy.name
            );
        }

        // The filter is generated from `ALLOWED_SOCKETS` (ADR 0039) and refuses every family it
        // does not list; the survey is what a reader consults. Two lists that disagree would mean
        // one of them is a document about something else.
        let surveyed: Vec<(&str, libc::c_int)> = FAMILY_POLICY
            .iter()
            .filter(|policy| policy.denied)
            .map(|policy| (policy.name, policy.family))
            .collect();
        let mut surveyed_allowed: Vec<(&str, libc::c_int)> = FAMILY_POLICY
            .iter()
            .filter(|policy| !policy.denied)
            .map(|policy| (policy.name, policy.family))
            .collect();
        let mut generated: Vec<(&str, libc::c_int)> = ALLOWED_SOCKETS
            .iter()
            .map(|(name, family, _)| (*name, *family))
            .collect();
        surveyed_allowed.sort_by_key(|(_, family)| *family);
        generated.sort_by_key(|(_, family)| *family);
        assert_eq!(
            surveyed_allowed, generated,
            "the families the BPF chain admits and the families the survey records as allowed are \
             different sets; the filter is what runs and the survey is what is read, so a reader \
             would be told something the sandbox does not do"
        );
        assert_eq!(
            surveyed.iter().map(|(name, _)| *name).collect::<Vec<_>>(),
            vec![
                "AF_UNIX",
                "AF_RDS",
                "AF_PPPOX",
                "AF_ALG",
                "AF_VSOCK",
                "AF_KCM",
                "AF_QIPCRTR",
                "AF_SMC",
                "AF_MCTP",
                "AF_PACKET",
            ],
            "the refused set is the confinement floor; changing it is a deliberate act and this \
             is where it is declared"
        );
    }

    /// ADR 0039 at the profile: every allowlisted (family, type, protocol) triple is not refused
    /// by this profile.
    ///
    /// Asserted as "not `EACCES`", never as success, for `family_answer_matches`'s reason: a
    /// kernel without the family answers for itself, and that is not this profile speaking.
    #[test]
    fn every_allowlisted_socket_triple_is_not_refused() {
        let triples = [
            (libc::AF_INET, libc::SOCK_STREAM, 0),
            (libc::AF_INET, libc::SOCK_STREAM, libc::IPPROTO_TCP),
            (libc::AF_INET, libc::SOCK_DGRAM, 0),
            (libc::AF_INET, libc::SOCK_DGRAM, libc::IPPROTO_UDP),
            (libc::AF_INET6, libc::SOCK_STREAM, 0),
            (libc::AF_INET6, libc::SOCK_STREAM, libc::IPPROTO_TCP),
            (libc::AF_INET6, libc::SOCK_DGRAM, 0),
            (libc::AF_INET6, libc::SOCK_DGRAM, libc::IPPROTO_UDP),
            (libc::AF_NETLINK, libc::SOCK_RAW, libc::NETLINK_ROUTE),
            (libc::AF_NETLINK, libc::SOCK_DGRAM, libc::NETLINK_ROUTE),
        ];
        for (family, socket_type, protocol) in triples {
            assert!(
                profile_child_passes(&|| {
                    // SAFETY: the child calls socket and close on its own return value only.
                    unsafe { socket_errno(family, socket_type, protocol) != Some(libc::EACCES) }
                }),
                "socket({family}, {socket_type}, {protocol}) is on the ADR 0039 allowlist and \
                 this profile refused it"
            );
        }
    }

    /// ADR 0039 at the profile: a protocol outside the allowlist, on an allowed family, is
    /// refused with `EACCES`, natively and through the x32 number.
    #[test]
    fn a_protocol_outside_the_allowlist_is_refused_on_every_allowed_family() {
        const IPPROTO_SCTP: libc::c_int = 132;
        const NETLINK_NETFILTER: libc::c_int = 12;
        let triples = [
            (libc::AF_INET, libc::SOCK_STREAM, IPPROTO_SCTP),
            (libc::AF_INET6, libc::SOCK_STREAM, IPPROTO_SCTP),
            (libc::AF_INET, libc::SOCK_DGRAM, libc::IPPROTO_ICMP),
            (libc::AF_INET6, libc::SOCK_DGRAM, libc::IPPROTO_ICMPV6),
            (libc::AF_NETLINK, libc::SOCK_RAW, NETLINK_NETFILTER),
        ];
        for (family, socket_type, protocol) in triples {
            assert!(
                profile_child_passes(&|| {
                    // SAFETY: the child calls socket and close on its own return value only.
                    unsafe { socket_errno(family, socket_type, protocol) == Some(libc::EACCES) }
                }),
                "socket({family}, {socket_type}, {protocol}) is outside the ADR 0039 allowlist \
                 and this profile did not refuse it with EACCES"
            );
            #[cfg(target_arch = "x86_64")]
            assert!(
                profile_child_passes(&|| {
                    // SAFETY: the child issues one raw syscall and closes nothing it did not get.
                    let result = unsafe {
                        libc::syscall(
                            libc::SYS_socket | X32_SYSCALL_BIT,
                            family,
                            socket_type | libc::SOCK_CLOEXEC,
                            protocol,
                        )
                    };
                    result == -1
                        && std::io::Error::last_os_error().raw_os_error() == Some(libc::EACCES)
                }),
                "socket({family}, {socket_type}, {protocol}) is refused natively and reachable \
                 through the x32 socket syscall"
            );
        }
    }

    /// ADR 0039 at the profile: `socketpair(2)` stays `AF_UNIX` only.
    #[test]
    fn socketpair_outside_af_unix_is_refused() {
        for family in [libc::AF_INET, libc::AF_INET6, libc::AF_NETLINK] {
            assert!(
                profile_child_passes(&|| {
                    let mut sockets = [-1; 2];
                    // SAFETY: `sockets` has room for exactly two returned descriptors.
                    let result = unsafe {
                        libc::socketpair(
                            family,
                            libc::SOCK_STREAM | libc::SOCK_CLOEXEC,
                            0,
                            sockets.as_mut_ptr(),
                        )
                    };
                    result == -1
                        && std::io::Error::last_os_error().raw_os_error() == Some(libc::EACCES)
                }),
                "socketpair({family}) was not refused with EACCES"
            );
        }
    }

    /// ADR 0039 at the profile: `ioctl(TIOCSETD)` is refused with `EACCES` before the kernel
    /// looks at the descriptor, and another terminal ioctl is not.
    ///
    /// Issued on descriptor -1 so that no terminal is touched: the profile answers before the
    /// kernel would answer `EBADF`.
    #[test]
    fn the_line_discipline_ioctl_is_refused_and_other_terminal_ioctls_are_not() {
        assert!(
            profile_child_passes(&|| {
                let discipline: libc::c_int = 0;
                // SAFETY: TIOCSETD reads one int through a pointer that outlives the call.
                let result = unsafe { libc::ioctl(-1, libc::TIOCSETD, &raw const discipline) };
                result == -1 && std::io::Error::last_os_error().raw_os_error() == Some(libc::EACCES)
            }),
            "ioctl(TIOCSETD) was not refused with EACCES"
        );
        assert!(
            profile_child_passes(&|| {
                let mut size = libc::winsize {
                    ws_row: 0,
                    ws_col: 0,
                    ws_xpixel: 0,
                    ws_ypixel: 0,
                };
                // SAFETY: TIOCGWINSZ writes one winsize through a pointer that outlives the call.
                let result = unsafe { libc::ioctl(-1, libc::TIOCGWINSZ, &raw mut size) };
                result == -1 && std::io::Error::last_os_error().raw_os_error() == Some(libc::EBADF)
            }),
            "ioctl(TIOCGWINSZ) must reach the kernel and answer EBADF on descriptor -1"
        );
    }

    /// The errno `socket(family, socket_type, protocol)` answered, or `None` if it opened.
    unsafe fn socket_errno(
        family: libc::c_int,
        socket_type: libc::c_int,
        protocol: libc::c_int,
    ) -> Option<i32> {
        // SAFETY: socket takes three integers and returns an owned descriptor or -1.
        let result = unsafe { libc::socket(family, socket_type | libc::SOCK_CLOEXEC, protocol) };
        if result >= 0 {
            // SAFETY: the descriptor was returned by the successful call above.
            unsafe { libc::close(result) };
            return None;
        }
        std::io::Error::last_os_error().raw_os_error()
    }

    fn assert_profile_result(probe: unsafe fn() -> bool) {
        assert!(
            profile_child_passes(&|| {
                // SAFETY: the caller's probe is one of this module's own post-fork-safe probes.
                unsafe { probe() }
            }),
            "the probe reported a failure under the installed profile"
        );
    }

    /// Installs the profile in a forked child, runs `probe` under it, and reports whether the
    /// child both installed the filter and answered true.
    ///
    /// The closure is called after the fork, so it must allocate nothing and take no lock; every
    /// probe below is a bare libc call over inherited stack storage.
    fn profile_child_passes(probe: &dyn Fn() -> bool) -> bool {
        let deny = ERRNO | u32::try_from(libc::EACCES).unwrap_or(13);
        let mut instructions = filters(deny);
        let program = libc::sock_fprog {
            len: u16::try_from(instructions.len()).expect("seccomp program fits u16"),
            filter: instructions.as_mut_ptr(),
        };
        // SAFETY: the child performs only libc calls against inherited stack storage, then exits.
        let child = unsafe { libc::fork() };
        assert!(child >= 0, "fork seccomp test child");
        if child == 0 {
            // SAFETY: no-new-privileges precedes installation of the valid classic-BPF program.
            let installed = unsafe {
                libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) == 0
                    && libc::prctl(
                        libc::PR_SET_SECCOMP,
                        libc::SECCOMP_MODE_FILTER,
                        &raw const program,
                    ) == 0
            };
            let passed = installed && probe();
            // SAFETY: this is the post-fork child and no Rust destructor may run here.
            unsafe { libc::_exit(i32::from(!passed)) };
        }
        let mut status = 0;
        // SAFETY: `child` is the live pid returned by fork and `status` is writable.
        assert_eq!(unsafe { libc::waitpid(child, &raw mut status, 0) }, child);
        assert!(libc::WIFEXITED(status), "the probe child did not exit");
        libc::WEXITSTATUS(status) == 0
    }

    /// Whether `family` answers `EACCES` exactly when the table says this profile denies it.
    ///
    /// An allowed family is asserted **not to be refused by this profile**, never asserted to
    /// succeed: `AF_PACKET` needs `CAP_NET_RAW` and a family whose module is absent answers
    /// `EAFNOSUPPORT`, and neither of those is this profile speaking.
    unsafe fn family_answer_matches(
        family: libc::c_int,
        socket_type: libc::c_int,
        denied: bool,
    ) -> bool {
        // SAFETY: socket takes three integers and returns an owned descriptor or -1.
        let result = unsafe { libc::socket(family, socket_type | libc::SOCK_CLOEXEC, 0) };
        if result >= 0 {
            // SAFETY: the descriptor was returned by the successful call above.
            unsafe { libc::close(result) };
            return !denied;
        }
        (std::io::Error::last_os_error().raw_os_error() == Some(libc::EACCES)) == denied
    }

    unsafe fn unix_stream_socketpair_succeeds() -> bool {
        let mut sockets = [-1; 2];
        // SAFETY: `sockets` has room for exactly two returned descriptors.
        let result = unsafe {
            libc::socketpair(
                libc::AF_UNIX,
                libc::SOCK_STREAM | libc::SOCK_CLOEXEC,
                0,
                sockets.as_mut_ptr(),
            )
        };
        if result != 0 {
            return false;
        }
        // SAFETY: both descriptors were returned by the successful socketpair call.
        unsafe {
            libc::close(sockets[0]);
            libc::close(sockets[1]);
        }
        true
    }

    unsafe fn unix_datagram_socketpair_is_denied() -> bool {
        let mut sockets = [-1; 2];
        // SAFETY: `sockets` has room for exactly two returned descriptors.
        let result = unsafe {
            libc::socketpair(
                libc::AF_UNIX,
                libc::SOCK_DGRAM | libc::SOCK_CLOEXEC,
                0,
                sockets.as_mut_ptr(),
            )
        };
        result == -1 && std::io::Error::last_os_error().raw_os_error() == Some(libc::EACCES)
    }

    #[cfg(target_arch = "x86_64")]
    unsafe fn x32_unix_socket_is_denied() -> bool {
        // SAFETY: the raw syscall receives the documented socket arguments. Seccomp answers before
        // kernels without x32 dispatch need to understand the ABI-tagged syscall number.
        let result = unsafe {
            libc::syscall(
                libc::SYS_socket | X32_SYSCALL_BIT,
                libc::AF_UNIX,
                libc::SOCK_STREAM | libc::SOCK_CLOEXEC,
                0,
            )
        };
        result == -1 && std::io::Error::last_os_error().raw_os_error() == Some(libc::EACCES)
    }
}
