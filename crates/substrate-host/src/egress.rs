//! Destination-bound egress apertures (ADR 0013), by mechanism (c).
//!
//! The sandbox keeps `--unshare-net`. Its network namespace has loopback and no other interface, no
//! route and no resolver, exactly as it does for a run with no aperture at all. What an aperture
//! adds is **one listening socket inside that namespace**, created by substrate and connected by
//! substrate to one address pinned at declaration. The kernel floor is untouched: everything else
//! is still `ENETUNREACH`, from a raw socket as much as from a TCP one, because there is nothing to
//! route to (`docs/design/10a-egress-mechanism-spike.md` § 3.1).
//!
//! Three processes, and which one is where is the whole mechanism:
//!
//! | | namespace | lifetime |
//! |---|---|---|
//! | **helper** | joins the sandbox's netns | one `setns`, one `bind`, one `listen`, one handback, exit |
//! | **forwarder** | never leaves the host netns | the run |
//! | **relay**, one per connection | host netns | one connection |
//!
//! The helper cannot also be the forwarder: getting *back* out of the sandbox netns needs
//! `CAP_SYS_ADMIN` in the namespace that owns the **host** netns, which is the initial user
//! namespace and not one an unprivileged daemon holds. So the listening socket is created inside
//! and handed out over `SCM_RIGHTS`, and the process that dials the pinned destination never enters
//! the sandbox at all.
//!
//! ## Two traps, both silent
//!
//! 1. **The owning user namespace, not the child's.** Bubblewrap nests a *second* user namespace,
//!    so `/proc/<child>/ns/user` is not the namespace that owns `/proc/<child>/ns/net`, and joining
//!    it returns `EPERM` that reads like kernel policy when it is an addressing mistake. The owner
//!    is reachable only through `ioctl(netns_fd, NS_GET_USERNS)`, and its owner uid is the daemon's
//!    own.
//! 2. **`child-pid` from `--info-fd`, never the spawned pid.** `Child::id()` is the *bubblewrap*
//!    pid and `/proc/<that>/ns/net` is the **host** namespace. Binding there would put the aperture
//!    on host loopback, reachable by everything on the machine — and every test would still pass.
//!
//! ## Forking from a threaded daemon
//!
//! The daemon is a multi-threaded tokio runtime, and `setns(CLONE_NEWUSER)` refuses a threaded
//! caller with `EINVAL`; a forked child is single-threaded, so it does not. Both children therefore
//! run **after `fork` and without `exec`**, which is the async-signal-safe regime: they call libc
//! entry points on descriptors and memory the parent prepared *before* the fork, and allocate
//! nothing. Every buffer, every counter page and every descriptor number here exists before the
//! fork for that reason.
//!
//! ## What is counted, and where
//!
//! Byte accounting lives in the relay, because the relay is the only thing that sees the bytes.
//! The counters are a shared anonymous page mapped before the fork, so the daemon reads what the
//! relays wrote without either of them holding a lock the other could be killed inside.

use std::fs::{File, OpenOptions};
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use substrate_wire::{
    APERTURE_CA_BUNDLE_PATH, APERTURE_HOSTS_PATH, APERTURE_LOOPBACK_ADDRESS, ApertureBytes,
    ApertureMechanism, ApertureMode, AppliedAperture, EgressApertureFact,
};

use crate::DriverError;

/// `_IO(NSIO, 0x1)` from `linux/nsfs.h`: the user namespace that owns a namespace descriptor.
const NS_GET_USERNS: libc::c_ulong = 0xb701;
/// Bytes moved per relay iteration, on the forked child's existing stack — never a new
/// allocation, because a relay runs after `fork` and before nothing.
const RELAY_BUFFER: usize = 16_384;
/// What the probe writes through a throwaway aperture and expects to read back inside the sandbox.
const PROBE_SENTINEL: &[u8] = b"substrate-egress-aperture-probe";
/// How long the startup probe waits for its own sentinel before calling the mechanism unproven.
///
/// Every wait in the probe is bounded by it. A daemon that cannot verify must **say so**: a probe
/// that hangs is a daemon that never starts and never explains why.
const PROBE_DEADLINE: std::time::Duration = std::time::Duration::from_secs(5);

/// One operator-declared egress aperture, resolved once at declaration (ADR 0013).
///
/// `host` and `port` are what the operator wrote; `pinned` is what it resolved to, once, at
/// startup. Both are kept because they can differ and only one of them is what the kernel is asked
/// for. The sandbox never resolves anything: it gets no resolver at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EgressAperture {
    pub name: String,
    pub host: String,
    pub port: u16,
    pub pinned: SocketAddr,
    /// The declared byte ceiling over both directions summed, per run (ADR 0014).
    ///
    /// `None` installs exactly what installed before the term existed: no comparison in the relay,
    /// nothing for the parent to read, and no field on the observation.
    pub max_bytes: Option<u64>,
}

impl EgressAperture {
    /// The capability fact for this aperture: its name and the address it is pinned to.
    pub(crate) fn fact(&self) -> EgressApertureFact {
        EgressApertureFact {
            name: self.name.clone(),
            destination: self.pinned.to_string(),
            max_bytes: self.max_bytes,
        }
    }

    /// True when the declared host is a name rather than a literal address, so a generated
    /// `/etc/hosts` entry has something to say and TLS has a name to verify.
    fn host_is_a_name(&self) -> bool {
        self.host.parse::<IpAddr>().is_err()
    }
}

/// The apertures fact, published only from proof (ADR 0013, invariant 3).
///
/// `verified` is whether the *mechanism* ran in a throwaway sandbox — never whether any declared
/// destination answered. A reachability check would make daemon readiness depend on somebody else's
/// uptime (`docs/design/10-destination-bound-egress.md` § 9 decision 6).
pub(crate) fn egress_apertures_fact(
    declared: &[EgressAperture],
    verified: bool,
) -> Option<Vec<EgressApertureFact>> {
    if declared.is_empty() || !verified {
        return None;
    }
    let mut facts: Vec<EgressApertureFact> = declared.iter().map(EgressAperture::fact).collect();
    facts.sort();
    Some(facts)
}

// -------------------------------------------------------------------------------------------
// The generated per-run resolution
// -------------------------------------------------------------------------------------------

/// What a run with an aperture gets bound into a sandbox that otherwise has no `/etc` at all.
///
/// Two files, generated per run and read-only. `hosts` maps exactly the declared name to loopback,
/// so a child uses the URL the operator declared and the forwarder is what answers; nothing else
/// resolves, because nothing else is in the file and there is no resolver behind it. The CA bundle
/// is a **snapshot**, not a bind of the live host path: TLS is byte-transparent through the
/// forwarder but unverifiable without a trust anchor, and a mid-run rotation must not change what a
/// running child already trusts (`docs/design/10a-egress-mechanism-spike.md` § 6 row 4).
pub(crate) struct GeneratedResolution {
    directory: tempfile::TempDir,
    hosts: PathBuf,
    ca_bundle: Option<PathBuf>,
}

impl GeneratedResolution {
    pub(crate) fn prepare(
        aperture: &EgressAperture,
        ca_source: Option<&Path>,
        root: &Path,
    ) -> Result<Self, DriverError> {
        let directory = tempfile::Builder::new()
            .prefix("aperture-")
            .tempdir_in(root)
            .map_err(|error| install_failed("private aperture directory", &error))?;
        let hosts = directory.path().join("hosts");
        let mut text = String::from("127.0.0.1\tlocalhost\n::1\tlocalhost\n");
        if aperture.host_is_a_name() {
            text.push_str(APERTURE_LOOPBACK_ADDRESS);
            text.push('\t');
            text.push_str(&aperture.host);
            text.push('\n');
        }
        std::fs::write(&hosts, text)
            .map_err(|error| install_failed("generated host mapping", &error))?;
        let ca_bundle = match ca_source {
            Some(source) => {
                let destination = directory.path().join("ca-bundle.crt");
                std::fs::copy(source, &destination)
                    .map_err(|error| install_failed("generated certificate bundle", &error))?;
                Some(destination)
            }
            None => None,
        };
        Ok(Self {
            directory,
            hosts,
            ca_bundle,
        })
    }

    /// `(host path, mount point)` for every generated file, in bind order.
    pub(crate) fn binds(&self) -> Vec<(&Path, &'static str)> {
        let mut binds = vec![(self.hosts.as_path(), APERTURE_HOSTS_PATH)];
        if let Some(bundle) = &self.ca_bundle {
            binds.push((bundle.as_path(), APERTURE_CA_BUNDLE_PATH));
        }
        binds
    }

    /// True when a trust anchor was generated, so the child can be told where it is.
    pub(crate) fn has_ca_bundle(&self) -> bool {
        self.ca_bundle.is_some()
    }

    /// Removes the generated files, reporting whether the removal was complete.
    pub(crate) fn close(self) -> bool {
        self.directory.close().is_ok()
    }
}

// -------------------------------------------------------------------------------------------
// Shared byte counters
// -------------------------------------------------------------------------------------------

/// Two `u64`s in a shared anonymous mapping, written by relays and read by the daemon.
///
/// Mapped **before** the fork, so a relay only ever stores into memory that already existed. There
/// is no lock: a relay can be killed by `cgroup.kill` at any instruction, and a lock it held would
/// then be a lock nobody releases.
struct SharedCounters {
    base: *mut libc::c_void,
}

// SAFETY: the mapping is a fixed-size shared page holding two `AtomicU64`s and nothing else, so
// every access is an atomic load or store through a stable address.
unsafe impl Send for SharedCounters {}
// SAFETY: as above — all access is atomic.
unsafe impl Sync for SharedCounters {}

impl SharedCounters {
    const BYTES: usize = 2 * std::mem::size_of::<u64>();

    fn new() -> io::Result<Self> {
        // SAFETY: an anonymous shared mapping of a fixed size, with no file backing and no fixed
        // address requested; the kernel either returns a valid mapping or MAP_FAILED.
        let base = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                Self::BYTES,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED | libc::MAP_ANONYMOUS,
                -1,
                0,
            )
        };
        if std::ptr::eq(base, libc::MAP_FAILED) {
            return Err(io::Error::last_os_error());
        }
        Ok(Self { base })
    }

    fn slot(&self, index: usize) -> &AtomicU64 {
        // SAFETY: `index` is 0 or 1 by construction below, the mapping holds exactly two `u64`s,
        // and `mmap` returns memory aligned to at least a page.
        unsafe { AtomicU64::from_ptr(self.base.cast::<u64>().add(index)) }
    }

    fn outbound(&self) -> &AtomicU64 {
        self.slot(0)
    }

    fn inbound(&self) -> &AtomicU64 {
        self.slot(1)
    }

    fn read(&self) -> ApertureBytes {
        ApertureBytes {
            to_destination: self.outbound().load(Ordering::Relaxed),
            from_destination: self.inbound().load(Ordering::Relaxed),
        }
    }
}

impl Drop for SharedCounters {
    fn drop(&mut self) {
        // SAFETY: unmapping exactly the mapping this value owns, once.
        unsafe {
            libc::munmap(self.base, Self::BYTES);
        }
    }
}

// -------------------------------------------------------------------------------------------
// Installation
// -------------------------------------------------------------------------------------------

/// One aperture, installed for one run and alive exactly as long as it is.
///
/// Dropping it kills the forwarder. In production the run's cgroup would reap it anyway — the
/// forwarder joins that cgroup before it touches the listening socket, which is the fix for the
/// spike's observed failure of a forwarder outliving its sandbox while pinning a dead namespace
/// (`docs/design/10a-egress-mechanism-spike.md` § 6 row 1). This is the second half of that belt:
/// where there is no cgroup there is still no orphan.
pub(crate) struct InstalledAperture {
    forwarder: libc::pid_t,
    counters: SharedCounters,
    name: String,
    destination: String,
    max_bytes: Option<u64>,
}

impl InstalledAperture {
    /// What was installed, with the bytes that have crossed so far.
    pub(crate) fn applied(&self) -> AppliedAperture {
        AppliedAperture {
            mode: ApertureMode::Aperture,
            name: self.name.clone(),
            destination: self.destination.clone(),
            mechanism: ApertureMechanism::LoopbackForwarder,
            bytes: self.counters.read(),
            max_bytes: self.max_bytes,
        }
    }

    /// Whether the declared ceiling has been reached, over both directions summed (ADR 0014).
    ///
    /// The same two relaxed loads the relay makes against the same page, so the parent classifies
    /// the run from the numbers that stopped it rather than from a second, weaker account — and
    /// answers the same for every declared value, including `Some(0)`, which the relay enforces at
    /// byte zero. An aperture declared without a ceiling is never exceeded: there is nothing to
    /// exceed.
    pub(crate) fn ceiling_exceeded(&self) -> bool {
        self.max_bytes.is_some_and(|ceiling| {
            let bytes = self.counters.read();
            bytes.to_destination.saturating_add(bytes.from_destination) >= ceiling
        })
    }
}

impl Drop for InstalledAperture {
    fn drop(&mut self) {
        // SAFETY: `forwarder` is a pid this process forked and has not yet reaped, so the number
        // cannot have been recycled behind our back.
        unsafe {
            libc::kill(self.forwarder, libc::SIGKILL);
            let mut status = 0;
            libc::waitpid(self.forwarder, &raw mut status, 0);
        }
    }
}

/// Installs the aperture into the namespace of a *bubblewrap-reported* child.
///
/// `sandbox_pid` must be the `child-pid` bubblewrap wrote to its `--info-fd`, never the pid of the
/// spawned bubblewrap process. `cgroup_procs` is the run's `cgroup.procs`; both forked processes
/// join it before they touch a socket.
///
/// # Errors
///
/// Every failure is `exec.aperture-install-failed` with nothing partial left behind: an aperture
/// that cannot be installed exactly as declared refuses the dispatch (ADR 0013).
pub(crate) fn install(
    aperture: &EgressAperture,
    sandbox_pid: u32,
    cgroup_procs: Option<&Path>,
) -> Result<InstalledAperture, DriverError> {
    let listener = listen_inside(sandbox_pid, aperture.port, cgroup_procs)?;
    let counters =
        SharedCounters::new().map_err(|error| install_failed("aperture byte counters", &error))?;
    let cgroup = open_cgroup(cgroup_procs)?;
    let forwarder = spawn_forwarder(
        &listener,
        aperture.pinned,
        &counters,
        cgroup.as_ref(),
        aperture.max_bytes,
    )?;
    drop(listener);
    Ok(InstalledAperture {
        forwarder,
        counters,
        name: aperture.name.clone(),
        destination: aperture.pinned.to_string(),
        max_bytes: aperture.max_bytes,
    })
}

/// Creates the listening socket **inside** the sandbox's network namespace and hands it back.
///
/// The helper does not also verify the aperture: at this point nothing is accepting on it yet, so a
/// connect from inside would sit in the backlog and the read behind it would never return. What
/// checks the aperture end to end is [`reaches_from_inside`], after the forwarder exists.
fn listen_inside(
    sandbox_pid: u32,
    port: u16,
    cgroup_procs: Option<&Path>,
) -> Result<OwnedFd, DriverError> {
    let netns = File::open(format!("/proc/{sandbox_pid}/ns/net"))
        .map_err(|error| install_failed("sandbox network namespace", &error))?;
    // Trap 1. The child's own `ns/user` is bubblewrap's *nested* namespace and joining it is
    // `EPERM`; the namespace that owns the netns is the one the daemon can enter.
    // SAFETY: an ioctl on a namespace descriptor this process opened, returning a new descriptor
    // or -1.
    let owner = unsafe { libc::ioctl(netns.as_raw_fd(), NS_GET_USERNS) };
    if owner < 0 {
        return Err(install_failed(
            "owning user namespace",
            &io::Error::last_os_error(),
        ));
    }
    // SAFETY: `owner` is a fresh descriptor the ioctl just returned and nothing else owns it.
    let owner = unsafe { OwnedFd::from_raw_fd(owner) };
    let cgroup = open_cgroup(cgroup_procs)?;

    let mut handback = [0_i32; 2];
    // SAFETY: a socketpair into a two-element array of the required size.
    if unsafe {
        libc::socketpair(
            libc::AF_UNIX,
            libc::SOCK_STREAM,
            0,
            handback.as_mut_ptr().cast(),
        )
    } != 0
    {
        return Err(install_failed(
            "aperture handback socket",
            &io::Error::last_os_error(),
        ));
    }
    // SAFETY: both descriptors were just created by `socketpair` and are owned here.
    let (parent_end, child_end) = unsafe {
        (
            OwnedFd::from_raw_fd(handback[0]),
            OwnedFd::from_raw_fd(handback[1]),
        )
    };

    let cgroup_fd = cgroup.as_ref().map_or(-1, AsRawFd::as_raw_fd);
    let netns_fd = netns.as_raw_fd();
    let owner_fd = owner.as_raw_fd();
    let child_fd = child_end.as_raw_fd();
    let parent_fd = parent_end.as_raw_fd();
    // SAFETY: `fork` from a threaded process yields a single-threaded child, which is what
    // `setns(CLONE_NEWUSER)` requires. The child below touches nothing but libc entry points and
    // descriptor numbers that existed before the fork.
    let pid = unsafe { libc::fork() };
    if pid < 0 {
        return Err(install_failed(
            "aperture helper",
            &io::Error::last_os_error(),
        ));
    }
    if pid == 0 {
        // SAFETY: post-fork child. Async-signal-safe calls only, no allocation, and `_exit` so no
        // atexit handler inherited from the daemon ever runs.
        unsafe {
            libc::close(parent_fd);
            libc::_exit(helper_body(cgroup_fd, owner_fd, netns_fd, port, child_fd));
        }
    }
    drop(child_end);
    let handback = receive_descriptor(parent_fd);
    let mut status = 0;
    // SAFETY: reaping a child this process just forked.
    unsafe {
        libc::waitpid(pid, &raw mut status, 0);
    }
    let helper_exit = if libc::WIFEXITED(status) {
        libc::WEXITSTATUS(status)
    } else {
        -1
    };
    match (handback, helper_exit) {
        (HelperHandback::Descriptor(listener), 0) => Ok(listener),
        (HelperHandback::Failure { stage, errno }, _) => Err(DriverError::failed(
            "exec.aperture-install-failed",
            format!(
                "The egress aperture could not be installed exactly as declared (stage {stage}, errno {errno}: {}).",
                io::Error::from_raw_os_error(errno)
            ),
        )),
        (_, code) => Err(DriverError::failed(
            "exec.aperture-install-failed",
            format!(
                "The egress aperture could not be installed exactly as declared (stage {code}, errno unavailable)."
            ),
        )),
    }
}

/// Everything the helper does inside the sandbox's namespaces, in the order it must do it.
///
/// The return value is the stage that failed, so an install failure names *where* rather than
/// leaving an operator to guess between a namespace, a bind and a handback.
///
/// # Safety
///
/// Must be called only in a freshly forked child. Every call below is async-signal-safe and none
/// allocates.
unsafe fn helper_body(
    cgroup_fd: RawFd,
    owner_fd: RawFd,
    netns_fd: RawFd,
    port: u16,
    handback_fd: RawFd,
) -> i32 {
    unsafe {
        // Everything the daemon had open is in this fd table, and one of those descriptors is the
        // spawn barrier the sandbox is waiting on: hold it and the sandbox never runs. Closing the
        // inherited table first is not hygiene, it is the difference between a run and a deadlock.
        keep_only(&[0, 1, 2, cgroup_fd, owner_fd, netns_fd, handback_fd]);
        // The cgroup next, and before any socket exists: a forwarder that outlives its sandbox
        // while holding sockets is what `cgroup.kill` has to be able to reach.
        if cgroup_fd >= 0 && !join_cgroup(cgroup_fd) {
            return helper_failure(handback_fd, 2);
        }
        if libc::setns(owner_fd, libc::CLONE_NEWUSER) != 0 {
            return helper_failure(handback_fd, 3);
        }
        if libc::setns(netns_fd, libc::CLONE_NEWNET) != 0 {
            return helper_failure(handback_fd, 4);
        }
        let listener = libc::socket(libc::AF_INET, libc::SOCK_STREAM | libc::SOCK_CLOEXEC, 0);
        if listener < 0 {
            return helper_failure(handback_fd, 5);
        }
        let one: libc::c_int = 1;
        libc::setsockopt(
            listener,
            libc::SOL_SOCKET,
            libc::SO_REUSEADDR,
            std::ptr::from_ref(&one).cast(),
            u32::try_from(std::mem::size_of::<libc::c_int>()).unwrap_or(4),
        );
        let address = loopback_sockaddr(port);
        if libc::bind(
            listener,
            std::ptr::from_ref(&address).cast(),
            u32::try_from(std::mem::size_of::<libc::sockaddr_in>()).unwrap_or(16),
        ) != 0
        {
            return helper_failure(handback_fd, 6);
        }
        if libc::listen(listener, 16) != 0 {
            return helper_failure(handback_fd, 7);
        }
        if !send_descriptor(handback_fd, listener) {
            return helper_failure(handback_fd, 8);
        }
        0
    }
}

/// Reports the failing stage and the thread-local errno without allocation in the post-fork child.
unsafe fn helper_failure(handback_fd: RawFd, stage: i32) -> i32 {
    unsafe {
        let errno = *libc::__errno_location();
        let record = [stage, errno];
        let _ = libc::write(
            handback_fd,
            record.as_ptr().cast(),
            std::mem::size_of_val(&record),
        );
        stage
    }
}

/// Connects to the installed aperture **from inside the sandbox's own namespace** and reads back.
///
/// This is what the startup probe proves: the mechanism works end to end in a throwaway sandbox —
/// a listener that exists in the right namespace, a forwarder that accepts, and bytes that cross.
/// It is deliberately not a connection to any declared destination, because a declared
/// destination's uptime is somebody else's (design 10 § 9 decision 6).
///
/// A separate short-lived process again, for the same reason the helper is one: `setns` with
/// `CLONE_NEWUSER` refuses a threaded caller, and nothing that entered the sandbox's namespace can
/// get back out.
fn reaches_from_inside(sandbox_pid: u32, port: u16, sentinel: &[u8]) -> bool {
    let Ok(netns) = File::open(format!("/proc/{sandbox_pid}/ns/net")) else {
        return false;
    };
    // SAFETY: an ioctl on a namespace descriptor this process opened.
    let owner = unsafe { libc::ioctl(netns.as_raw_fd(), NS_GET_USERNS) };
    if owner < 0 {
        return false;
    }
    // SAFETY: a fresh descriptor the ioctl returned, owned here.
    let owner = unsafe { OwnedFd::from_raw_fd(owner) };
    let owner_fd = owner.as_raw_fd();
    let netns_fd = netns.as_raw_fd();
    // SAFETY: `fork` from a threaded process yields the single-threaded child `setns` requires.
    let pid = unsafe { libc::fork() };
    if pid < 0 {
        return false;
    }
    if pid == 0 {
        // SAFETY: post-fork child; async-signal-safe calls only, then `_exit`.
        unsafe {
            keep_only(&[0, 1, 2, owner_fd, netns_fd]);
            let reached = if libc::setns(owner_fd, libc::CLONE_NEWUSER) == 0
                && libc::setns(netns_fd, libc::CLONE_NEWNET) == 0
            {
                i32::from(!probe_from_inside(port, sentinel))
            } else {
                1
            };
            libc::_exit(reached);
        }
    }
    let mut status = 0;
    // SAFETY: reaping a child this process forked.
    unsafe {
        libc::waitpid(pid, &raw mut status, 0);
    }
    libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0
}

/// The connect-and-read half of [`reaches_from_inside`], already inside the namespace.
///
/// # Safety
///
/// Post-fork child only.
unsafe fn probe_from_inside(port: u16, sentinel: &[u8]) -> bool {
    unsafe {
        let socket = libc::socket(libc::AF_INET, libc::SOCK_STREAM | libc::SOCK_CLOEXEC, 0);
        if socket < 0 {
            return false;
        }
        // The same deadline, enforced by the kernel. This runs in a forked child the daemon is
        // about to `waitpid` on, so a read that never returns is a daemon that never starts.
        let timeout = libc::timeval {
            tv_sec: libc::time_t::try_from(PROBE_DEADLINE.as_secs()).unwrap_or(5),
            tv_usec: 0,
        };
        for option in [libc::SO_RCVTIMEO, libc::SO_SNDTIMEO] {
            libc::setsockopt(
                socket,
                libc::SOL_SOCKET,
                option,
                std::ptr::from_ref(&timeout).cast(),
                u32::try_from(std::mem::size_of::<libc::timeval>()).unwrap_or(16),
            );
        }
        let address = loopback_sockaddr(port);
        if libc::connect(
            socket,
            std::ptr::from_ref(&address).cast(),
            u32::try_from(std::mem::size_of::<libc::sockaddr_in>()).unwrap_or(16),
        ) != 0
        {
            libc::close(socket);
            return false;
        }
        let mut buffer = [0_u8; 64];
        let mut filled = 0_usize;
        while filled < sentinel.len() && filled < buffer.len() {
            let read = libc::read(
                socket,
                buffer.as_mut_ptr().add(filled).cast(),
                buffer.len() - filled,
            );
            if read <= 0 {
                break;
            }
            filled += usize::try_from(read).unwrap_or(0);
        }
        libc::close(socket);
        filled == sentinel.len() && buffer[..filled] == *sentinel
    }
}

/// Forks the long-lived forwarder: host netns, run cgroup, one relay per connection.
fn spawn_forwarder(
    listener: &OwnedFd,
    destination: SocketAddr,
    counters: &SharedCounters,
    cgroup: Option<&File>,
    max_bytes: Option<u64>,
) -> Result<libc::pid_t, DriverError> {
    let SocketAddr::V4(destination) = destination else {
        return Err(DriverError::failed(
            "exec.aperture-install-failed",
            "Only IPv4 destinations are served by this mechanism.",
        ));
    };
    let address = sockaddr_in(*destination.ip(), destination.port());
    let listener_fd = listener.as_raw_fd();
    let cgroup_fd = cgroup.map_or(-1, AsRawFd::as_raw_fd);
    let to_destination: *const AtomicU64 = std::ptr::from_ref(counters.outbound());
    let from_destination: *const AtomicU64 = std::ptr::from_ref(counters.inbound());
    // Fixed before the fork, like every other value the forked children read. `u64::MAX` is "no
    // declared ceiling" and `0` is a ceiling of zero, so every value an operator can declare means
    // the same thing to the relay as it does to `InstalledAperture::ceiling_exceeded`. A sentinel
    // of `0` made those two disagree exactly at zero: the parent called the run exhausted at byte
    // zero and the relay passed everything (ADR 0014).
    let ceiling = max_bytes.unwrap_or(u64::MAX);

    // SAFETY: as in `listen_inside` — the child runs post-fork with libc calls only, over
    // descriptors, a `sockaddr_in` and a shared mapping that all existed before the fork.
    let pid = unsafe { libc::fork() };
    if pid < 0 {
        return Err(install_failed(
            "aperture forwarder",
            &io::Error::last_os_error(),
        ));
    }
    if pid == 0 {
        // SAFETY: post-fork child; `_exit` rather than `exit` so no inherited atexit handler runs.
        unsafe {
            libc::_exit(forwarder_body(
                cgroup_fd,
                listener_fd,
                &address,
                to_destination,
                from_destination,
                ceiling,
            ));
        }
    }
    Ok(pid)
}

/// The forwarder loop. Accepts inside-out connections and relays each one in its own child.
///
/// A relay per connection rather than a multiplexer, because the run's own `pids` bound already
/// caps how many there can be — a forwarder that had to police that itself would be a second,
/// weaker copy of a limit the cgroup already enforces.
///
/// # Safety
///
/// Post-fork child only. Nothing here allocates.
unsafe fn forwarder_body(
    cgroup_fd: RawFd,
    listener: RawFd,
    destination: &libc::sockaddr_in,
    to_destination: *const AtomicU64,
    from_destination: *const AtomicU64,
    ceiling: u64,
) -> i32 {
    unsafe {
        // As in the helper: the daemon's whole fd table came across the fork, and the run's own
        // spawn barrier is in it.
        keep_only(&[0, 1, 2, cgroup_fd, listener]);
        // Before the first byte and before the first accept: the cgroup is what reaps this.
        if cgroup_fd >= 0 && !join_cgroup(cgroup_fd) {
            return 2;
        }
        // Relays are fire and forget; the kernel reaps them so this loop never has to.
        libc::signal(libc::SIGCHLD, libc::SIG_IGN);
        loop {
            let accepted = libc::accept4(
                listener,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                libc::SOCK_CLOEXEC,
            );
            if accepted < 0 {
                if *libc::__errno_location() == libc::EINTR {
                    continue;
                }
                return 3;
            }
            let relay = libc::fork();
            if relay == 0 {
                libc::close(listener);
                libc::_exit(relay_body(
                    accepted,
                    destination,
                    to_destination,
                    from_destination,
                    ceiling,
                ));
            }
            // A failed fork means the run is out of process budget. Refusing this one connection
            // is the whole response: the aperture stays installed and the next may still succeed.
            libc::close(accepted);
        }
    }
}

/// One connection: dial the pinned tuple from the host namespace and move bytes both ways.
///
/// The destination is the pinned `address:port` and nothing the child sent can change it. A child
/// may put any `Host` header, any SNI and any URL on the wire; the bytes still go here
/// (`docs/design/10a-egress-mechanism-spike.md` § 6 row 7).
///
/// # Safety
///
/// Post-fork child only.
unsafe fn relay_body(
    client: RawFd,
    destination: &libc::sockaddr_in,
    to_destination: *const AtomicU64,
    from_destination: *const AtomicU64,
    ceiling: u64,
) -> i32 {
    unsafe {
        let upstream = libc::socket(libc::AF_INET, libc::SOCK_STREAM | libc::SOCK_CLOEXEC, 0);
        if upstream < 0 {
            return 1;
        }
        if libc::connect(
            upstream,
            std::ptr::from_ref(destination).cast(),
            u32::try_from(std::mem::size_of::<libc::sockaddr_in>()).unwrap_or(16),
        ) != 0
        {
            libc::close(upstream);
            return 2;
        }
        let mut buffer = [0_u8; RELAY_BUFFER];
        let mut client_open = true;
        let mut upstream_open = true;
        while client_open || upstream_open {
            // Checked before each pump rather than once per iteration, which is what bounds the
            // overshoot at one relay buffer per live relay rather than two (ADR 0014).
            if ceiling_reached(ceiling, to_destination, from_destination) {
                break;
            }
            let mut fds = [
                libc::pollfd {
                    fd: if client_open { client } else { -1 },
                    events: libc::POLLIN,
                    revents: 0,
                },
                libc::pollfd {
                    fd: if upstream_open { upstream } else { -1 },
                    events: libc::POLLIN,
                    revents: 0,
                },
            ];
            if libc::poll(fds.as_mut_ptr(), 2, -1) < 0 {
                if *libc::__errno_location() == libc::EINTR {
                    continue;
                }
                break;
            }
            if fds[0].revents != 0
                && !pump(
                    client,
                    upstream,
                    &mut buffer,
                    to_destination,
                    &mut client_open,
                )
            {
                libc::shutdown(upstream, libc::SHUT_WR);
            }
            if ceiling_reached(ceiling, to_destination, from_destination) {
                break;
            }
            if fds[1].revents != 0
                && !pump(
                    upstream,
                    client,
                    &mut buffer,
                    from_destination,
                    &mut upstream_open,
                )
            {
                libc::shutdown(client, libc::SHUT_WR);
            }
        }
        libc::close(upstream);
        libc::close(client);
        0
    }
}

/// Whether the declared ceiling has been reached, over both directions summed (ADR 0014).
///
/// `u64::MAX` is "no declared ceiling" and returns `false` without loading anything, so a run that
/// declared none pays one integer comparison and nothing else. It is the sentinel rather than `0`
/// because `0` is a ceiling an operator can write, and the parent enforces it at byte zero; a
/// ceiling of `u64::MAX` is not distinguishable from unbounded by any relay that would have to move
/// sixteen exabytes to tell them apart. Two relaxed loads otherwise, from the page this relay
/// already writes: there is no lock, because a relay can be killed by `cgroup.kill` at any
/// instruction and a lock it held would be a lock nobody releases.
///
/// # Safety
///
/// Post-fork child only.
unsafe fn ceiling_reached(
    ceiling: u64,
    to_destination: *const AtomicU64,
    from_destination: *const AtomicU64,
) -> bool {
    unsafe {
        ceiling != u64::MAX
            && (*to_destination)
                .load(Ordering::Relaxed)
                .saturating_add((*from_destination).load(Ordering::Relaxed))
                >= ceiling
    }
}

/// Moves one readable chunk from `source` to `sink`, counting it. `false` once `source` is done.
///
/// # Safety
///
/// Post-fork child only.
unsafe fn pump(
    source: RawFd,
    sink: RawFd,
    buffer: &mut [u8; RELAY_BUFFER],
    counter: *const AtomicU64,
    open: &mut bool,
) -> bool {
    unsafe {
        let read = libc::read(source, buffer.as_mut_ptr().cast(), buffer.len());
        if read <= 0 {
            *open = false;
            return false;
        }
        let mut written = 0_isize;
        while written < read {
            let count = libc::write(
                sink,
                buffer
                    .as_ptr()
                    .add(usize::try_from(written).unwrap_or(0))
                    .cast(),
                usize::try_from(read - written).unwrap_or(0),
            );
            if count <= 0 {
                *open = false;
                return false;
            }
            written += count;
        }
        (*counter).fetch_add(u64::try_from(read).unwrap_or(0), Ordering::Relaxed);
        true
    }
}

// -------------------------------------------------------------------------------------------
// The startup probe
// -------------------------------------------------------------------------------------------

/// Proves the mechanism in a throwaway sandbox, which is the only thing that publishes the fact.
///
/// A sentinel listener in the host namespace stands in for a destination, a bubblewrap sandbox with
/// `--unshare-net` stands in for a run, and the helper — inside that sandbox's namespace — connects
/// to the aperture and reads the sentinel back through the forwarder. Nothing here touches a
/// declared destination, so a daemon starts ready whatever the internet is doing.
pub(crate) fn mechanism_is_provable(bubblewrap: &Path) -> bool {
    if !bubblewrap.is_file() {
        return false;
    }
    let Ok(sentinel) = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)) else {
        return false;
    };
    let Ok(destination) = sentinel.local_addr() else {
        return false;
    };
    // Non-blocking with a deadline, and never a bare `accept`. A probe that did not reach the
    // sentinel would otherwise leave this thread blocked for the life of the daemon, and the join
    // below is where a daemon that failed its probe would hang instead of reporting it.
    if sentinel.set_nonblocking(true).is_err() {
        return false;
    }
    let responder = std::thread::spawn(move || {
        let deadline = std::time::Instant::now() + PROBE_DEADLINE;
        while std::time::Instant::now() < deadline {
            match sentinel.accept() {
                Ok((mut stream, _)) => {
                    use std::io::Write as _;
                    let _ = stream.set_nonblocking(false);
                    let _ = stream.write_all(PROBE_SENTINEL);
                    let _ = stream.flush();
                    return;
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                Err(_) => return,
            }
        }
    });
    let proven = probe_through_sandbox(bubblewrap, destination);
    let _ = responder.join();
    proven
}

/// A pipe whose descriptors survive `exec`, which is the only kind bubblewrap can be handed.
///
/// `std::io::pipe` sets `O_CLOEXEC`, so a sandbox given `--info-fd` from one reports nothing and
/// `--block-fd` from one is never read — and both failures are silent, which is how a test can
/// "open a sandbox" that never opened. `pipe2` with no flags is what the spawn barrier already uses
/// (`crates/substrate-host/src/process.rs`).
fn inheritable_pipe() -> io::Result<(File, File)> {
    let mut descriptors = [-1; 2];
    // SAFETY: `descriptors` points at two writable integers; `pipe2` initializes both on success.
    if unsafe { libc::pipe2(descriptors.as_mut_ptr(), 0) } != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: a successful `pipe2` returns two fresh descriptors owned by this function.
    Ok(unsafe {
        (
            File::from_raw_fd(descriptors[0]),
            File::from_raw_fd(descriptors[1]),
        )
    })
}

fn probe_through_sandbox(bubblewrap: &Path, destination: SocketAddr) -> bool {
    let Ok(info) = inheritable_pipe() else {
        return false;
    };
    let Ok(block) = inheritable_pipe() else {
        return false;
    };
    let (mut info_read, info_write) = info;
    let (block_read, block_write) = block;
    let info_fd = info_write.as_raw_fd();
    let block_fd = block_read.as_raw_fd();
    let mut command = std::process::Command::new(bubblewrap);
    command
        .env_clear()
        .args([
            "--unshare-user",
            "--unshare-ipc",
            "--unshare-pid",
            "--unshare-net",
            "--unshare-uts",
            "--new-session",
            "--die-with-parent",
            "--clearenv",
            "--ro-bind",
            "/usr",
            "/usr",
            "--ro-bind-try",
            "/lib",
            "/lib",
            "--ro-bind-try",
            "/lib64",
            "/lib64",
            "--proc",
            "/proc",
            "--info-fd",
        ])
        .arg(info_fd.to_string())
        .arg("--block-fd")
        .arg(block_fd.to_string())
        .args(["--", "/bin/true"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let Ok(mut child) = command.spawn() else {
        return false;
    };
    drop(info_write);
    let proven = read_sandbox_pid(&mut info_read).is_some_and(|sandbox_pid| {
        // A throwaway aperture pinned at the sentinel listener, installed exactly the way a run's
        // aperture is installed — and then reached from inside the sandbox, which is the claim.
        let installed = SharedCounters::new().ok().and_then(|counters| {
            let listener = listen_inside(sandbox_pid, destination.port(), None).ok()?;
            // The probe proves the *mechanism*, never a declaration: no ceiling, because there is
            // no operator statement here to enforce.
            let forwarder = spawn_forwarder(&listener, destination, &counters, None, None).ok()?;
            Some(InstalledAperture {
                forwarder,
                counters,
                name: String::new(),
                destination: destination.to_string(),
                max_bytes: None,
            })
        });
        installed.is_some_and(|installed| {
            let reached = reaches_from_inside(sandbox_pid, destination.port(), PROBE_SENTINEL);
            // Dropped here rather than at the end of the scope: the forwarder must be gone before
            // the sandbox is, or the probe leaves the very orphan it exists to rule out.
            drop(installed);
            reached
        })
    });
    release_sandbox(block_write);
    let _ = child.wait();
    proven
}

/// Releases a sandbox held at `--block-fd` by writing the byte it is waiting for.
///
/// A byte and not just a close, because closing only releases the sandbox if **every** copy of the
/// write end is gone — and this process forks. The daemon's own barrier already works this way
/// (`release_barrier` in `crates/substrate-host/src/process.rs`); a probe that relied on EOF would
/// be a sandbox that never runs and a `wait` that never returns.
fn release_sandbox(mut barrier: File) {
    use std::io::Write as _;
    let _ = barrier.write_all(&[1]);
    let _ = barrier.flush();
    drop(barrier);
}

/// Reads bubblewrap's `--info-fd` JSON far enough to take `child-pid` — trap 2 in one place.
pub(crate) fn read_sandbox_pid<R: std::io::Read>(info: &mut R) -> Option<u32> {
    let mut buffer = Vec::new();
    let mut chunk = [0_u8; 512];
    loop {
        if let Some(pid) = sandbox_pid_from(&buffer) {
            return Some(pid);
        }
        let read = info.read(&mut chunk).ok()?;
        if read == 0 {
            return sandbox_pid_from(&buffer);
        }
        buffer.extend_from_slice(&chunk[..read]);
        if buffer.len() > 8192 {
            return None;
        }
    }
}

/// The `child-pid` field, taken without waiting for bubblewrap to close the descriptor.
///
/// Bubblewrap writes the whole object before it releases `--block-fd` but keeps the descriptor
/// open, so waiting for EOF here would deadlock against the barrier this exists to run inside.
pub(crate) fn sandbox_pid_from(bytes: &[u8]) -> Option<u32> {
    let text = std::str::from_utf8(bytes).ok()?;
    let after = text.split_once("\"child-pid\"")?.1;
    let after = after.trim_start().strip_prefix(':')?.trim_start();
    let digits: String = after.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

// -------------------------------------------------------------------------------------------
// Small shared machinery
// -------------------------------------------------------------------------------------------

fn install_failed(what: &str, error: &io::Error) -> DriverError {
    DriverError::failed(
        "exec.aperture-install-failed",
        format!(
            "The egress aperture could not be installed exactly as declared ({what}: {error})."
        ),
    )
}

fn open_cgroup(procs: Option<&Path>) -> Result<Option<File>, DriverError> {
    procs
        .map(|path| {
            OpenOptions::new()
                .write(true)
                .open(path)
                .map_err(|error| install_failed("run cgroup", &error))
        })
        .transpose()
}

/// Writes `0` to an open `cgroup.procs`, which is cgroup v2 for "the calling process".
///
/// # Safety
///
/// Post-fork child only.
unsafe fn join_cgroup(cgroup_fd: RawFd) -> bool {
    unsafe { libc::write(cgroup_fd, c"0".as_ptr().cast(), 1) == 1 }
}

fn loopback_sockaddr(port: u16) -> libc::sockaddr_in {
    sockaddr_in(Ipv4Addr::LOCALHOST, port)
}

fn sockaddr_in(address: Ipv4Addr, port: u16) -> libc::sockaddr_in {
    let mut value: libc::sockaddr_in = unsafe { std::mem::zeroed() };
    value.sin_family = libc::c_ushort::try_from(libc::AF_INET).unwrap_or(2);
    value.sin_port = port.to_be();
    value.sin_addr = libc::in_addr {
        s_addr: u32::from(address).to_be(),
    };
    value
}

/// Sends one descriptor over a Unix socket as `SCM_RIGHTS`.
///
/// # Safety
///
/// Post-fork child only.
unsafe fn send_descriptor(socket: RawFd, descriptor: RawFd) -> bool {
    unsafe {
        let mut payload = [0_u8; 1];
        let mut iov = libc::iovec {
            iov_base: payload.as_mut_ptr().cast(),
            iov_len: payload.len(),
        };
        let mut control = [0_u8; 64];
        let mut message: libc::msghdr = std::mem::zeroed();
        message.msg_iov = &raw mut iov;
        message.msg_iovlen = 1;
        message.msg_control = control.as_mut_ptr().cast();
        message.msg_controllen = control.len();
        let header = libc::CMSG_FIRSTHDR(&raw const message);
        if header.is_null() {
            return false;
        }
        (*header).cmsg_level = libc::SOL_SOCKET;
        (*header).cmsg_type = libc::SCM_RIGHTS;
        (*header).cmsg_len = libc::CMSG_LEN(4) as usize;
        std::ptr::write_unaligned(libc::CMSG_DATA(header).cast::<RawFd>(), descriptor);
        message.msg_controllen = libc::CMSG_SPACE(4) as usize;
        libc::sendmsg(socket, &raw const message, 0) == 1
    }
}

/// Receives one descriptor sent as `SCM_RIGHTS`, or nothing if the sender failed before sending.
enum HelperHandback {
    Descriptor(OwnedFd),
    Failure { stage: i32, errno: i32 },
    Missing,
}

fn receive_descriptor(socket: RawFd) -> HelperHandback {
    // SAFETY: a `recvmsg` into stack buffers of the sizes its own macros computed, followed by a
    // bounds-checked read of exactly the descriptor the kernel placed in the control message.
    unsafe {
        let mut payload = [0_u8; 8];
        let mut iov = libc::iovec {
            iov_base: payload.as_mut_ptr().cast(),
            iov_len: payload.len(),
        };
        let mut control = [0_u8; 64];
        let mut message: libc::msghdr = std::mem::zeroed();
        message.msg_iov = &raw mut iov;
        message.msg_iovlen = 1;
        message.msg_control = control.as_mut_ptr().cast();
        message.msg_controllen = control.len();
        let received = libc::recvmsg(socket, &raw mut message, 0);
        if received == 8 {
            let stage = i32::from_ne_bytes(payload[..4].try_into().unwrap_or_default());
            let errno = i32::from_ne_bytes(payload[4..].try_into().unwrap_or_default());
            return HelperHandback::Failure { stage, errno };
        }
        if received != 1 {
            return HelperHandback::Missing;
        }
        let header = libc::CMSG_FIRSTHDR(&raw const message);
        if header.is_null()
            || (*header).cmsg_level != libc::SOL_SOCKET
            || (*header).cmsg_type != libc::SCM_RIGHTS
            || (*header).cmsg_len < libc::CMSG_LEN(4) as usize
        {
            return HelperHandback::Missing;
        }
        let descriptor = std::ptr::read_unaligned(libc::CMSG_DATA(header).cast::<RawFd>());
        if descriptor >= 0 {
            HelperHandback::Descriptor(OwnedFd::from_raw_fd(descriptor))
        } else {
            HelperHandback::Missing
        }
    }
}

/// Closes every descriptor except the listed ones, one `close_range` per gap.
///
/// # Safety
///
/// Post-fork child only.
unsafe fn keep_only(retained: &[RawFd]) {
    unsafe {
        let mut sorted = [0_i32; 8];
        let mut count = 0;
        for descriptor in retained {
            // `-1` is the "no cgroup was configured" placeholder; there is nothing to keep.
            if *descriptor >= 0 && count < sorted.len() {
                sorted[count] = *descriptor;
                count += 1;
            }
        }
        sorted[..count].sort_unstable();
        let mut low = 0_u32;
        for keep in &sorted[..count] {
            let keep = u32::try_from(*keep).unwrap_or(0);
            if keep > low {
                libc::syscall(libc::SYS_close_range, low, keep - 1, 0);
            }
            low = keep.saturating_add(1);
        }
        libc::syscall(libc::SYS_close_range, low, libc::c_uint::MAX, 0);
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;
    use std::net::{Ipv4Addr, SocketAddr, TcpListener};
    use std::os::fd::AsRawFd as _;
    use std::path::{Path, PathBuf};
    use std::process::{Child, Command, Stdio};

    use substrate_wire::{
        ApertureMechanism, ApertureMode, ConfinementRequest, NetworkMode, SandboxProfile,
    };

    use super::{
        EgressAperture, GeneratedResolution, HelperHandback, InstalledAperture,
        egress_apertures_fact, install, read_sandbox_pid, receive_descriptor, sandbox_pid_from,
    };
    use crate::{DriverErrorClass, HostConfig};

    const BUBBLEWRAP: &str = "/usr/bin/bwrap";
    /// What the model-free fake app-server answers with. Not a model, not a protocol: bytes.
    const APP_SERVER_BODY: &[u8] = b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok";
    /// An address off this machine, never connected to from the host: what the *kernel* refuses
    /// when a namespace has no route at all, as against the `ECONNREFUSED` a bare loopback gives.
    ///
    /// Both endpoints these cases use are on loopback, so from inside the sandbox "the second
    /// endpoint" and "the pinned address, dialled directly" are the same address and cannot be told
    /// apart. Telling them apart needs a destination on a non-loopback host address, which is a
    /// lane this case does not have; what is proved here is that neither of them is reached.
    const PUBLIC_ADDRESS: SocketAddr =
        SocketAddr::new(std::net::IpAddr::V4(Ipv4Addr::new(198, 51, 100, 7)), 443);

    #[test]
    fn helper_failure_handback_preserves_stage_and_errno() {
        let mut sockets = [-1_i32; 2];
        // SAFETY: `sockets` is writable storage for exactly two descriptors.
        assert_eq!(
            unsafe {
                libc::socketpair(
                    libc::AF_UNIX,
                    libc::SOCK_SEQPACKET | libc::SOCK_CLOEXEC,
                    0,
                    sockets.as_mut_ptr(),
                )
            },
            0
        );
        let mut payload = Vec::new();
        payload.extend_from_slice(&7_i32.to_ne_bytes());
        payload.extend_from_slice(&libc::EADDRINUSE.to_ne_bytes());
        // SAFETY: both descriptors are live and payload is valid for its declared length.
        assert_eq!(
            unsafe {
                libc::send(
                    sockets[1],
                    payload.as_ptr().cast(),
                    payload.len(),
                    libc::MSG_NOSIGNAL,
                )
            },
            8
        );
        match receive_descriptor(sockets[0]) {
            HelperHandback::Failure { stage, errno } => {
                assert_eq!(stage, 7);
                assert_eq!(errno, libc::EADDRINUSE);
            }
            _ => panic!("failure handback was not preserved"),
        }
        // SAFETY: the test owns both descriptors and closes each once.
        unsafe {
            libc::close(sockets[0]);
            libc::close(sockets[1]);
        }
    }

    /// A throwaway sandbox held open at bubblewrap's own `--block-fd` barrier.
    ///
    /// This is the barrier a real dispatch installs an aperture at, so a test that installs one
    /// here is installing it where production does and not somewhere more convenient.
    struct Sandbox {
        child: Child,
        release: Option<std::fs::File>,
        pid: u32,
    }

    impl Sandbox {
        fn open() -> Option<Self> {
            if !Path::new(BUBBLEWRAP).is_file() {
                return None;
            }
            let (mut info_read, info_write) = super::inheritable_pipe().expect("info pipe");
            let (block_read, block_write) = super::inheritable_pipe().expect("block pipe");
            let info_fd = info_write.as_raw_fd();
            let block_fd = block_read.as_raw_fd();
            let mut command = Command::new(BUBBLEWRAP);
            command
                .env_clear()
                .args([
                    "--unshare-user",
                    "--unshare-ipc",
                    "--unshare-pid",
                    "--unshare-net",
                    "--unshare-uts",
                    "--new-session",
                    "--die-with-parent",
                    "--clearenv",
                    "--ro-bind",
                    "/usr",
                    "/usr",
                    "--ro-bind-try",
                    "/lib",
                    "/lib",
                    "--ro-bind-try",
                    "/lib64",
                    "/lib64",
                    "--proc",
                    "/proc",
                    "--info-fd",
                ])
                .arg(info_fd.to_string())
                .arg("--block-fd")
                .arg(block_fd.to_string())
                .args(["--", "/bin/true"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            let child = command.spawn().expect("spawn the throwaway sandbox");
            drop(info_write);
            // Never `?`: a sandbox that reported no pid is a broken harness, not an absent one, and
            // silently skipping here is exactly how these cases would pass without running.
            let pid = read_sandbox_pid(&mut info_read)
                .expect("bubblewrap reported no child-pid on its --info-fd");
            Some(Self {
                child,
                release: Some(block_write),
                pid,
            })
        }
    }

    impl Drop for Sandbox {
        fn drop(&mut self) {
            if let Some(barrier) = self.release.take() {
                super::release_sandbox(barrier);
            }
            let _ = self.child.wait();
        }
    }

    /// A model-free app-server: one listener, one fixed body, no protocol opinion.
    struct AppServer {
        address: SocketAddr,
        handle: Option<std::thread::JoinHandle<()>>,
    }

    impl AppServer {
        fn start() -> Self {
            let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind app server");
            let address = listener.local_addr().expect("app server address");
            let handle = std::thread::spawn(move || {
                while let Ok((mut stream, _)) = listener.accept() {
                    if stream.write_all(APP_SERVER_BODY).is_err() {
                        break;
                    }
                    let _ = stream.flush();
                }
            });
            Self {
                address,
                handle: Some(handle),
            }
        }
    }

    impl Drop for AppServer {
        fn drop(&mut self) {
            // The listener dies with the thread once the test drops its last connection.
            self.handle.take();
        }
    }

    /// What a connection attempt from inside a sandbox's own network namespace did.
    #[derive(Debug, PartialEq, Eq)]
    enum Reach {
        /// Connected and read exactly the bytes the app-server serves.
        Served,
        /// The kernel had no route at all: the namespace has loopback and nothing else.
        Unreachable,
        /// Loopback answered but nothing was listening on that port inside the namespace.
        Refused,
        Other(i32),
    }

    /// Connects to `address` from inside the sandbox's network namespace.
    ///
    /// The confined child and this probe are in the same namespace, so what the kernel refuses one
    /// it refuses the other. A forked, single-threaded process, because `setns(CLONE_NEWUSER)`
    /// refuses a threaded caller.
    fn reach(sandbox_pid: u32, address: SocketAddr) -> Reach {
        let SocketAddr::V4(address) = address else {
            return Reach::Other(-1);
        };
        let target = super::sockaddr_in(*address.ip(), address.port());
        let netns = std::fs::File::open(format!("/proc/{sandbox_pid}/ns/net")).expect("netns");
        // SAFETY: an ioctl on a namespace descriptor this process opened.
        let owner = unsafe { libc::ioctl(netns.as_raw_fd(), super::NS_GET_USERNS) };
        assert!(owner >= 0, "owning user namespace");
        let netns_fd = netns.as_raw_fd();
        // SAFETY: forked child is single-threaded and calls libc entry points only.
        let pid = unsafe { libc::fork() };
        assert!(pid >= 0, "fork");
        if pid == 0 {
            // SAFETY: post-fork child, async-signal-safe calls only.
            unsafe {
                let code = reach_body(owner, netns_fd, &target);
                libc::_exit(code);
            }
        }
        let mut status = 0;
        // SAFETY: reaping a child this process forked.
        unsafe {
            libc::waitpid(pid, &raw mut status, 0);
        }
        // SAFETY: the ioctl descriptor is owned here and closed exactly once.
        unsafe {
            libc::close(owner);
        }
        match libc::WEXITSTATUS(status) {
            0 => Reach::Served,
            1 => Reach::Unreachable,
            2 => Reach::Refused,
            other => Reach::Other(other),
        }
    }

    /// # Safety
    ///
    /// Post-fork child only.
    unsafe fn reach_body(owner: i32, netns: i32, target: &libc::sockaddr_in) -> i32 {
        unsafe {
            if libc::setns(owner, libc::CLONE_NEWUSER) != 0 {
                return 10;
            }
            if libc::setns(netns, libc::CLONE_NEWNET) != 0 {
                return 11;
            }
            let socket = libc::socket(libc::AF_INET, libc::SOCK_STREAM | libc::SOCK_CLOEXEC, 0);
            if socket < 0 {
                return 12;
            }
            if libc::connect(
                socket,
                std::ptr::from_ref(target).cast(),
                u32::try_from(std::mem::size_of::<libc::sockaddr_in>()).unwrap_or(16),
            ) != 0
            {
                let errno = *libc::__errno_location();
                libc::close(socket);
                return match errno {
                    libc::ENETUNREACH | libc::EHOSTUNREACH => 1,
                    libc::ECONNREFUSED => 2,
                    other => 20 + other,
                };
            }
            let mut buffer = [0_u8; 64];
            let read = libc::read(socket, buffer.as_mut_ptr().cast(), buffer.len());
            libc::close(socket);
            if read <= 0 {
                return 13;
            }
            let filled = usize::try_from(read).unwrap_or(0);
            i32::from(buffer[..filled] != *APP_SERVER_BODY)
        }
    }

    fn aperture(name: &str, pinned: SocketAddr) -> EgressAperture {
        EgressAperture {
            name: name.to_owned(),
            host: "app.example.invalid".to_owned(),
            port: pinned.port(),
            pinned,
            max_bytes: None,
        }
    }

    /// The same declaration with the one optional term ADR 0014 adds.
    fn capped_aperture(name: &str, pinned: SocketAddr, max_bytes: u64) -> EgressAperture {
        EgressAperture {
            max_bytes: Some(max_bytes),
            ..aperture(name, pinned)
        }
    }

    fn sandbox_request(network: NetworkMode, name: Option<&str>) -> ConfinementRequest {
        ConfinementRequest {
            capability_snapshot: "sha256:".to_owned() + &"0".repeat(64),
            network,
            aperture: name.map(str::to_owned),
            profile: SandboxProfile::Workspace,
            required: true,
        }
    }

    /// A run that asks for nothing gets nothing: no aperture, no listener, no route.
    ///
    /// Two claims, and the second is the one that matters. Admission returning `None` is a
    /// statement about this code; a live sandbox with no listener anywhere in its namespace is a
    /// statement about the kernel.
    #[test]
    fn egress_defaults_to_none() {
        let config = HostConfig::minimum("/does/not/exist");
        assert!(
            crate::process::ProcessRuntime::admit_egress_aperture(
                &config,
                None,
                &sandbox_request(NetworkMode::None, None),
            )
            .expect("the default is admissible")
            .is_none(),
            "a start naming no aperture selected one"
        );
        assert_eq!(
            egress_apertures_fact(&[], true),
            None,
            "a daemon declaring no aperture published the fact"
        );
        let Some(sandbox) = Sandbox::open() else {
            return;
        };
        let server = AppServer::start();
        // The host's listener is on loopback, and the sandbox has a loopback of its own with
        // nothing on it: the connection is refused *inside* rather than routed outside, which is
        // the whole point — there is no outside.
        // Assert on the guarantee — *did not reach* — and report the errno rather than pinning
        // it. Both `Refused` and `Unreachable` prove the same thing, and which one the kernel
        // returns is environment-dependent: under `strace -f` this host answers `Unreachable`
        // where it otherwise answers `Refused`. A test that pins the variant goes red on a
        // different kernel for a reason that is not a regression.
        let to_host = reach(sandbox.pid, server.address);
        assert_ne!(
            to_host,
            Reach::Served,
            "a sandbox with no aperture reached a host listener (observed {to_host:?})"
        );
        let to_public = reach(sandbox.pid, PUBLIC_ADDRESS);
        assert_ne!(
            to_public,
            Reach::Served,
            "a sandbox with no aperture had a route off the machine (observed {to_public:?})"
        );
    }

    /// The declared destination is reachable through the aperture, and the bytes are the server's.
    #[test]
    fn declared_aperture_is_reachable() {
        let Some(sandbox) = Sandbox::open() else {
            return;
        };
        let server = AppServer::start();
        let declared = aperture("model", server.address);
        let installed = install(&declared, sandbox.pid, None).expect("install the aperture");
        assert_eq!(
            reach(
                sandbox.pid,
                SocketAddr::from((Ipv4Addr::LOCALHOST, declared.port))
            ),
            Reach::Served,
            "the declared aperture did not serve the declared destination"
        );
        drop(installed);
    }

    /// One run, two connects: the aperture serves, everything else is refused by the kernel.
    ///
    /// And the *request* half of the same claim: an aperture this deployment never declared is
    /// refused with the name in the message, because an operator debugging a harness needs to know
    /// which name was asked for.
    #[test]
    fn undeclared_destination_is_unreachable_and_named() {
        let config = HostConfig::minimum("/does/not/exist");
        let published = vec![substrate_wire::EgressApertureFact {
            name: "model".to_owned(),
            destination: "127.0.0.1:1".to_owned(),
            max_bytes: None,
        }];
        let refusal = crate::process::ProcessRuntime::admit_egress_aperture(
            &config,
            Some(&published),
            &sandbox_request(NetworkMode::Aperture, Some("registry")),
        )
        .expect_err("an undeclared aperture is refused");
        assert_eq!(refusal.class, DriverErrorClass::Unserved);
        assert_eq!(refusal.code, "exec.aperture-undeclared");
        assert!(
            refusal.message.contains("registry"),
            "the refusal did not name the aperture: {}",
            refusal.message
        );

        let Some(sandbox) = Sandbox::open() else {
            return;
        };
        let inside = AppServer::start();
        let outside = AppServer::start();
        let declared = aperture("model", inside.address);
        let installed = install(&declared, sandbox.pid, None).expect("install the aperture");
        assert_eq!(
            reach(
                sandbox.pid,
                SocketAddr::from((Ipv4Addr::LOCALHOST, declared.port))
            ),
            Reach::Served,
            "the aperture did not serve its own destination"
        );
        assert_eq!(
            reach(sandbox.pid, outside.address),
            Reach::Refused,
            "a second endpoint outside the aperture was reachable"
        );
        assert_eq!(
            reach(sandbox.pid, PUBLIC_ADDRESS),
            Reach::Unreachable,
            "the run had a route off the machine besides its aperture"
        );
        drop(installed);
    }

    /// A name the deployment did not declare, and a fact that is absent, are both `unserved`.
    #[test]
    fn aperture_outside_operator_declaration_is_unserved() {
        let mut config = HostConfig::minimum("/does/not/exist");
        config.egress_apertures = vec![aperture(
            "model",
            SocketAddr::from((Ipv4Addr::LOCALHOST, 9)),
        )];
        let absent = crate::process::ProcessRuntime::admit_egress_aperture(
            &config,
            None,
            &sandbox_request(NetworkMode::Aperture, Some("model")),
        )
        .expect_err("an unverified mechanism serves no aperture");
        assert_eq!(absent.class, DriverErrorClass::Unserved);
        assert_eq!(absent.code, "exec.egress-apertures-unserved");

        let published = vec![config.egress_apertures[0].fact()];
        let selected = crate::process::ProcessRuntime::admit_egress_aperture(
            &config,
            Some(&published),
            &sandbox_request(NetworkMode::Aperture, Some("model")),
        )
        .expect("a declared, verified aperture is admissible")
        .expect("a name selects an aperture");
        assert_eq!(selected.name, "model");

        let escalation = crate::process::ProcessRuntime::admit_egress_aperture(
            &config,
            Some(&published),
            &sandbox_request(NetworkMode::Aperture, Some("10.0.0.1:443")),
        )
        .expect_err("a destination where a name belongs is refused");
        assert_eq!(escalation.class, DriverErrorClass::Refused);
        assert_eq!(escalation.code, "exec.aperture-destination-in-request");
    }

    /// What was installed is reported: name, pinned destination, mechanism and the bytes that
    /// crossed — counted in the forwarder, which is the only thing that sees them.
    #[test]
    fn applied_aperture_is_observed() {
        let Some(sandbox) = Sandbox::open() else {
            return;
        };
        let server = AppServer::start();
        let declared = aperture("model", server.address);
        let installed = install(&declared, sandbox.pid, None).expect("install the aperture");
        assert_eq!(
            reach(
                sandbox.pid,
                SocketAddr::from((Ipv4Addr::LOCALHOST, declared.port))
            ),
            Reach::Served
        );
        let applied = wait_for_bytes(&installed);
        assert_eq!(applied.mode, ApertureMode::Aperture);
        assert_eq!(applied.name, "model");
        assert_eq!(applied.destination, server.address.to_string());
        assert_eq!(applied.mechanism, ApertureMechanism::LoopbackForwarder);
        assert_eq!(
            applied.bytes.from_destination,
            APP_SERVER_BODY.len() as u64,
            "the observed byte count is not what the destination sent"
        );
        assert_ne!(
            applied.destination, declared.host,
            "the observation reported the configured host string, not the pinned address"
        );
        drop(installed);
    }

    /// The relay counts after it has copied, so a reader may be one scheduling quantum early.
    fn wait_for_bytes(installed: &InstalledAperture) -> substrate_wire::AppliedAperture {
        for _ in 0..200 {
            let applied = installed.applied();
            if applied.bytes.from_destination > 0 {
                return applied;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        installed.applied()
    }

    /// The generated resolution is exactly the declared mapping, and nothing else resolves.
    #[test]
    fn the_generated_resolution_names_only_the_declared_host() {
        let root = tempfile::tempdir().unwrap();
        let declared = aperture("model", SocketAddr::from((Ipv4Addr::LOCALHOST, 443)));
        let generated = GeneratedResolution::prepare(&declared, None, root.path())
            .expect("generate the per-run resolution");
        let binds = generated.binds();
        assert_eq!(binds.len(), 1, "no trust anchor was configured");
        assert_eq!(binds[0].1, substrate_wire::APERTURE_HOSTS_PATH);
        let text = std::fs::read_to_string(binds[0].0).unwrap();
        assert!(text.contains("127.0.0.1\tapp.example.invalid\n"), "{text}");
        assert!(
            !text.contains("nameserver"),
            "the sandbox was given a resolver"
        );
        assert!(!generated.has_ca_bundle());

        let anchor = root.path().join("anchor.pem");
        std::fs::write(&anchor, b"-----BEGIN CERTIFICATE-----\n").unwrap();
        let with_anchor = GeneratedResolution::prepare(&declared, Some(&anchor), root.path())
            .expect("generate with a trust anchor");
        assert!(with_anchor.has_ca_bundle());
        let anchored = with_anchor.binds();
        assert_eq!(anchored[1].1, substrate_wire::APERTURE_CA_BUNDLE_PATH);
        // A snapshot, not a bind of the live path: rotating the source cannot change what a
        // running child already trusts.
        std::fs::write(&anchor, b"rotated").unwrap();
        assert_eq!(
            std::fs::read(anchored[1].0).unwrap(),
            b"-----BEGIN CERTIFICATE-----\n"
        );
        assert!(generated.close());
        assert!(with_anchor.close());
    }

    /// Trap 2: the pid the aperture binds against comes out of bubblewrap's own report.
    #[test]
    fn the_sandbox_pid_comes_from_the_info_report() {
        let report = br#"{
    "child-pid": 2313820,
    "net-namespace": 4026533901
}"#;
        assert_eq!(sandbox_pid_from(report), Some(2_313_820));
        assert_eq!(sandbox_pid_from(b"{\"net-namespace\": 4026533901}"), None);
        assert_eq!(sandbox_pid_from(b"{\"child-pid\":"), None);
    }

    /// The startup probe: the mechanism, exercised end to end in a throwaway sandbox.
    ///
    /// This is the one path a run never takes and the daemon always does, so it gets its own case.
    /// Absent, never reported as passed: where bubblewrap is not on the machine the case makes no
    /// claim at all, because a probe that cannot run has proven nothing.
    #[test]
    fn the_mechanism_is_proven_in_a_throwaway_sandbox() {
        let bubblewrap = Path::new(BUBBLEWRAP);
        if !bubblewrap.is_file() {
            return;
        }
        assert!(
            super::mechanism_is_provable(bubblewrap),
            "the aperture mechanism did not verify in a throwaway sandbox"
        );
        assert!(
            !super::mechanism_is_provable(Path::new("/does/not/exist")),
            "a missing confinement backend proved a mechanism"
        );
    }

    /// The fact is published only from proof, and it carries the pinned address.
    #[test]
    fn the_apertures_fact_needs_the_mechanism() {
        let declared = vec![
            aperture("model", SocketAddr::from((Ipv4Addr::LOCALHOST, 443))),
            aperture("audit", SocketAddr::from((Ipv4Addr::LOCALHOST, 8443))),
        ];
        assert_eq!(egress_apertures_fact(&declared, false), None);
        let fact = egress_apertures_fact(&declared, true).expect("a proven mechanism publishes");
        assert_eq!(fact[0].name, "audit", "the fact is sorted");
        assert_eq!(fact[1].destination, "127.0.0.1:443");
    }

    // ---------------------------------------------------------------------------------------
    // The declared byte ceiling (ADR 0014)
    // ---------------------------------------------------------------------------------------

    /// What the firehose offers a run: far more than any ceiling these cases declare, so a run
    /// that stops short stopped because substrate stopped it.
    const FIREHOSE_BYTES: u64 = 1 << 20;
    /// The ceiling these cases declare. A whole number of relay buffers, so "the ceiling" and
    /// "the ceiling plus one buffer" are unambiguous numbers rather than an off-by-a-chunk.
    const DECLARED_CEILING: u64 = 64 * 1024;

    /// A destination that answers every connection with [`FIREHOSE_BYTES`] bytes and then closes.
    ///
    /// [`AppServer`] sends 39 bytes, which no ceiling worth declaring would ever reach; a ceiling
    /// is only observable against a destination willing to send past it.
    struct Firehose {
        address: SocketAddr,
        handle: Option<std::thread::JoinHandle<()>>,
    }

    impl Firehose {
        fn start() -> Self {
            let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind firehose");
            let address = listener.local_addr().expect("firehose address");
            let handle = std::thread::spawn(move || {
                while let Ok((mut stream, _)) = listener.accept() {
                    let chunk = [b'x'; 4096];
                    let mut sent = 0_u64;
                    while sent < FIREHOSE_BYTES {
                        // A relay that stopped is a closed socket here; that is the case passing,
                        // not a failure, so the error ends this connection and nothing else.
                        if stream.write_all(&chunk).is_err() {
                            break;
                        }
                        sent += chunk.len() as u64;
                    }
                    let _ = stream.flush();
                }
            });
            Self {
                address,
                handle: Some(handle),
            }
        }
    }

    impl Drop for Firehose {
        fn drop(&mut self) {
            self.handle.take();
        }
    }

    /// Reads from `address` inside the sandbox's own namespace until the connection ends.
    ///
    /// The confined child's view: it is told nothing, it just runs out of stream (ADR 0014, "the
    /// child gets a closed socket"). What it read is not the claim — what the relay counted is —
    /// so this reports only that the connection ended.
    fn drain(sandbox_pid: u32, address: SocketAddr) {
        let SocketAddr::V4(address) = address else {
            panic!("the aperture serves IPv4 only");
        };
        let target = super::sockaddr_in(*address.ip(), address.port());
        let netns = std::fs::File::open(format!("/proc/{sandbox_pid}/ns/net")).expect("netns");
        // SAFETY: an ioctl on a namespace descriptor this process opened.
        let owner = unsafe { libc::ioctl(netns.as_raw_fd(), super::NS_GET_USERNS) };
        assert!(owner >= 0, "owning user namespace");
        let netns_fd = netns.as_raw_fd();
        // SAFETY: forked child is single-threaded, which `setns(CLONE_NEWUSER)` requires.
        let pid = unsafe { libc::fork() };
        assert!(pid >= 0, "fork");
        if pid == 0 {
            // SAFETY: post-fork child, async-signal-safe calls only.
            unsafe {
                let code = drain_body(owner, netns_fd, &target);
                libc::_exit(code);
            }
        }
        let mut status = 0;
        // SAFETY: reaping a child this process forked.
        unsafe {
            libc::waitpid(pid, &raw mut status, 0);
        }
        // SAFETY: the ioctl descriptor is owned here and closed exactly once.
        unsafe {
            libc::close(owner);
        }
        assert_eq!(
            libc::WEXITSTATUS(status),
            0,
            "the drain never reached the aperture"
        );
    }

    /// # Safety
    ///
    /// Post-fork child only.
    unsafe fn drain_body(owner: i32, netns: i32, target: &libc::sockaddr_in) -> i32 {
        unsafe {
            if libc::setns(owner, libc::CLONE_NEWUSER) != 0 {
                return 10;
            }
            if libc::setns(netns, libc::CLONE_NEWNET) != 0 {
                return 11;
            }
            let socket = libc::socket(libc::AF_INET, libc::SOCK_STREAM | libc::SOCK_CLOEXEC, 0);
            if socket < 0 {
                return 12;
            }
            if libc::connect(
                socket,
                std::ptr::from_ref(target).cast(),
                u32::try_from(std::mem::size_of::<libc::sockaddr_in>()).unwrap_or(16),
            ) != 0
            {
                libc::close(socket);
                return 13;
            }
            let mut buffer = [0_u8; 4096];
            loop {
                let read = libc::read(socket, buffer.as_mut_ptr().cast(), buffer.len());
                if read <= 0 {
                    break;
                }
            }
            libc::close(socket);
            0
        }
    }

    /// Waits until the counters stop moving, which is the relay having stopped or the stream done.
    fn settled(installed: &InstalledAperture) -> substrate_wire::ApertureBytes {
        let mut last = installed.applied().bytes;
        for _ in 0..200 {
            std::thread::sleep(std::time::Duration::from_millis(10));
            let now = installed.applied().bytes;
            if now == last && (now.to_destination + now.from_destination) > 0 {
                return now;
            }
            last = now;
        }
        last
    }

    /// The ceiling is a stop, not a report: the relay stops relaying, and the overshoot is the one
    /// stated bound — at most one relay buffer (ADR 0014, *Consequences*).
    #[test]
    fn a_declared_ceiling_stops_the_relay() {
        let Some(sandbox) = Sandbox::open() else {
            return;
        };
        let firehose = Firehose::start();
        let declared = capped_aperture("model", firehose.address, DECLARED_CEILING);
        let installed = install(&declared, sandbox.pid, None).expect("install the aperture");
        drain(
            sandbox.pid,
            SocketAddr::from((Ipv4Addr::LOCALHOST, declared.port)),
        );
        let bytes = settled(&installed);
        let total = bytes.to_destination + bytes.from_destination;
        assert!(
            total >= DECLARED_CEILING,
            "the relay stopped short of the declared ceiling: {total} < {DECLARED_CEILING}"
        );
        assert!(
            total <= DECLARED_CEILING + super::RELAY_BUFFER as u64,
            "the overshoot is larger than one relay buffer: {total} > {}",
            DECLARED_CEILING + super::RELAY_BUFFER as u64
        );
        assert!(
            installed.ceiling_exceeded(),
            "the parent cannot see the ceiling the relay stopped at"
        );
        let applied = installed.applied();
        assert_eq!(
            applied.max_bytes,
            Some(DECLARED_CEILING),
            "the observation does not state the ceiling the run ran under"
        );
        drop(installed);
    }

    /// The negative, which is the whole of "an aperture declared without the term keeps working
    /// byte for byte": the same destination, the same traffic, no ceiling, nothing stopped.
    #[test]
    fn an_aperture_without_a_ceiling_passes_the_same_traffic() {
        let Some(sandbox) = Sandbox::open() else {
            return;
        };
        let firehose = Firehose::start();
        let declared = aperture("model", firehose.address);
        let installed = install(&declared, sandbox.pid, None).expect("install the aperture");
        drain(
            sandbox.pid,
            SocketAddr::from((Ipv4Addr::LOCALHOST, declared.port)),
        );
        let bytes = settled(&installed);
        assert_eq!(
            bytes.from_destination, FIREHOSE_BYTES,
            "an aperture with no declared ceiling did not pass the whole stream"
        );
        assert!(
            !installed.ceiling_exceeded(),
            "an aperture with no ceiling reported one exceeded"
        );
        assert_eq!(
            installed.applied().max_bytes,
            None,
            "an aperture with no ceiling published one"
        );
        drop(installed);
    }

    /// The relay and the parent answer the same question the same way, including at zero.
    ///
    /// The daemon refuses `max=0` at startup, but `substrate_host::EgressAperture` is public and a
    /// caller may build one directly. A ceiling the parent treats as reached at byte zero while the
    /// relay treats it as absent is a bound that exists in the observation and nowhere on the byte
    /// path — the silent degradation invariant 3 forbids, and worse than either answer alone.
    #[test]
    fn a_zero_ceiling_binds_the_relay_and_the_parent_alike() {
        let Some(sandbox) = Sandbox::open() else {
            return;
        };
        let firehose = Firehose::start();
        let declared = capped_aperture("model", firehose.address, 0);
        let installed = install(&declared, sandbox.pid, None).expect("install the aperture");
        drain(
            sandbox.pid,
            SocketAddr::from((Ipv4Addr::LOCALHOST, declared.port)),
        );
        let bytes = settled(&installed);
        assert_eq!(
            bytes.to_destination + bytes.from_destination,
            0,
            "a ceiling of zero let bytes cross the relay"
        );
        assert!(
            installed.ceiling_exceeded(),
            "the parent does not see the ceiling the relay stopped at"
        );
        drop(installed);
    }

    /// A ceiling is deployment vocabulary. A request that carries one is refused by its own name,
    /// so a rejected escalation is not read as a configuration typo.
    #[test]
    fn a_ceiling_in_a_request_is_refused_by_name() {
        let mut config = HostConfig::minimum("/does/not/exist");
        config.egress_apertures = vec![aperture(
            "model",
            SocketAddr::from((Ipv4Addr::LOCALHOST, 9)),
        )];
        let published = vec![config.egress_apertures[0].fact()];
        let escalation = crate::process::ProcessRuntime::admit_egress_aperture(
            &config,
            Some(&published),
            &sandbox_request(NetworkMode::Aperture, Some("model/max=1MiB")),
        )
        .expect_err("a ceiling where a name belongs is refused");
        assert_eq!(escalation.class, DriverErrorClass::Refused);
        assert_eq!(escalation.code, "exec.aperture-ceiling-in-request");
    }

    /// The published fact answers "how much could this daemon ever pass", and answers nothing for
    /// an aperture declared without a ceiling.
    #[test]
    fn the_apertures_fact_publishes_the_declared_ceiling() {
        let declared = vec![
            capped_aperture(
                "model",
                SocketAddr::from((Ipv4Addr::LOCALHOST, 443)),
                67_108_864,
            ),
            aperture("audit", SocketAddr::from((Ipv4Addr::LOCALHOST, 8443))),
        ];
        let fact = egress_apertures_fact(&declared, true).expect("a proven mechanism publishes");
        assert_eq!(fact[0].name, "audit");
        assert_eq!(fact[0].max_bytes, None);
        assert_eq!(fact[1].name, "model");
        assert_eq!(fact[1].max_bytes, Some(67_108_864));
    }

    fn _unused(_: PathBuf) {}
}
