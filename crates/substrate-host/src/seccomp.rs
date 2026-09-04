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
const AF_QIPCRTR: libc::c_int = 42;

#[cfg(target_arch = "x86_64")]
const X32_SYSCALL_BIT: u32 = 0x4000_0000;

#[cfg(target_arch = "x86_64")]
const AUDIT_ARCH: u32 = 0xc000_003e;
#[cfg(target_arch = "aarch64")]
const AUDIT_ARCH: u32 = 0xc000_00b7;
#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
compile_error!("the confinement seccomp profile has no audited architecture for this target");

fn statement(code: u16, value: u32) -> libc::sock_filter {
    libc::sock_filter {
        code,
        jt: 0,
        jf: 0,
        k: value,
    }
}

fn jump(value: u32, yes: u8, no: u8) -> libc::sock_filter {
    libc::sock_filter {
        code: JMP_JEQ_K,
        jt: yes,
        jf: no,
        k: value,
    }
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

fn filters(deny: u32) -> Vec<libc::sock_filter> {
    let mut filters = vec![
        statement(LD_W_ABS, 4),
        jump(AUDIT_ARCH, 1, 0),
        statement(RET_K, deny),
        statement(LD_W_ABS, 0),
    ];
    normalize_syscall_number(&mut filters);
    filters.extend([
        jump(
            u32::try_from(libc::SYS_io_uring_setup).unwrap_or(u32::MAX),
            0,
            1,
        ),
        statement(RET_K, deny),
        jump(u32::try_from(libc::SYS_socket).unwrap_or(u32::MAX), 0, 5),
        statement(LD_W_ABS, 16),
        jump(u32::try_from(libc::AF_UNIX).unwrap_or(1), 2, 0),
        // `--unshare-net` gives the child an empty network namespace, and that is what confines
        // every family which lives in one. **Vsock does not live in one**: mainline has no vsock
        // namespace, so a confined process on a virtual-machine host — which the observed
        // development nodes are (`STATUS.md` § Current state) — can open a CID straight to the
        // hypervisor side while the empty netns holds. Measured before this jump existed: an
        // admitted exec created the socket and reached `connect(2)`.
        //
        // `AF_NETLINK` and `AF_PACKET` are deliberately *not* here, and `seccomp::tests`'
        // `FAMILY_POLICY` carries the reason for each beside this one: both belong to a network
        // namespace, so the child's own empty one already confines them and a second refusal here
        // would state a confinement this profile is not the source of.
        jump(u32::try_from(libc::AF_VSOCK).unwrap_or(40), 1, 0),
        // The second family the survey found, and the one that was found by measuring rather
        // than by reading: two mutually-isolated sandboxes — `net:[4026534317]` and
        // `net:[4026534407]`, each with its own fresh network and user namespace over this very
        // argv — exchanged a datagram over `AF_QIPCRTR`. The same script over abstract `AF_UNIX`,
        // `AF_INET` and `AF_INET6` across the same boundary delivered nothing. The Qualcomm IPC
        // router has no per-network-namespace address space, so a confined process reaches every
        // other domain on the node, including the host.
        jump(u32::try_from(AF_QIPCRTR).unwrap_or(42), 0, 1),
        statement(RET_K, deny),
        statement(LD_W_ABS, 0),
    ]);
    normalize_syscall_number(&mut filters);
    // A Unix datagram socketpair is not confined to its peer: `connect(2)` can retarget either
    // endpoint to a mounted host socket. Connection-oriented socketpairs remain available for
    // process-local IPC, while the repurposable form receives the same EACCES as `socket(AF_UNIX)`.
    filters.extend([
        jump(
            u32::try_from(libc::SYS_socketpair).unwrap_or(u32::MAX),
            0,
            6,
        ),
        statement(LD_W_ABS, 16),
        jump(u32::try_from(libc::AF_UNIX).unwrap_or(1), 0, 4),
        statement(LD_W_ABS, 24),
        statement(ALU_AND_K, SOCKET_TYPE_MASK),
        jump(u32::try_from(libc::SOCK_DGRAM).unwrap_or(2), 0, 1),
        statement(RET_K, deny),
        statement(RET_K, ALLOW),
    ]);
    filters
}

fn normalize_syscall_number(filters: &mut Vec<libc::sock_filter>) {
    // x32 uses the x86_64 audit architecture with an ABI bit in the syscall number. Compare the
    // underlying call so an x32 `socket` or `socketpair` cannot skip the policy's native numbers.
    #[cfg(target_arch = "x86_64")]
    filters.push(statement(ALU_AND_K, !X32_SYSCALL_BIT));
    #[cfg(not(target_arch = "x86_64"))]
    let _ = filters;
}

/// One socket family's recorded stance, from the survey below.
#[cfg(test)]
pub(crate) struct FamilyPolicy {
    /// The family's name, for the message a failing case prints.
    pub name: &'static str,
    /// `AF_*`.
    pub family: libc::c_int,
    /// The socket type the survey observed opening inside the confinement argv, so a case that
    /// drives this row exercises the shape the kernel actually offers rather than a guess.
    pub socket_type: libc::c_int,
    /// Whether this profile refuses the family.
    pub denied: bool,
    /// Why — measured, not considered.
    pub reason: &'static str,
}

/// **The measured stance of this profile on every socket family reachable from a confined
/// process**, and the reason for each.
///
/// The four-row table this replaces was written from reading, and the gap was exactly that: it
/// covered `AF_UNIX`, `AF_VSOCK`, `AF_NETLINK` and `AF_PACKET`, said it took no position on the
/// other ~40, and named `AF_ALG` as the one it would look at next. An adversary then found
/// `AF_QIPCRTR` — reachable, unconfined, and not in the table. A survey is not a shortlist.
///
/// **How this list was produced.** Every family `0..46` was tried with each of `SOCK_DGRAM`,
/// `SOCK_STREAM`, `SOCK_SEQPACKET` and `SOCK_RAW` from inside the confinement argv — the same
/// `--unshare-user --disable-userns --unshare-net …` bubblewrap the driver builds — on this host.
/// **Twelve of the forty-six opened**; they are the twelve rows below carrying a real
/// `socket_type`. Each was then put to the question that decides this class: two mutually-isolated
/// sandboxes, each with its own fresh network namespace, and a message sent from one to the other
/// over that family's own addressing.
///
/// | family | opened in the sandbox | crossed two isolated sandboxes |
/// |---|---|---|
/// | `AF_UNIX` | yes | no — `ECONNREFUSED` on an abstract name |
/// | `AF_INET`, `AF_INET6` | yes | no — sent, never delivered |
/// | `AF_NETLINK` | yes | no — `EPERM`, and the address space is per-netns |
/// | `AF_QIPCRTR` | yes | **yes — `hello-from-the-other-sandbox` arrived** |
/// | `AF_VSOCK` | yes | reaches the hypervisor, not a sibling (see its row) |
/// | `AF_ALG` | yes, `SOCK_SEQPACKET` only | no peer to reach; see its row |
/// | `AF_RDS`, `AF_PPPOX`, `AF_KCM`, `AF_SMC`, `AF_MCTP` | yes | no address outside the namespace could be constructed |
///
/// `AF_PACKET` is the thirteenth row: it is here because the vsock story's Notes asked about it,
/// and the survey answered more sharply than the reasoning it replaces — it does not open inside
/// the confinement argv at all.
///
/// **What this list still does not claim.** It is complete for *this host's kernel with the
/// modules it has loaded*. A family whose module is absent here answers `EAFNOSUPPORT` and would
/// open elsewhere, which is why `process.rs`'s
/// `no_socket_family_opens_inside_a_confined_exec_without_a_recorded_decision` re-runs the
/// enumeration inside a real admitted exec on every delegated run: a family that becomes
/// reachable and has no row here is a red case, not a wait for the next adversary.
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
    },
    FamilyPolicy {
        name: "AF_INET",
        family: libc::AF_INET,
        socket_type: libc::SOCK_DGRAM,
        denied: false,
        reason: "measured confined: a datagram sent to 127.0.0.1:34567 from one sandbox never \
                 reached a socket bound to it in another",
    },
    FamilyPolicy {
        name: "AF_INET6",
        family: libc::AF_INET6,
        socket_type: libc::SOCK_DGRAM,
        denied: false,
        reason: "measured confined, the same way as AF_INET, over [::1]:34567",
    },
    FamilyPolicy {
        name: "AF_NETLINK",
        family: libc::AF_NETLINK,
        socket_type: libc::SOCK_DGRAM,
        denied: false,
        reason: "measured confined: a unicast to a netlink pid bound in another sandbox was \
                 refused EPERM and delivered nothing; netlink address spaces are per network \
                 namespace and the child's own is empty",
    },
    FamilyPolicy {
        name: "AF_RDS",
        family: libc::AF_RDS,
        socket_type: libc::SOCK_SEQPACKET,
        denied: false,
        reason: "opens, but RDS rides addresses on network interfaces and the child's namespace \
                 has only its own loopback; no address outside it could be constructed",
    },
    FamilyPolicy {
        name: "AF_PPPOX",
        family: libc::AF_PPPOX,
        socket_type: libc::SOCK_DGRAM,
        denied: false,
        reason: "opens, but a PPPoX socket carries nothing until it is connected to a session on \
                 an interface, and the child's namespace has none",
    },
    FamilyPolicy {
        name: "AF_ALG",
        family: libc::AF_ALG,
        socket_type: libc::SOCK_SEQPACKET,
        denied: false,
        reason: "reachable and namespace-free — bind(hash/sha256) then accept() computed a real \
                 digest inside the sandbox — but it is not a channel to another domain: nothing \
                 sent through it reaches any other process, and two sandboxes each got their own \
                 independent transform. It is kernel attack surface rather than an escape, which \
                 is a hardening decision and not this class; recorded so it is a decision and not \
                 a silence. Note it opens as SOCK_SEQPACKET only, which is why a survey that \
                 tried SOCK_STREAM alone would have called it absent",
    },
    FamilyPolicy {
        name: "AF_VSOCK",
        family: libc::AF_VSOCK,
        socket_type: libc::SOCK_STREAM,
        denied: true,
        reason: "vsock is not confined by a network namespace, so on a virtual-machine host a CID \
                 reaches the hypervisor side from inside --unshare-net (review finding 7)",
    },
    FamilyPolicy {
        name: "AF_KCM",
        family: AF_KCM,
        socket_type: libc::SOCK_DGRAM,
        denied: false,
        reason: "opens, but a kernel connection multiplexor carries nothing until a TCP socket is \
                 attached to it, and any such socket is already inside the child's own namespace",
    },
    FamilyPolicy {
        name: "AF_QIPCRTR",
        family: AF_QIPCRTR,
        socket_type: libc::SOCK_DGRAM,
        denied: true,
        reason: "measured **unconfined**: two mutually-isolated sandboxes, each with its own \
                 fresh network namespace, exchanged a datagram over it (adversary wave B u3 \
                 pass 1, reproduced here). The Qualcomm IPC router has no per-namespace address \
                 space, so a confined process reaches the host and every sibling sandbox",
    },
    FamilyPolicy {
        name: "AF_SMC",
        family: AF_SMC,
        socket_type: libc::SOCK_STREAM,
        denied: false,
        reason: "opens, but SMC falls back to and rides TCP over the interfaces of the network \
                 namespace it is in, which is the child's own empty one",
    },
    FamilyPolicy {
        name: "AF_MCTP",
        family: AF_MCTP,
        socket_type: libc::SOCK_DGRAM,
        denied: false,
        reason: "opens, but bind answered EACCES and sendto EINVAL inside the sandbox: MCTP \
                 routing is per network namespace and the child's carries no MCTP interface",
    },
    FamilyPolicy {
        name: "AF_PACKET",
        family: libc::AF_PACKET,
        socket_type: libc::SOCK_DGRAM,
        denied: false,
        reason: "measured **not reachable at all**: no socket type opened it inside the \
                 confinement argv, and on the host itself it answers EPERM without CAP_NET_RAW. \
                 Packet sockets are per network namespace besides, and the child's carries no \
                 interface to capture from",
    },
];

/// The three further families this crate's `libc` does not name, for `AF_QIPCRTR`'s reason.
///
/// The survey measured all three opening inside the confinement argv, so they are spelled out
/// here rather than waited for.
#[cfg(test)]
const AF_KCM: libc::c_int = 41;
#[cfg(test)]
const AF_SMC: libc::c_int = 43;
#[cfg(test)]
const AF_MCTP: libc::c_int = 45;

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
    use super::{ERRNO, FAMILY_POLICY, filters};

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
            checked, 3,
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

    /// The survey is not allowed to shrink, and its two halves are not allowed to disagree.
    ///
    /// The defect this whole round answers was a table that covered four families and named the
    /// rest as out of scope. A row count and a denied set asserted here mean removing a family —
    /// or quietly flipping one to allowed — is a red case rather than a smaller document.
    #[test]
    fn the_survey_keeps_every_family_it_measured_and_refuses_the_three_it_must() {
        assert_eq!(
            FAMILY_POLICY.len(),
            13,
            "the survey measured 13 families; a shorter table is a narrower claim than the one \
             this profile makes"
        );
        let denied: Vec<&str> = FAMILY_POLICY
            .iter()
            .filter(|policy| policy.denied)
            .map(|policy| policy.name)
            .collect();
        assert_eq!(
            denied,
            vec!["AF_UNIX", "AF_VSOCK", "AF_QIPCRTR"],
            "the families this profile refuses are the ones the survey found reaching out of the \
             sandbox; a change to this set is a change to the confinement floor"
        );
        for policy in FAMILY_POLICY {
            assert!(
                !policy.reason.is_empty(),
                "{} carries no recorded reason, which is the shape the adversary found",
                policy.name
            );
        }
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
