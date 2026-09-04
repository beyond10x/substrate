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
        jump(u32::try_from(libc::SYS_socket).unwrap_or(u32::MAX), 0, 4),
        statement(LD_W_ABS, 16),
        jump(u32::try_from(libc::AF_UNIX).unwrap_or(1), 1, 0),
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
        jump(u32::try_from(libc::AF_VSOCK).unwrap_or(40), 0, 1),
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
    use super::{ERRNO, filters};

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
        for (name, family, denied, reason) in FAMILY_POLICY {
            if !denied {
                continue;
            }
            assert!(
                profile_child_passes(&|| {
                    // SAFETY: the child issues one raw syscall and closes nothing.
                    unsafe { x32_family_is_denied(family) }
                }),
                "{name} is refused natively and still reachable through the x32 socket syscall; \
                 it is recorded denied because {reason}"
            );
        }
    }

    /// Whether the ABI-tagged `socket` number is refused for `family`.
    ///
    /// The raw syscall and not `libc::socket`, for `x32_unix_socket_is_denied`'s reason: seccomp
    /// answers before a kernel without x32 dispatch needs to understand the number.
    #[cfg(target_arch = "x86_64")]
    unsafe fn x32_family_is_denied(family: libc::c_int) -> bool {
        // SAFETY: the raw syscall receives the documented socket arguments.
        let result = unsafe {
            libc::syscall(
                libc::SYS_socket | X32_SYSCALL_BIT,
                family,
                libc::SOCK_STREAM | libc::SOCK_CLOEXEC,
                0,
            )
        };
        result == -1 && std::io::Error::last_os_error().raw_os_error() == Some(libc::EACCES)
    }

    /// Every socket family this profile has decided about, the decision, and the reason for it.
    ///
    /// A table rather than one case per family, because the defect review finding 7 named is a
    /// family nobody had decided about at all: `AF_VSOCK` was neither refused nor recorded as
    /// deliberately allowed, and the profile's silence read to every later reader as a decision.
    /// Here a family cannot be present without a reason, and one whose behaviour drifts from its
    /// recorded decision is a red case rather than a comment nobody re-checked.
    ///
    /// **Bounded, and it does not claim to be the whole family list.** These four are the ones
    /// review finding 7 and `story:seccomp-denies-af-vsock` name. Linux has some forty address
    /// families and this profile takes no position on the rest; saying so is better than an
    /// enumeration that looks complete and is not.
    const FAMILY_POLICY: [(&str, libc::c_int, bool, &str); 4] = [
        (
            "AF_UNIX",
            libc::AF_UNIX,
            true,
            "connect(2) can retarget a Unix socket at a host socket mounted into the sandbox, and \
             no namespace confines the filesystem the child was given",
        ),
        (
            "AF_VSOCK",
            libc::AF_VSOCK,
            true,
            "vsock is not confined by a network namespace, so on a virtual-machine host a CID \
             reaches the hypervisor side from inside --unshare-net (review finding 7)",
        ),
        (
            "AF_NETLINK",
            libc::AF_NETLINK,
            false,
            "netlink sockets belong to a network namespace; --unshare-net already gives the child \
             an empty one of its own, so this profile is not what confines them",
        ),
        (
            "AF_PACKET",
            libc::AF_PACKET,
            false,
            "packet sockets belong to a network namespace too, and the child's carries no \
             interface to capture from, so this profile is not what confines them either",
        ),
    ];

    /// The acceptance of `story:seccomp-denies-af-vsock` at the profile, and the record for the
    /// two families its Notes asked about.
    ///
    /// One forked child per family so a failure names the family and quotes the reason it was
    /// recorded under, rather than reporting that some socket call somewhere answered wrongly.
    #[test]
    fn every_decided_socket_family_answers_the_way_the_profile_decided() {
        for (name, family, denied, reason) in FAMILY_POLICY {
            assert!(
                profile_child_passes(&|| {
                    // SAFETY: the child calls socket and close on its own return value only.
                    unsafe { family_answer_matches(family, denied) }
                }),
                "{name} did not answer the way this profile records it: it is {} because {reason}",
                if denied {
                    "recorded denied"
                } else {
                    "recorded allowed"
                }
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
    unsafe fn family_answer_matches(family: libc::c_int, denied: bool) -> bool {
        // SAFETY: socket takes three integers and returns an owned descriptor or -1.
        let result = unsafe { libc::socket(family, libc::SOCK_STREAM | libc::SOCK_CLOEXEC, 0) };
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
