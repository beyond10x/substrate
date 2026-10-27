//! Destination-bound egress apertures (ADR 0013), by mechanism (c).
//!
//! The sandbox keeps `--unshare-net`. Its network namespace has loopback and no other interface, no
//! route and no resolver, exactly as it does for a run with no aperture at all. What an aperture
//! adds is **one listening socket inside that namespace**, created by substrate and connected by
//! substrate to one address pinned at declaration. The kernel floor is untouched: everything else
//! is still `ENETUNREACH`, from a raw socket as much as from a TCP one, because there is nothing to
//! route to (`docs/design/10a-egress-mechanism-spike.md` § 3.1).
//!
//! A run may hold up to `MAX_EGRESS_APERTURES` of them (design 26 § 2). Each is installed exactly
//! as one is, with its own listener, forwarder, counters and ceiling; what tells them apart inside
//! is the address: the first listens on `127.0.0.1`, the n-th on `127.0.0.n`, so several may share
//! a port. They are installed together or not at all.
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
//! ## Three traps, all silent
//!
//! 1. **The owning user namespace, not the child's.** Bubblewrap nests a *second* user namespace,
//!    so `/proc/<child>/ns/user` is not the namespace that owns `/proc/<child>/ns/net`, and joining
//!    it returns `EPERM` that reads like kernel policy when it is an addressing mistake. The owner
//!    is reachable only through `ioctl(netns_fd, NS_GET_USERNS)`, and its owner uid is the daemon's
//!    own.
//! 2. **`child-pid` from `--info-fd`, never the spawned pid.** `Child::id()` is the *bubblewrap*
//!    pid and `/proc/<that>/ns/net` is the **host** namespace. Binding there would put the aperture
//!    on host loopback, reachable by everything on the machine — and every test would still pass.
//! 3. **Wait for loopback before binding.** Bubblewrap reports `child-pid` before its concurrent
//!    child has necessarily raised `lo`. Racing that setup makes a correct bind fail with
//!    `EADDRNOTAVAIL`, or makes the first connect see `ENETUNREACH`. The helper retries only that
//!    readiness errno, with a fixed bound; it does not race bubblewrap by configuring the
//!    interface itself, and every other bind error remains an immediate refusal.
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
use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4, TcpListener};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use substrate_wire::{
    APERTURE_CA_BUNDLE_PATH, APERTURE_HOSTS_PATH, APERTURE_LOOPBACK_ADDRESS, ApertureBytes,
    ApertureMechanism, ApertureMode, AppliedAperture, EgressApertureFact, MAX_EGRESS_APERTURES,
};

use crate::DriverError;
pub use crate::aperture::EgressAperture;

/// `_IO(NSIO, 0x1)` from `linux/nsfs.h`: the user namespace that owns a namespace descriptor.
const NS_GET_USERNS: libc::c_ulong = 0xb701;
/// One-millisecond waits before unavailable loopback remains a stage-6 failure.
const LOOPBACK_READY_ATTEMPTS: usize = 1_000;
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

/// The loopback address the aperture at `index` of a run's list listens on (design 26 § 2).
///
/// The first is [`APERTURE_LOOPBACK_ADDRESS`], the address a single aperture has always listened
/// on, and the n-th is `127.0.0.n`. `None` at and past `MAX_EGRESS_APERTURES`, so no index names
/// an address no run may be given.
pub(crate) fn listen_address(index: usize) -> Option<Ipv4Addr> {
    if index >= usize::try_from(MAX_EGRESS_APERTURES).ok()? {
        return None;
    }
    let first: Ipv4Addr = APERTURE_LOOPBACK_ADDRESS.parse().ok()?;
    let [a, b, c, d] = first.octets();
    Some(Ipv4Addr::new(
        a,
        b,
        c,
        d.checked_add(u8::try_from(index).ok()?)?,
    ))
}

/// The lines every generated `/etc/hosts` opens with, before any aperture's own.
const HOSTS_HEADER: &str = "127.0.0.1\tlocalhost\n::1\tlocalhost\n";

/// The one name [`HOSTS_HEADER`] maps to an IPv4 address, and that address.
const HEADER_NAME: (&str, Ipv4Addr) = ("localhost", Ipv4Addr::LOCALHOST);

/// Whether the generated mapping would give one host name two addresses, so that every aperture
/// after the first holding it is installed and never reached by its name (design 26 § 2).
///
/// A resolver answers with the first line for a name and compares names ASCII case-insensitively,
/// with one trailing `.` ignored, so they are compared that way here. `localhost` is already
/// mapped by [`HOSTS_HEADER`] to the first listen address, so only the first aperture may declare
/// it. A host declared as a literal address gets no line and shadows nothing. A set longer than
/// `MAX_EGRESS_APERTURES` is not this rule's to refuse.
pub(crate) fn a_host_is_mapped_twice<'a>(
    apertures: impl IntoIterator<Item = &'a EgressAperture>,
) -> bool {
    let mut mapped = vec![(HEADER_NAME.0.to_owned(), HEADER_NAME.1)];
    for (index, aperture) in apertures.into_iter().enumerate() {
        let Some(address) = listen_address(index) else {
            return false;
        };
        if !aperture.host_is_a_name() {
            continue;
        }
        let host = aperture.host.strip_suffix('.').unwrap_or(&aperture.host);
        let name = host.to_ascii_lowercase();
        match mapped.iter().find(|(held, _)| *held == name) {
            Some((_, held)) if *held != address => return true,
            Some(_) => {}
            None => mapped.push((name, address)),
        }
    }
    false
}

/// One listen address per aperture, in list order, or the refusal for a set no run may hold.
///
/// Admission bounds the list before it gets here. The bound is held again where it is spent, so
/// that neither half of the mechanism can build a set nobody admitted: an empty one would install
/// nothing and still report an aperture run, and a fifth would listen on an address no generated
/// mapping names.
fn listen_addresses(count: usize) -> Result<Vec<Ipv4Addr>, DriverError> {
    let addresses = (0..count).map(listen_address).collect::<Option<Vec<_>>>();
    match addresses {
        Some(addresses) if !addresses.is_empty() => Ok(addresses),
        _ => Err(DriverError::failed(
            "exec.aperture-install-failed",
            format!(
                "The egress apertures could not be installed exactly as declared: a run holds between 1 and {MAX_EGRESS_APERTURES} apertures, and this one named {count}."
            ),
        )),
    }
}

/// What a run with apertures gets bound into a sandbox that otherwise has no `/etc` at all.
///
/// Two files, generated per run and read-only. `hosts` maps each declared name to the address its
/// own aperture listens on ([`listen_address`]), so a child uses the URL the operator declared and
/// that aperture's forwarder is what answers; nothing else resolves, because nothing else is in the
/// file and there is no resolver behind it. A host declared as a literal address gets no line, and
/// keeps its place in the list, so the apertures after it keep theirs. The CA bundle is a
/// **snapshot**, not a bind of the live host path: TLS is byte-transparent through the forwarder
/// but unverifiable without a trust anchor, and a mid-run rotation must not change what a running
/// child already trusts (`docs/design/10a-egress-mechanism-spike.md` § 6 row 4).
pub(crate) struct GeneratedResolution {
    directory: tempfile::TempDir,
    hosts: PathBuf,
    ca_bundle: Option<PathBuf>,
}

impl GeneratedResolution {
    /// Generates the resolution for `apertures`, in the order the run listed them.
    ///
    /// # Errors
    ///
    /// `exec.aperture-install-failed` for an empty list, one longer than `MAX_EGRESS_APERTURES` or
    /// one whose mapping would name a host twice ([`a_host_is_mapped_twice`]), before anything is
    /// created, and for a file that cannot be written.
    pub(crate) fn prepare(
        apertures: &[EgressAperture],
        ca_source: Option<&Path>,
        root: &Path,
    ) -> Result<Self, DriverError> {
        let addresses = listen_addresses(apertures.len())?;
        // Admission refuses this by name; held again where the mapping is written, so no set
        // nobody admitted gets a mapping whose later lines are never read.
        if a_host_is_mapped_twice(apertures) {
            return Err(DriverError::failed(
                "exec.aperture-install-failed",
                "The egress apertures could not be installed exactly as declared: the generated host mapping would name one host for two apertures.",
            ));
        }
        let directory = tempfile::Builder::new()
            .prefix("aperture-")
            .tempdir_in(root)
            .map_err(|error| install_failed("private aperture directory", &error))?;
        let hosts = directory.path().join("hosts");
        let mut text = String::from(HOSTS_HEADER);
        for (aperture, address) in apertures.iter().zip(addresses) {
            if aperture.host_is_a_name() {
                text.push_str(&address.to_string());
                text.push('\t');
                text.push_str(&aperture.host);
                text.push('\n');
            }
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
    /// Kept so that teardown can end the listening socket itself, not only this process's
    /// descriptor for it: see [`Listener`].
    _listener: Listener,
    counters: SharedCounters,
    name: String,
    destination: String,
    max_bytes: Option<u64>,
    listen: SocketAddrV4,
}

impl InstalledAperture {
    /// The `address:port` this aperture listens on inside the run's namespace.
    pub(crate) fn listen(&self) -> SocketAddrV4 {
        self.listen
    }

    /// What was installed, with the bytes that have crossed so far and the address it listens on.
    ///
    /// The listen address is always stated here; a run that named one `aperture` reports it
    /// without, so that record is unchanged (design 26 § 2), and that is the caller's to drop.
    pub(crate) fn applied(&self) -> AppliedAperture {
        AppliedAperture {
            mode: ApertureMode::Aperture,
            name: self.name.clone(),
            destination: self.destination.clone(),
            mechanism: ApertureMechanism::LoopbackForwarder,
            bytes: self.counters.read(),
            max_bytes: self.max_bytes,
            listen: Some(self.listen().to_string()),
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

/// The listening socket an aperture's helper made inside the run's namespace, as the installing
/// process holds it.
///
/// Dropping it shuts the socket down before closing this descriptor. A socket stops listening when
/// its *last* descriptor closes, and this process does not hold the last one: any child another
/// thread forks while the descriptor is open holds a copy until that child closes its inherited
/// table or execs, however briefly. Killing and reaping the forwarder therefore did not by itself
/// stop the listener answering inside; on a loaded host a copy outlived both, and a connection from
/// inside was accepted by a set that had been refused. `shutdown` acts on the socket every copy
/// shares, so from that point no copy accepts and a connection inside is refused.
struct Listener(OwnedFd);

impl Drop for Listener {
    fn drop(&mut self) {
        // SAFETY: `shutdown` on a descriptor this value owns; its result is not needed, because
        // the descriptor is closed right after whether or not the socket was still listening.
        unsafe {
            libc::shutdown(self.0.as_raw_fd(), libc::SHUT_RDWR);
        }
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

/// Every aperture of one run, installed together and alive exactly as long as the run.
///
/// There is no partial value of this type: [`install_all`] returns one only when every aperture in
/// the list is installed. Dropping it drops each [`InstalledAperture`], which kills and reaps that
/// aperture's forwarder.
pub(crate) struct InstalledApertures {
    installed: Vec<InstalledAperture>,
}

impl InstalledApertures {
    /// The installed apertures, in the order the run listed them, which is the order of their
    /// listen addresses.
    pub(crate) fn apertures(&self) -> &[InstalledAperture] {
        &self.installed
    }

    /// Whether any one aperture has reached its own ceiling, which ends the run.
    ///
    /// Each aperture is held to its own declared ceiling over its own counters (ADR 0014; design
    /// 26 § 2): bytes through one never count against another, and none is exceeded because a
    /// neighbour was.
    pub(crate) fn ceiling_exceeded(&self) -> bool {
        self.installed
            .iter()
            .any(InstalledAperture::ceiling_exceeded)
    }
}

/// Installs every aperture of a run into the namespace of a *bubblewrap-reported* child, the
/// aperture at index `n` listening on [`listen_address`]`(n)` at its declared port.
///
/// `sandbox_pid` must be the `child-pid` bubblewrap wrote to its `--info-fd`, never the pid of the
/// spawned bubblewrap process. `cgroup_procs` is the run's `cgroup.procs`; every forked process
/// joins it before it touches a socket.
///
/// # Errors
///
/// Every failure is `exec.aperture-install-failed` with nothing partial left behind: an aperture
/// that cannot be installed exactly as declared refuses the dispatch, and so does a list that is
/// empty or longer than `MAX_EGRESS_APERTURES` (ADR 0013). When the n-th install fails, every
/// forwarder the ones before it started is killed and reaped before this returns.
pub(crate) fn install_all(
    apertures: &[EgressAperture],
    sandbox_pid: u32,
    cgroup_procs: Option<&Path>,
) -> Result<InstalledApertures, DriverError> {
    let addresses = listen_addresses(apertures.len())?;
    let mut installed = Vec::with_capacity(apertures.len());
    for (aperture, address) in apertures.iter().zip(addresses) {
        let listen = SocketAddrV4::new(address, aperture.port);
        // All or nothing: the `?` drops `installed`, and each value in it kills and reaps its own
        // forwarder on the way out.
        installed.push(install_one(aperture, listen, sandbox_pid, cgroup_procs)?);
    }
    Ok(InstalledApertures { installed })
}

/// Installs one aperture on `listen`: its listener inside, then its own counters and forwarder.
fn install_one(
    aperture: &EgressAperture,
    listen: SocketAddrV4,
    sandbox_pid: u32,
    cgroup_procs: Option<&Path>,
) -> Result<InstalledAperture, DriverError> {
    // A `Listener` from here on, so a failure below shuts the socket down rather than only closing
    // this descriptor for it.
    let listener = Listener(listen_inside(sandbox_pid, listen, cgroup_procs)?);
    let counters =
        SharedCounters::new().map_err(|error| install_failed("aperture byte counters", &error))?;
    let cgroup = open_cgroup(cgroup_procs)?;
    let forwarder = spawn_forwarder(
        &listener.0,
        aperture.pinned,
        &counters,
        cgroup.as_ref(),
        aperture.max_bytes,
    )?;
    Ok(InstalledAperture {
        forwarder,
        _listener: listener,
        counters,
        name: aperture.name.clone(),
        destination: aperture.pinned.to_string(),
        max_bytes: aperture.max_bytes,
        listen,
    })
}

/// Creates the listening socket **inside** the sandbox's network namespace and hands it back.
///
/// The helper does not also verify the aperture: at this point nothing is accepting on it yet, so a
/// connect from inside would sit in the backlog and the read behind it would never return. What
/// checks the aperture end to end is [`reaches_from_inside`], after the forwarder exists.
///
/// `listen` is the loopback `address:port` inside the namespace; the `sockaddr_in` the helper binds
/// is built from it here, before the fork, like every other value the helper reads.
fn listen_inside(
    sandbox_pid: u32,
    listen: SocketAddrV4,
    cgroup_procs: Option<&Path>,
) -> Result<OwnedFd, DriverError> {
    let address = sockaddr_in(*listen.ip(), listen.port());
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

    let (parent_end, child_end) =
        handback_pair().map_err(|error| install_failed("aperture handback socket", &error))?;

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
            libc::_exit(helper_body(
                cgroup_fd, owner_fd, netns_fd, &address, child_fd,
            ));
        }
    }
    drop(child_end);
    let handback = receive_descriptor(parent_fd);
    let mut status = 0;
    loop {
        // SAFETY: reaping a child this process just forked.
        let waited = unsafe { libc::waitpid(pid, &raw mut status, 0) };
        if waited == pid {
            break;
        }
        let error = io::Error::last_os_error();
        if waited < 0 && error.raw_os_error() == Some(libc::EINTR) {
            continue;
        }
        return Err(install_failed("aperture helper reaping", &error));
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
/// Must be called only in a freshly forked child, with `address` built before the fork. Every call
/// below is async-signal-safe and none allocates.
unsafe fn helper_body(
    cgroup_fd: RawFd,
    owner_fd: RawFd,
    netns_fd: RawFd,
    address: &libc::sockaddr_in,
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
        if !bind_when_loopback_is_ready(listener, address) {
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

/// Binds once bubblewrap's concurrent child has raised loopback, without configuring it here.
///
/// A loopback address can become bindable just before the interface is `IFF_UP`; binding at that
/// instant succeeds but the first connection still gets `ENETUNREACH`. Reading the flags first
/// closes that half of the race. `EADDRNOTAVAIL` remains retryable inside the same fixed budget for
/// the address-publication edge; every other bind errno returns immediately. `poll` with no
/// descriptors is an allocation-free, async-signal-safe one-millisecond wait that yields to the
/// bubblewrap child instead of competing with it for the interface configuration.
///
/// # Safety
///
/// `socket` must be a live, unbound socket in the freshly joined sandbox network namespace.
/// Post-fork child only; the function calls libc over stack memory and allocates nothing.
unsafe fn bind_when_loopback_is_ready(socket: RawFd, address: &libc::sockaddr_in) -> bool {
    unsafe {
        let mut request: libc::ifreq = std::mem::zeroed();
        request.ifr_name[0] = libc::c_char::from_ne_bytes(*b"l");
        request.ifr_name[1] = libc::c_char::from_ne_bytes(*b"o");
        let up = libc::c_short::try_from(libc::IFF_UP).unwrap_or(1);
        for attempt in 0..LOOPBACK_READY_ATTEMPTS {
            if libc::ioctl(socket, libc::SIOCGIFFLAGS, &raw mut request) != 0 {
                if *libc::__errno_location() == libc::EINTR {
                    continue;
                }
                return false;
            }
            if request.ifr_ifru.ifru_flags & up != 0
                && libc::bind(
                    socket,
                    std::ptr::from_ref(address).cast(),
                    u32::try_from(std::mem::size_of::<libc::sockaddr_in>()).unwrap_or(16),
                ) == 0
            {
                return true;
            }
            if request.ifr_ifru.ifru_flags & up != 0
                && *libc::__errno_location() != libc::EADDRNOTAVAIL
            {
                return false;
            }
            if attempt + 1 < LOOPBACK_READY_ATTEMPTS {
                libc::poll(std::ptr::null_mut(), 0, 1);
            }
        }
        *libc::__errno_location() = libc::EADDRNOTAVAIL;
        false
    }
}

/// Reports the failing stage and the thread-local errno without allocation in the post-fork child.
unsafe fn helper_failure(handback_fd: RawFd, stage: i32) -> i32 {
    unsafe {
        let errno = *libc::__errno_location();
        let record = [stage, errno];
        loop {
            let sent = libc::send(
                handback_fd,
                record.as_ptr().cast(),
                std::mem::size_of_val(&record),
                libc::MSG_NOSIGNAL,
            );
            if sent == isize::try_from(std::mem::size_of_val(&record)).unwrap_or(8) {
                break;
            }
            if sent < 0 && *libc::__errno_location() == libc::EINTR {
                continue;
            }
            break;
        }
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

#[cfg(test)]
thread_local! {
    /// Every forwarder this test's thread forked, so a case can prove a refused install left none
    /// of them alive without scanning a process table other cases share.
    static SPAWNED_FORWARDERS: std::cell::RefCell<Vec<libc::pid_t>> =
        const { std::cell::RefCell::new(Vec::new()) };
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
    #[cfg(test)]
    SPAWNED_FORWARDERS.with_borrow_mut(|spawned| spawned.push(pid));
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
/// "open a sandbox" that never opened. This throwaway sandbox places no secret slot, so `pipe2`
/// with no flags is enough here; the exec spawn instead creates its barrier and info pipes
/// close-on-exec above the slot range and clears the flag in its own `pre_exec`
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
        .args(crate::process::USER_NAMESPACE_ARGV)
        .args([
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
            let listen = SocketAddrV4::new(Ipv4Addr::LOCALHOST, destination.port());
            let listener = Listener(listen_inside(sandbox_pid, listen, None).ok()?);
            // The probe proves the *mechanism*, never a declaration: no ceiling, because there is
            // no operator statement here to enforce.
            let forwarder =
                spawn_forwarder(&listener.0, destination, &counters, None, None).ok()?;
            Some(InstalledAperture {
                forwarder,
                _listener: listener,
                counters,
                name: String::new(),
                destination: destination.to_string(),
                max_bytes: None,
                listen,
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

/// Reads bubblewrap's whole `--info-fd` JSON object and takes `child-pid` — trap 2 in one place.
///
/// The whole object, not just the field: bubblewrap writes it in separate `write` calls
/// (`child-pid`, then one per namespace id, then the closing brace) and only then lets its child
/// run. A caller that drops the read end after the first write makes the next one raise `SIGPIPE`
/// in bubblewrap, which dies with the child unstarted and `lo` never raised, so the install fails
/// `EADDRNOTAVAIL` and the run is refused. EOF is still not awaited: the object's closing brace is
/// the end, because other forks may hold a copy of the write end.
pub(crate) fn read_sandbox_pid<R: std::io::Read>(info: &mut R) -> Option<u32> {
    let mut buffer = Vec::new();
    let mut chunk = [0_u8; 512];
    loop {
        if buffer.trim_ascii_end().ends_with(b"}") {
            return sandbox_pid_from(&buffer);
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

/// The `child-pid` field of bytes read from `--info-fd`.
///
/// Bubblewrap writes the whole object before it releases `--block-fd`, but a copy of the
/// descriptor may stay open, so waiting for EOF would deadlock against the barrier this exists to
/// run inside; [`read_sandbox_pid`] stops at the object's closing brace instead.
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
        libc::sendmsg(socket, &raw const message, libc::MSG_NOSIGNAL) == 1
    }
}

/// Receives one descriptor sent as `SCM_RIGHTS`, or nothing if the sender failed before sending.
enum HelperHandback {
    Descriptor(OwnedFd),
    Failure { stage: i32, errno: i32 },
    Missing,
}

fn handback_pair() -> io::Result<(OwnedFd, OwnedFd)> {
    let mut sockets = [0_i32; 2];
    // SAFETY: `sockets` is writable storage for exactly two descriptors.
    if unsafe {
        libc::socketpair(
            libc::AF_UNIX,
            libc::SOCK_SEQPACKET | libc::SOCK_CLOEXEC,
            0,
            sockets.as_mut_ptr(),
        )
    } != 0
    {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: both descriptors were just created by `socketpair` and are owned here.
    Ok(unsafe {
        (
            OwnedFd::from_raw_fd(sockets[0]),
            OwnedFd::from_raw_fd(sockets[1]),
        )
    })
}

fn receive_descriptor(socket: RawFd) -> HelperHandback {
    // SAFETY: a `recvmsg` into stack buffers of the sizes its own macros computed, followed by a
    // bounds-checked read of exactly the descriptor the kernel placed in the control message.
    unsafe {
        loop {
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
            let received = libc::recvmsg(socket, &raw mut message, libc::MSG_CMSG_CLOEXEC);
            if received < 0 && *libc::__errno_location() == libc::EINTR {
                continue;
            }
            if message.msg_flags & (libc::MSG_TRUNC | libc::MSG_CTRUNC) != 0 {
                return HelperHandback::Missing;
            }
            let header = libc::CMSG_FIRSTHDR(&raw const message);
            if received == 8 && header.is_null() {
                let stage = i32::from_ne_bytes(payload[..4].try_into().unwrap_or_default());
                let errno = i32::from_ne_bytes(payload[4..].try_into().unwrap_or_default());
                return HelperHandback::Failure { stage, errno };
            }
            if received != 1
                || header.is_null()
                || (*header).cmsg_level != libc::SOL_SOCKET
                || (*header).cmsg_type != libc::SCM_RIGHTS
                || (*header).cmsg_len != libc::CMSG_LEN(4) as usize
                || !libc::CMSG_NXTHDR(&raw const message, header).is_null()
            {
                return HelperHandback::Missing;
            }
            let descriptor = std::ptr::read_unaligned(libc::CMSG_DATA(header).cast::<RawFd>());
            return if descriptor >= 0 {
                HelperHandback::Descriptor(OwnedFd::from_raw_fd(descriptor))
            } else {
                HelperHandback::Missing
            };
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
    use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpListener, TcpStream};
    use std::os::fd::{AsRawFd as _, RawFd};
    use std::path::{Path, PathBuf};
    use std::process::{Child, Command, Stdio};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};

    use substrate_wire::{
        APERTURE_LOOPBACK_ADDRESS, ApertureMechanism, ApertureMode, ConfinementRequest,
        MAX_EGRESS_APERTURES, NetworkMode, SandboxProfile,
    };

    use super::{
        EgressAperture, GeneratedResolution, HelperHandback, InstalledAperture,
        egress_apertures_fact, handback_pair, install_all, listen_address, listen_inside,
        read_sandbox_pid, receive_descriptor, sandbox_pid_from,
    };
    use crate::{DriverError, DriverErrorClass, HostConfig};

    const BUBBLEWRAP: &str = "/usr/bin/bwrap";
    /// What the model-free fake app-server answers with. Not a model, not a protocol: bytes.
    const APP_SERVER_BODY: &[u8] = b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok";
    /// Two more bodies, one per destination, so a reply says which destination sent it.
    const MODEL_BODY: &[u8] = b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nmodel";
    const AUDIT_BODY: &[u8] = b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\naudit";

    /// One aperture through the production entry point, exactly as a single-aperture run
    /// installs it: a set of one, listening on the first loopback address.
    fn install(
        aperture: &EgressAperture,
        sandbox_pid: u32,
        cgroup_procs: Option<&Path>,
    ) -> Result<InstalledAperture, DriverError> {
        let mut set = install_all(std::slice::from_ref(aperture), sandbox_pid, cgroup_procs)?;
        assert_eq!(
            set.installed.len(),
            1,
            "one aperture installed one forwarder"
        );
        Ok(set.installed.pop().expect("the one installed aperture"))
    }

    /// Every forwarder pid this thread has spawned since the last call.
    fn take_spawned_forwarders() -> Vec<libc::pid_t> {
        super::SPAWNED_FORWARDERS.with_borrow_mut(std::mem::take)
    }

    /// Whether `pid` is gone: killed **and** reaped. A killed child nobody waited for is a zombie,
    /// and signal 0 still finds a zombie, so `ESRCH` is the only answer that rules out both.
    fn reaped(pid: libc::pid_t) -> bool {
        // SAFETY: signal 0 delivers nothing; it only asks whether the pid exists.
        let found = unsafe { libc::kill(pid, 0) } == 0;
        !found && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
    }
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
    fn helper_handback_preserves_record_boundaries() {
        let (receiver, _sender) = handback_pair().expect("create the production handback");
        let mut socket_type = 0_i32;
        let mut length = u32::try_from(std::mem::size_of_val(&socket_type)).unwrap_or(4);
        // SAFETY: the output pointer and its length describe `socket_type` exactly.
        assert_eq!(
            unsafe {
                libc::getsockopt(
                    receiver.as_raw_fd(),
                    libc::SOL_SOCKET,
                    libc::SO_TYPE,
                    (&raw mut socket_type).cast(),
                    &raw mut length,
                )
            },
            0
        );
        assert_eq!(
            socket_type,
            libc::SOCK_SEQPACKET,
            "the stage+errno record can be split on a stream socket"
        );
    }

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

    /// The throwaway sandbox's argv, built and returned rather than built and spawned.
    ///
    /// Split out so this argv — the eighth and last bubblewrap command line in the crate — is
    /// asserted like the other seven. It is the one that cannot be pointed at the recording
    /// backend, because `Sandbox::open` needs a real `--info-fd` child pid to install an aperture
    /// into; reading what it built needs no child at all.
    fn sandbox_command(info_fd: RawFd, block_fd: RawFd) -> Command {
        let mut command = Command::new(BUBBLEWRAP);
        command
            .env_clear()
            .args(crate::process::USER_NAMESPACE_ARGV)
            .args([
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
        command
    }

    /// The eighth argv carries the posture too — asserted as built, like the other seven.
    ///
    /// A throwaway sandbox in a test is still a sandbox: these cases put a forwarder in its
    /// namespaces and a process in its cgroup, so a child in it that could nest a user namespace
    /// is the same defect one place further from production. Reading the argv needs no backend on
    /// disk, so this holds on every host.
    #[test]
    fn the_throwaway_sandbox_these_cases_open_carries_the_user_namespace_posture() {
        let command = sandbox_command(3, 4);
        let argv: Vec<String> = command
            .get_args()
            .map(|part| part.to_string_lossy().into_owned())
            .collect();
        for option in crate::process::USER_NAMESPACE_ARGV {
            assert!(
                argv.iter().any(|part| part == option),
                "the throwaway sandbox does not carry {option}: {argv:?}"
            );
        }
    }

    impl Sandbox {
        fn open() -> Option<Self> {
            if !Path::new(BUBBLEWRAP).is_file() {
                return None;
            }
            let (mut info_read, info_write) = super::inheritable_pipe().expect("info pipe");
            let (block_read, block_write) = super::inheritable_pipe().expect("block pipe");
            let mut command = sandbox_command(info_write.as_raw_fd(), block_read.as_raw_fd());
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

    #[test]
    fn a_forced_bind_failure_names_its_stage_and_errno() {
        let Some(sandbox) = Sandbox::open() else {
            return;
        };
        let port = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .expect("reserve a host-side port number")
            .local_addr()
            .expect("read the reserved port")
            .port();
        let tuple = SocketAddrV4::new(Ipv4Addr::LOCALHOST, port);
        let _first = listen_inside(sandbox.pid, tuple, None).expect("bind the first listener");
        let failure = listen_inside(sandbox.pid, tuple, None)
            .expect_err("a second listener on the same sandbox tuple must fail");
        assert_eq!(failure.class, DriverErrorClass::Failed);
        assert_eq!(failure.code, "exec.aperture-install-failed");
        assert!(
            failure.message.contains("stage 6"),
            "the bind stage was lost: {}",
            failure.message
        );
        assert!(
            failure
                .message
                .contains(&format!("errno {}", libc::EADDRINUSE)),
            "the bind errno was lost: {}",
            failure.message
        );
        assert!(
            failure
                .message
                .contains(&std::io::Error::from_raw_os_error(libc::EADDRINUSE).to_string()),
            "the bind errno was not decoded: {}",
            failure.message
        );
    }

    /// A model-free app-server: one listener, one fixed body, no protocol opinion.
    struct AppServer {
        address: SocketAddr,
        stop: Arc<AtomicBool>,
        handle: Option<std::thread::JoinHandle<()>>,
    }

    impl AppServer {
        fn start() -> Self {
            Self::serving(APP_SERVER_BODY)
        }

        /// The same server answering with `body`, so two of them can be told apart by their reply.
        fn serving(body: &'static [u8]) -> Self {
            let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind app server");
            let address = listener.local_addr().expect("app server address");
            let stop = Arc::new(AtomicBool::new(false));
            let child_stop = Arc::clone(&stop);
            let handle = std::thread::spawn(move || {
                while !child_stop.load(AtomicOrdering::Acquire) {
                    let Ok((mut stream, _)) = listener.accept() else {
                        break;
                    };
                    if child_stop.load(AtomicOrdering::Acquire) {
                        break;
                    }
                    if stream.write_all(body).is_err() {
                        break;
                    }
                    let _ = stream.flush();
                }
            });
            Self {
                address,
                stop,
                handle: Some(handle),
            }
        }
    }

    impl Drop for AppServer {
        fn drop(&mut self) {
            self.stop.store(true, AtomicOrdering::Release);
            let _ = TcpStream::connect(self.address);
            if let Some(handle) = self.handle.take() {
                let _ = handle.join();
            }
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
        /// Connected and read bytes, but not the ones expected: some other destination answered.
        Mismatched,
        Other(i32),
    }

    /// Connects to `address` from inside the sandbox's network namespace.
    ///
    /// The confined child and this probe are in the same namespace, so what the kernel refuses one
    /// it refuses the other. A forked, single-threaded process, because `setns(CLONE_NEWUSER)`
    /// refuses a threaded caller.
    fn reach(sandbox_pid: u32, address: SocketAddr) -> Reach {
        reach_expecting(sandbox_pid, address, APP_SERVER_BODY)
    }

    /// [`reach`], served only when the reply is exactly `expected`.
    fn reach_expecting(sandbox_pid: u32, address: SocketAddr, expected: &[u8]) -> Reach {
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
                let code = reach_body(owner, netns_fd, &target, expected);
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
            3 => Reach::Mismatched,
            other => Reach::Other(other),
        }
    }

    /// # Safety
    ///
    /// Post-fork child only.
    unsafe fn reach_body(
        owner: i32,
        netns: i32,
        target: &libc::sockaddr_in,
        expected: &[u8],
    ) -> i32 {
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
            if buffer[..filled] == *expected { 0 } else { 3 }
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

    /// A declaration with its own host and its own listen port, as an operator writes one.
    fn named_aperture(name: &str, host: &str, port: u16, pinned: SocketAddr) -> EgressAperture {
        EgressAperture {
            host: host.to_owned(),
            port,
            ..aperture(name, pinned)
        }
    }

    fn sandbox_request(network: NetworkMode, name: Option<&str>) -> ConfinementRequest {
        ConfinementRequest {
            capability_snapshot: "sha256:".to_owned() + &"0".repeat(64),
            network,
            aperture: name.map(str::to_owned),
            apertures: Vec::new(),
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
            .is_empty(),
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
        .expect("a declared, verified aperture is admissible");
        let names: Vec<&str> = selected.iter().map(|one| one.name.as_str()).collect();
        assert_eq!(names, ["model"], "a name selects exactly its aperture");

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
        let declared = std::slice::from_ref(&declared);
        let generated = GeneratedResolution::prepare(declared, None, root.path())
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
        let with_anchor = GeneratedResolution::prepare(declared, Some(&anchor), root.path())
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

    // ---------------------------------------------------------------------------------------
    // Several apertures per run (design 26 § 2)
    // ---------------------------------------------------------------------------------------

    /// The first listed aperture on `APERTURE_LOOPBACK_ADDRESS`, the n-th on `127.0.0.n`, and no
    /// address at all past `MAX_EGRESS_APERTURES`.
    #[test]
    fn each_aperture_listens_on_its_own_loopback_address() {
        assert_eq!(
            listen_address(0),
            Some(
                APERTURE_LOOPBACK_ADDRESS
                    .parse::<Ipv4Addr>()
                    .expect("a loopback address")
            ),
            "the first aperture moved off the address every 0.20.0 run listens on"
        );
        let maximum = usize::try_from(MAX_EGRESS_APERTURES).expect("a small bound");
        let addresses: Vec<Option<Ipv4Addr>> = (0..=maximum).map(listen_address).collect();
        assert_eq!(
            addresses,
            [
                Some(Ipv4Addr::LOCALHOST),
                Some(Ipv4Addr::new(127, 0, 0, 2)),
                Some(Ipv4Addr::new(127, 0, 0, 3)),
                Some(Ipv4Addr::new(127, 0, 0, 4)),
                None,
            ]
        );
    }

    /// One aperture: the file a `0.20.0` run was given, byte for byte.
    #[test]
    fn one_aperture_keeps_its_generated_hosts_file_byte_for_byte() {
        let root = tempfile::tempdir().unwrap();
        let declared = aperture("model", SocketAddr::from((Ipv4Addr::LOCALHOST, 443)));
        let generated =
            GeneratedResolution::prepare(std::slice::from_ref(&declared), None, root.path())
                .expect("generate the per-run resolution");
        assert_eq!(
            std::fs::read_to_string(&generated.hosts).unwrap(),
            "127.0.0.1\tlocalhost\n::1\tlocalhost\n127.0.0.1\tapp.example.invalid\n"
        );
        assert!(generated.close());
    }

    /// Several apertures: each declared name maps to its own listen address, in listed order, and
    /// a literal-address host keeps its index's address without being given a name.
    #[test]
    fn several_apertures_map_each_name_to_its_own_address() {
        let root = tempfile::tempdir().unwrap();
        let pinned = SocketAddr::from((Ipv4Addr::LOCALHOST, 9));
        let two = [
            named_aperture("model", "model.example.invalid", 443, pinned),
            named_aperture("audit", "audit.example.invalid", 443, pinned),
        ];
        let generated = GeneratedResolution::prepare(&two, None, root.path())
            .expect("generate the resolution for two apertures");
        assert_eq!(
            std::fs::read_to_string(&generated.hosts).unwrap(),
            "127.0.0.1\tlocalhost\n::1\tlocalhost\n\
             127.0.0.1\tmodel.example.invalid\n\
             127.0.0.2\taudit.example.invalid\n"
        );
        assert!(generated.close());

        let with_literal = [
            named_aperture("model", "model.example.invalid", 443, pinned),
            named_aperture("bare", "203.0.113.9", 443, pinned),
            named_aperture("audit", "audit.example.invalid", 443, pinned),
        ];
        let generated = GeneratedResolution::prepare(&with_literal, None, root.path())
            .expect("generate the resolution for three apertures");
        assert_eq!(
            std::fs::read_to_string(&generated.hosts).unwrap(),
            "127.0.0.1\tlocalhost\n::1\tlocalhost\n\
             127.0.0.1\tmodel.example.invalid\n\
             127.0.0.3\taudit.example.invalid\n",
            "a literal-address host was given a name, or shifted the address of the next one"
        );
        assert!(generated.close());
    }

    /// No apertures, or more than the maximum, is not a set any run can be given, and neither
    /// half of the mechanism builds one: nothing is generated and nothing is forked.
    #[test]
    fn an_empty_or_oversized_aperture_set_is_refused_before_anything_exists() {
        let root = tempfile::tempdir().unwrap();
        let pinned = SocketAddr::from((Ipv4Addr::LOCALHOST, 9));
        let maximum = usize::try_from(MAX_EGRESS_APERTURES).expect("a small bound");
        let oversized: Vec<EgressAperture> = (0..=maximum)
            .map(|index| {
                named_aperture(
                    &format!("a{index}"),
                    &format!("a{index}.example.invalid"),
                    443,
                    pinned,
                )
            })
            .collect();
        let _ = take_spawned_forwarders();
        for (set, count) in [(&[][..], 0), (&oversized[..], maximum + 1)] {
            let Err(refusal) = GeneratedResolution::prepare(set, None, root.path()) else {
                panic!("a resolution was generated for {count} apertures");
            };
            assert_eq!(refusal.code, "exec.aperture-install-failed");
            assert!(
                refusal.message.contains(&format!("named {count}")),
                "{}",
                refusal.message
            );
            // A pid that names no process: a refusal that reached the namespace would say so.
            let Err(refusal) = install_all(set, u32::MAX, None) else {
                panic!("{count} apertures were installed");
            };
            assert_eq!(refusal.code, "exec.aperture-install-failed");
            assert!(
                refusal.message.contains(&format!("named {count}")),
                "{}",
                refusal.message
            );
        }
        assert_eq!(
            std::fs::read_dir(root.path()).unwrap().count(),
            0,
            "a refused set left generated state behind"
        );
        assert_eq!(
            take_spawned_forwarders(),
            Vec::<libc::pid_t>::new(),
            "a refused set forked a forwarder"
        );
    }

    /// A name the generated mapping would hold twice reaches only its first address, so the set is
    /// not generated: admission refuses it by name (`exec.aperture-host-listed-twice`), and the
    /// mechanism holds the rule again where the mapping is written. Names compare as a resolver
    /// compares them, ignoring ASCII case and one trailing dot; `localhost` is already mapped to
    /// the first listen address; a literal address gets no line and shadows nothing.
    #[test]
    fn a_name_the_mapping_would_hold_twice_is_never_generated() {
        let root = tempfile::tempdir().unwrap();
        let pinned = SocketAddr::from((Ipv4Addr::LOCALHOST, 9));
        let model = named_aperture("model", "model.example.invalid", 443, pinned);
        let mirror = named_aperture("mirror", "MODEL.example.invalid.", 443, pinned);
        let local = named_aperture("local", "localhost", 443, pinned);
        let literal = named_aperture("literal", "192.0.2.1", 443, pinned);
        for set in [
            [model.clone(), mirror.clone()],
            [model.clone(), local.clone()],
        ] {
            let Err(refusal) = GeneratedResolution::prepare(&set, None, root.path()) else {
                panic!("a mapping naming one host twice was generated");
            };
            assert_eq!(refusal.code, "exec.aperture-install-failed");
        }
        assert_eq!(
            std::fs::read_dir(root.path()).unwrap().count(),
            0,
            "a refused set left generated state behind"
        );

        let generated = GeneratedResolution::prepare(&[local, model], None, root.path())
            .expect("localhost first is the address the mapping already gives it");
        assert_eq!(
            std::fs::read_to_string(&generated.hosts).unwrap(),
            "127.0.0.1\tlocalhost\n::1\tlocalhost\n\
             127.0.0.1\tlocalhost\n\
             127.0.0.2\tmodel.example.invalid\n"
        );
        assert!(generated.close());
        let generated =
            GeneratedResolution::prepare(&[literal.clone(), literal, mirror], None, root.path())
                .expect("a repeated literal address shadows no name");
        assert!(generated.close());
    }

    /// Two apertures on the same port: each reached from inside the sandbox on its own address,
    /// each served by its own destination, each counted on its own page.
    ///
    /// Port 443 on both is the point: they share a port because they do not share an address. It
    /// also proves the second address binds in a fresh namespace whose loopback bubblewrap has just
    /// raised — `127.0.0.2` is only local because `lo` carries `127.0.0.0/8`.
    #[test]
    fn two_apertures_on_one_port_each_reach_their_own_destination() {
        let Some(sandbox) = Sandbox::open() else {
            return;
        };
        let model = AppServer::serving(MODEL_BODY);
        let audit = AppServer::serving(AUDIT_BODY);
        let declared = [
            named_aperture("model", "model.example.invalid", 443, model.address),
            named_aperture("audit", "audit.example.invalid", 443, audit.address),
        ];
        let installed = install_all(&declared, sandbox.pid, None).expect("install both apertures");
        let listening: Vec<SocketAddrV4> = installed
            .apertures()
            .iter()
            .map(InstalledAperture::listen)
            .collect();
        let first = SocketAddrV4::new(Ipv4Addr::LOCALHOST, 443);
        let second = SocketAddrV4::new(Ipv4Addr::new(127, 0, 0, 2), 443);
        assert_eq!(listening, [first, second]);
        assert_eq!(
            reach_expecting(sandbox.pid, SocketAddr::V4(first), MODEL_BODY),
            Reach::Served,
            "the first aperture did not serve its own destination"
        );
        assert_eq!(
            reach_expecting(sandbox.pid, SocketAddr::V4(second), AUDIT_BODY),
            Reach::Served,
            "the second aperture did not serve its own destination"
        );
        let [model_installed, audit_installed] = installed.apertures() else {
            panic!(
                "two apertures installed {} forwarders",
                installed.apertures().len()
            );
        };
        let model_applied = wait_for_bytes(model_installed);
        let audit_applied = wait_for_bytes(audit_installed);
        assert_eq!(model_applied.name, "model");
        assert_eq!(model_applied.destination, model.address.to_string());
        assert_eq!(
            model_applied.bytes.from_destination,
            MODEL_BODY.len() as u64,
            "the first aperture counted bytes that were not its destination's"
        );
        assert_eq!(audit_applied.name, "audit");
        assert_eq!(audit_applied.destination, audit.address.to_string());
        assert_eq!(
            audit_applied.bytes.from_destination,
            AUDIT_BODY.len() as u64,
            "the second aperture counted bytes that were not its destination's"
        );
        drop(installed);
    }

    /// All or nothing: the second aperture cannot bind, so the first one's forwarder — already
    /// running — is killed and reaped, and the dispatch refuses (ADR 0013).
    #[test]
    fn a_failed_second_install_kills_the_first_forwarder() {
        let Some(sandbox) = Sandbox::open() else {
            return;
        };
        let model = AppServer::serving(MODEL_BODY);
        let audit = AppServer::serving(AUDIT_BODY);
        let declared = [
            named_aperture("model", "model.example.invalid", 443, model.address),
            named_aperture("audit", "audit.example.invalid", 443, audit.address),
        ];
        // The second aperture's own tuple, taken before the install: its bind fails at stage 6
        // with the first aperture's forwarder already forked.
        let second = SocketAddrV4::new(listen_address(1).expect("a second address"), 443);
        let occupied =
            listen_inside(sandbox.pid, second, None).expect("occupy the second listen tuple");
        let _ = take_spawned_forwarders();
        let Err(refusal) = install_all(&declared, sandbox.pid, None) else {
            panic!("an aperture set with an uninstallable member was installed");
        };
        assert_eq!(refusal.class, DriverErrorClass::Failed);
        assert_eq!(refusal.code, "exec.aperture-install-failed");
        assert!(
            refusal.message.contains("stage 6"),
            "the refusal is not the second aperture's bind: {}",
            refusal.message
        );
        let spawned = take_spawned_forwarders();
        assert_eq!(
            spawned.len(),
            1,
            "the first aperture's forwarder was not started before the second failed: {spawned:?}"
        );
        for pid in spawned {
            assert!(reaped(pid), "forwarder {pid} outlived the refused install");
        }
        assert_ne!(
            reach_expecting(
                sandbox.pid,
                SocketAddr::from((Ipv4Addr::LOCALHOST, 443)),
                MODEL_BODY
            ),
            Reach::Served,
            "the first aperture still served after its set was refused"
        );
        drop(occupied);
    }
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

    /// Bubblewrap 0.12.0's `--info-fd` report, one element per `write` it makes (strace), with
    /// the closing brace last.
    const INFO_WRITES: [&[u8]; 5] = [
        b"{\n    \"child-pid\": 2732544",
        b",\n    \"mnt-namespace\": 4026548654",
        b",\n    \"net-namespace\": 4026548657",
        b",\n    \"pid-namespace\": 4026548656",
        b"\n}\n",
    ];

    /// A reader that hands out one bubblewrap `write` per `read`, and counts how many it gave.
    struct InfoWrites {
        given: usize,
    }

    impl std::io::Read for InfoWrites {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            let Some(write) = INFO_WRITES.get(self.given) else {
                panic!("read past the closing brace: EOF may never come while a copy is open");
            };
            buffer[..write.len()].copy_from_slice(write);
            self.given += 1;
            Ok(write.len())
        }
    }

    /// The report is read to its closing brace before the pid is returned. A reader that stopped at
    /// `child-pid` let its caller close the pipe while bubblewrap still had four writes to make;
    /// the next one killed bubblewrap with `SIGPIPE` before its child raised `lo`, and the install
    /// failed `EADDRNOTAVAIL` (`a_failed_third_install_leaves_no_earlier_listener_answering`
    /// printed nothing, 40 of 1,600 parallel runs).
    #[test]
    fn the_sandbox_pid_is_returned_only_after_bubblewraps_last_info_write() {
        let mut info = InfoWrites { given: 0 };
        assert_eq!(read_sandbox_pid(&mut info), Some(2_732_544));
        assert_eq!(
            info.given,
            INFO_WRITES.len(),
            "the pid was returned with bubblewrap's later info writes still to come"
        );
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
        stop: Arc<AtomicBool>,
        handle: Option<std::thread::JoinHandle<()>>,
    }

    impl Firehose {
        fn start() -> Self {
            let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind firehose");
            let address = listener.local_addr().expect("firehose address");
            let stop = Arc::new(AtomicBool::new(false));
            let child_stop = Arc::clone(&stop);
            let handle = std::thread::spawn(move || {
                while !child_stop.load(AtomicOrdering::Acquire) {
                    let Ok((mut stream, _)) = listener.accept() else {
                        break;
                    };
                    if child_stop.load(AtomicOrdering::Acquire) {
                        break;
                    }
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
                stop,
                handle: Some(handle),
            }
        }
    }

    impl Drop for Firehose {
        fn drop(&mut self) {
            self.stop.store(true, AtomicOrdering::Release);
            let _ = TcpStream::connect(self.address);
            if let Some(handle) = self.handle.take() {
                let _ = handle.join();
            }
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

    /// Each aperture keeps its own ceiling (ADR 0014, per aperture, per run): an uncapped
    /// aperture passes its whole stream beside a capped one, the capped one stops at its own
    /// bound, and the set is exhausted as soon as — and only once — any one member is.
    #[test]
    fn each_aperture_ceiling_binds_only_its_own_aperture() {
        let Some(sandbox) = Sandbox::open() else {
            return;
        };
        let capped_hose = Firehose::start();
        let open_hose = Firehose::start();
        let declared = [
            capped_aperture("capped", capped_hose.address, DECLARED_CEILING),
            aperture("open", open_hose.address),
        ];
        let installed = install_all(&declared, sandbox.pid, None).expect("install both apertures");
        let [capped, open] = installed.apertures() else {
            panic!(
                "two apertures installed {} forwarders",
                installed.apertures().len()
            );
        };

        drain(sandbox.pid, SocketAddr::V4(open.listen()));
        assert_eq!(
            settled(open).from_destination,
            FIREHOSE_BYTES,
            "the uncapped aperture did not pass its whole stream"
        );
        let untouched = capped.applied().bytes;
        assert_eq!(
            untouched.to_destination + untouched.from_destination,
            0,
            "the capped aperture counted the uncapped one's bytes"
        );
        assert!(
            !installed.ceiling_exceeded(),
            "a set whose capped member moved nothing was called exhausted"
        );

        drain(sandbox.pid, SocketAddr::V4(capped.listen()));
        let bytes = settled(capped);
        let total = bytes.to_destination + bytes.from_destination;
        assert!(
            (DECLARED_CEILING..=DECLARED_CEILING + super::RELAY_BUFFER as u64).contains(&total),
            "the capped aperture did not stop at its own ceiling: {total}"
        );
        assert!(capped.ceiling_exceeded());
        assert!(
            !open.ceiling_exceeded(),
            "an aperture without a ceiling reported one exceeded"
        );
        assert!(
            installed.ceiling_exceeded(),
            "one member at its ceiling did not exhaust the set"
        );

        drain(sandbox.pid, SocketAddr::V4(open.listen()));
        assert_eq!(
            settled(open).from_destination,
            2 * FIREHOSE_BYTES,
            "the capped aperture's ceiling stopped the uncapped one's relay"
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

    /// Several apertures observed from inside the sandbox (design 26 § 2): a script in the run's
    /// namespace connects by name and by address, through the generated `/etc/hosts` bound where
    /// a run binds it, while the host installs the apertures from outside. The sandbox is held at
    /// `--block-fd` until the apertures exist, as `start_inner` orders it, and its child is bash
    /// for `/dev/tcp`. Without bubblewrap a case returns without asserting, as the cases above
    /// do: it is absent, not passed.
    mod observed_inside {
        use std::fs::File;
        use std::io::{Read as _, Write as _};
        use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
        use std::os::fd::{AsRawFd as _, FromRawFd as _, RawFd};
        use std::os::unix::process::CommandExt as _;
        use std::path::{Path, PathBuf};
        use std::process::{Child, Command, Stdio};
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::time::{Duration, Instant};

        use substrate_wire::ApertureBytes;

        use crate::egress::{
            EgressAperture, GeneratedResolution, RELAY_BUFFER, install_all, read_sandbox_pid,
        };

        const BUBBLEWRAP: &str = "/usr/bin/bwrap";

        /// A host-side destination answering every connection with `body`, then closing it.
        struct Destination {
            address: SocketAddr,
            stop: Arc<AtomicBool>,
            handle: Option<std::thread::JoinHandle<()>>,
        }

        impl Destination {
            fn serving(body: Vec<u8>) -> Self {
                let listener =
                    TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind a destination");
                let address = listener.local_addr().expect("destination address");
                let stop = Arc::new(AtomicBool::new(false));
                let serving = Arc::clone(&stop);
                let handle = std::thread::spawn(move || {
                    while let Ok((mut stream, _)) = listener.accept() {
                        if serving.load(Ordering::Acquire) {
                            break;
                        }
                        let body = body.clone();
                        std::thread::spawn(move || {
                            let _ = stream.write_all(&body);
                        });
                    }
                });
                Self {
                    address,
                    stop,
                    handle: Some(handle),
                }
            }
        }

        impl Drop for Destination {
            fn drop(&mut self) {
                self.stop.store(true, Ordering::Release);
                let _ = TcpStream::connect(self.address);
                if let Some(handle) = self.handle.take() {
                    let _ = handle.join();
                }
            }
        }

        fn aperture(
            name: &str,
            host: &str,
            pinned: SocketAddr,
            max_bytes: Option<u64>,
        ) -> EgressAperture {
            EgressAperture {
                name: name.to_owned(),
                host: host.to_owned(),
                port: 443,
                pinned,
                max_bytes,
            }
        }

        /// A pipe whose ends are close-on-exec; the sandbox's two ends are made inheritable in its
        /// own `pre_exec`, so no concurrently spawned sandbox can hold them.
        fn cloexec_pipe() -> (File, File) {
            let mut descriptors = [-1; 2];
            // SAFETY: two writable integers; `pipe2` initialises both on success.
            assert_eq!(
                unsafe { libc::pipe2(descriptors.as_mut_ptr(), libc::O_CLOEXEC) },
                0,
                "pipe2"
            );
            // SAFETY: two fresh descriptors owned here.
            unsafe {
                (
                    File::from_raw_fd(descriptors[0]),
                    File::from_raw_fd(descriptors[1]),
                )
            }
        }

        /// A sandbox shaped as a run's — no network, the generated resolution bound at
        /// `/etc/hosts` — held until [`ScriptedSandbox::finish`] releases it.
        struct ScriptedSandbox {
            child: Child,
            block: Option<File>,
            pid: u32,
            output: PathBuf,
        }

        impl ScriptedSandbox {
            fn open(scratch: &Path, hosts: Option<&Path>, script: &str) -> Option<Self> {
                if !Path::new(BUBBLEWRAP).is_file() {
                    return None;
                }
                let (mut info_read, info_write) = cloexec_pipe();
                let (block_read, block_write) = cloexec_pipe();
                let output = scratch.join(format!("sandbox-{}.out", block_read.as_raw_fd()));
                let stdout = File::create(&output).expect("sandbox output file");
                let mut command = Command::new(BUBBLEWRAP);
                command
                    .env_clear()
                    .args(crate::process::USER_NAMESPACE_ARGV)
                    .args([
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
                        "--dev",
                        "/dev",
                    ]);
                if let Some(hosts) = hosts {
                    command.arg("--ro-bind").arg(hosts).arg("/etc/hosts");
                }
                let inherited: [RawFd; 2] = [info_write.as_raw_fd(), block_read.as_raw_fd()];
                command
                    .arg("--info-fd")
                    .arg(inherited[0].to_string())
                    .arg("--block-fd")
                    .arg(inherited[1].to_string())
                    .args(["--", "/usr/bin/bash", "-c", script])
                    .stdin(Stdio::null())
                    .stdout(Stdio::from(stdout))
                    .stderr(Stdio::inherit());
                // SAFETY: `inherit` is async-signal-safe `fcntl` over two descriptor numbers.
                unsafe {
                    command.pre_exec(move || crate::process::inherit(&inherited));
                }
                let child = command.spawn().expect("spawn the sandbox");
                drop(info_write);
                drop(block_read);
                let pid = read_sandbox_pid(&mut info_read)
                    .expect("bubblewrap reported no child-pid on its --info-fd");
                Some(Self {
                    child,
                    block: Some(block_write),
                    pid,
                    output,
                })
            }

            fn release(&mut self) {
                if let Some(mut block) = self.block.take() {
                    let _ = block.write_all(&[1]);
                }
            }

            /// Releases the sandbox and returns what its script printed, killing it past
            /// `deadline`.
            fn finish(mut self, deadline: Duration) -> String {
                self.release();
                let started = Instant::now();
                loop {
                    if self
                        .child
                        .try_wait()
                        .expect("wait for the sandbox")
                        .is_some()
                    {
                        break;
                    }
                    if started.elapsed() > deadline {
                        let _ = self.child.kill();
                        let _ = self.child.wait();
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
                let mut text = String::new();
                File::open(&self.output)
                    .expect("open sandbox output")
                    .read_to_string(&mut text)
                    .expect("read sandbox output");
                text
            }
        }

        impl Drop for ScriptedSandbox {
            fn drop(&mut self) {
                self.release();
                if self.child.try_wait().ok().flatten().is_none() {
                    let _ = self.child.kill();
                    let _ = self.child.wait();
                }
            }
        }

        fn generated_hosts(resolution: &GeneratedResolution) -> PathBuf {
            resolution
                .binds()
                .into_iter()
                .find(|(_, mount)| *mount == substrate_wire::APERTURE_HOSTS_PATH)
                .expect("the generated resolution binds /etc/hosts")
                .0
                .to_path_buf()
        }

        /// An aperture whose declared host is `localhost` is reached at that name by its own
        /// destination, and so it may only be listed first.
        ///
        /// The generated file opens with `127.0.0.1\tlocalhost`, so a second aperture declaring
        /// `localhost` would be answered by the first aperture's forwarder and its bytes would go
        /// to the first aperture's destination, counted on the wrong aperture: that set is not
        /// generated. Listed first, the name's line and its listen address agree, and a child
        /// that connects to the declared `localhost:443` reaches that aperture's destination.
        #[test]
        fn a_listed_aperture_whose_host_is_localhost_reaches_its_own_destination() {
            let scratch = tempfile::tempdir().unwrap();
            let model = Destination::serving(b"model".to_vec());
            let sidecar = Destination::serving(b"sidecar".to_vec());
            let model_first = [
                aperture("model", "model.example.invalid", model.address, None),
                aperture("sidecar", "localhost", sidecar.address, None),
            ];
            let Err(refusal) = GeneratedResolution::prepare(&model_first, None, scratch.path())
            else {
                panic!("a second aperture declaring localhost was given a mapping");
            };
            assert_eq!(refusal.code, "exec.aperture-install-failed");

            let declared = [model_first[1].clone(), model_first[0].clone()];
            let resolution = GeneratedResolution::prepare(&declared, None, scratch.path())
                .expect("generate the resolution for two apertures");
            let hosts = generated_hosts(&resolution);
            // The first connection is calibration: a name only the generated file knows resolves
            // and is served, so the second connection's answer is about `localhost` alone.
            let script = "exec 3<>/dev/tcp/model.example.invalid/443 \
                          && printf 'model.example.invalid=' && cat <&3 && exec 3<&- \
                          && exec 4<>/dev/tcp/localhost/443 && printf ' localhost=' && cat <&4";
            let Some(sandbox) = ScriptedSandbox::open(scratch.path(), Some(&hosts), script) else {
                return;
            };
            let installed =
                install_all(&declared, sandbox.pid, None).expect("install both apertures");
            let printed = sandbox.finish(Duration::from_secs(30));
            let bytes: Vec<ApertureBytes> = installed
                .apertures()
                .iter()
                .map(|one| one.applied().bytes)
                .collect();
            drop(installed);
            assert!(resolution.close());
            assert_eq!(
                printed, "model.example.invalid=model localhost=sidecar",
                "the declared host `localhost` was not answered by its own destination; bytes \
                 per aperture: {bytes:?}"
            );
        }

        /// All or nothing, after the second install: the third aperture fails (an IPv6
        /// destination, refused after its listener is bound), and neither earlier forwarder still
        /// answers inside. Observed from inside the namespace rather than from a pid list: a
        /// forwarder that survived holds its listener, and a child connecting to its address
        /// would be accepted.
        #[test]
        fn a_failed_third_install_leaves_no_earlier_listener_answering() {
            let scratch = tempfile::tempdir().unwrap();
            let model = Destination::serving(b"model".to_vec());
            let audit = Destination::serving(b"audit".to_vec());
            let declared = [
                aperture("model", "model.example.invalid", model.address, None),
                aperture("audit", "audit.example.invalid", audit.address, None),
                aperture(
                    "broken",
                    "broken.example.invalid",
                    "[::1]:9".parse().unwrap(),
                    None,
                ),
            ];
            let script = "for a in 127.0.0.1 127.0.0.2 127.0.0.3; do \
                            if (exec 3<>/dev/tcp/$a/443) 2>/dev/null; then printf '%s=open ' $a; \
                            else printf '%s=refused ' $a; fi; done";
            let Some(sandbox) = ScriptedSandbox::open(scratch.path(), None, script) else {
                return;
            };
            let Err(refusal) = install_all(&declared, sandbox.pid, None) else {
                panic!("a set with an uninstallable third member was installed");
            };
            assert_eq!(refusal.code, "exec.aperture-install-failed");
            assert!(
                refusal.message.contains("Only IPv4"),
                "the set was refused before its third member: {}",
                refusal.message
            );
            let printed = sandbox.finish(Duration::from_secs(30));
            assert_eq!(
                printed, "127.0.0.1=refused 127.0.0.2=refused 127.0.0.3=refused ",
                "a forwarder of a refused set still answers inside the namespace"
            );
        }

        /// Another thread of this process forking throughout an install, as the daemon's other
        /// runs and this binary's other cases do. Each child holds a copy of every descriptor the
        /// process had when it forked, for `hold`, as any forked child does until it closes its
        /// inherited table or execs — a forwarder's listener among them when the fork lands while
        /// the installing thread still has it open. At most `MAX_CHILDREN` are forked, so a slow
        /// install cannot fill the host's process table.
        struct ForkingNeighbour {
            stop: Arc<AtomicBool>,
            handle: Option<std::thread::JoinHandle<usize>>,
        }

        impl ForkingNeighbour {
            const MAX_CHILDREN: usize = 4096;

            fn start(hold: Duration) -> Self {
                let stop = Arc::new(AtomicBool::new(false));
                let stopped = Arc::clone(&stop);
                let hold = libc::c_int::try_from(hold.as_millis()).expect("hold in milliseconds");
                let handle = std::thread::spawn(move || {
                    let mut children = Vec::new();
                    while !stopped.load(Ordering::Acquire) && children.len() < Self::MAX_CHILDREN {
                        // SAFETY: the child calls only `poll` with no descriptors and `_exit`,
                        // both async-signal-safe, and never returns into Rust code.
                        let pid = unsafe { libc::fork() };
                        if pid == 0 {
                            // SAFETY: post-fork child, async-signal-safe calls only.
                            unsafe {
                                libc::poll(std::ptr::null_mut(), 0, hold);
                                libc::_exit(0);
                            }
                        }
                        assert!(pid > 0, "fork");
                        children.push(pid);
                        std::thread::sleep(Duration::from_micros(200));
                    }
                    for pid in &children {
                        let mut status = 0;
                        // SAFETY: reaping a child this thread forked.
                        unsafe {
                            libc::waitpid(*pid, &raw mut status, 0);
                        }
                    }
                    children.len()
                });
                Self {
                    stop,
                    handle: Some(handle),
                }
            }

            /// Stops forking; the children forked so far keep their copies until `hold` ends.
            fn stop(&self) {
                self.stop.store(true, Ordering::Release);
            }

            /// Waits for every child, and returns how many there were.
            fn join(mut self) -> usize {
                self.stop();
                self.handle
                    .take()
                    .expect("joined once")
                    .join()
                    .expect("the forking thread")
            }
        }

        impl Drop for ForkingNeighbour {
            fn drop(&mut self) {
                self.stop();
                if let Some(handle) = self.handle.take() {
                    let _ = handle.join();
                }
            }
        }

        const FORKING_HELPER: &str =
            "egress::tests::observed_inside::forking_neighbour_third_install_helper";
        const FORKING_HELPER_ENV: &str = "SUBSTRATE_TEST_FORKING_NEIGHBOUR_HELPER";

        /// The case above, with a neighbour thread forking throughout the install
        /// (`story:egress-third-install-case-holds-under-load`). A forwarder's listener copied
        /// into a neighbour's child outlives the forwarder that was killed and reaped, so a set
        /// whose teardown relied on the last descriptor closing still answered inside. Each
        /// listener is shut down at the socket, which every copy shares.
        ///
        /// The install runs in a process of its own, this test binary running only
        /// [`forking_neighbour_third_install_helper`]: the neighbour's children copy every
        /// descriptor of the process they fork from, and in this one they would hold the other
        /// cases' terminals and pipes too.
        #[test]
        fn a_failed_third_install_leaves_no_listener_answering_while_another_thread_forks() {
            let output = std::process::Command::new(std::env::current_exe().expect("test binary"))
                .args([
                    FORKING_HELPER,
                    "--exact",
                    "--ignored",
                    "--nocapture",
                    "--test-threads=1",
                ])
                .env(FORKING_HELPER_ENV, "1")
                .output()
                .expect("run the helper");
            let stdout = String::from_utf8_lossy(&output.stdout);
            let report = format!("{stdout}{}", String::from_utf8_lossy(&output.stderr));
            assert!(output.status.success(), "the helper failed: {report}");
            assert!(
                stdout.contains("test result: ok. 1 passed;"),
                "the helper did not run exactly once: {report}"
            );
            if stdout.contains("FORKING-NEIGHBOUR absent") {
                return;
            }
            assert!(
                stdout.contains("FORKING-NEIGHBOUR held"),
                "the helper ran without reaching its assertion: {report}"
            );
        }

        /// Run only by the case above, in a process of its own.
        #[test]
        #[ignore = "run by a_failed_third_install_leaves_no_listener_answering_while_another_thread_forks"]
        fn forking_neighbour_third_install_helper() {
            if std::env::var_os(FORKING_HELPER_ENV).is_none() {
                return;
            }
            let scratch = tempfile::tempdir().unwrap();
            let model = Destination::serving(b"model".to_vec());
            let audit = Destination::serving(b"audit".to_vec());
            let declared = [
                aperture("model", "model.example.invalid", model.address, None),
                aperture("audit", "audit.example.invalid", audit.address, None),
                aperture(
                    "broken",
                    "broken.example.invalid",
                    "[::1]:9".parse().unwrap(),
                    None,
                ),
            ];
            let script = "for a in 127.0.0.1 127.0.0.2 127.0.0.3; do \
                            if (exec 3<>/dev/tcp/$a/443) 2>/dev/null; then printf '%s=open ' $a; \
                            else printf '%s=refused ' $a; fi; done";
            let Some(sandbox) = ScriptedSandbox::open(scratch.path(), None, script) else {
                println!("FORKING-NEIGHBOUR absent");
                return;
            };
            let neighbour = ForkingNeighbour::start(Duration::from_secs(2));
            let refused = install_all(&declared, sandbox.pid, None);
            neighbour.stop();
            let Err(refusal) = refused else {
                panic!("a set with an uninstallable third member was installed");
            };
            assert!(
                refusal.message.contains("Only IPv4"),
                "the set was refused before its third member: {}",
                refusal.message
            );
            let printed = sandbox.finish(Duration::from_secs(30));
            let forked = neighbour.join();
            assert!(forked > 0, "the neighbour forked nothing");
            assert_eq!(
                printed, "127.0.0.1=refused 127.0.0.2=refused 127.0.0.3=refused ",
                "a forwarder of a refused set still answers inside the namespace while {forked} \
                 children of a neighbour thread held the descriptors of the install"
            );
            println!("FORKING-NEIGHBOUR held");
        }

        /// Adversary, `story:egress-third-install-case-holds-under-load` pass 1. Once the
        /// installing process keeps the listener for the aperture's lifetime, the forwarder is no
        /// longer its only owner: a forwarder that dies during the run (its `accept4` error exit,
        /// or a kill inside the run's cgroup) leaves the socket listening with nobody accepting.
        /// A child inside then completes a handshake into a backlog nobody drains, where before
        /// the last descriptor closed with the forwarder and the connect was refused.
        ///
        /// The forwarder is killed here and waited for without being reaped, so the aperture's
        /// own drop still reaps exactly the pid it forked.
        #[test]
        fn a_dead_forwarder_leaves_its_aperture_refusing_rather_than_accepting_into_nothing() {
            let scratch = tempfile::tempdir().unwrap();
            let model = Destination::serving(b"model".to_vec());
            let declared = [aperture(
                "model",
                "model.example.invalid",
                model.address,
                None,
            )];
            let script = "if (exec 3<>/dev/tcp/127.0.0.1/443) 2>/dev/null; then printf open; \
                          else printf refused; fi";
            let Some(sandbox) = ScriptedSandbox::open(scratch.path(), None, script) else {
                return;
            };
            let installed = install_all(&declared, sandbox.pid, None).expect("install one");
            let forwarder = installed.apertures()[0].forwarder;
            // SAFETY: a pid this process forked and has not reaped; `WNOWAIT` leaves it a zombie
            // for the aperture's drop to reap.
            unsafe {
                libc::kill(forwarder, libc::SIGKILL);
                let mut info: libc::siginfo_t = std::mem::zeroed();
                let waited = libc::waitid(
                    libc::P_PID,
                    libc::id_t::try_from(forwarder).expect("forwarder pid"),
                    &raw mut info,
                    libc::WEXITED | libc::WNOWAIT,
                );
                assert_eq!(waited, 0, "wait for the killed forwarder");
            }
            let printed = sandbox.finish(Duration::from_secs(30));
            drop(installed);
            assert_eq!(
                printed, "refused",
                "with its forwarder dead, the aperture still completed a connection from inside \
                 that nothing will ever accept"
            );
        }

        /// Each ceiling is independent under concurrent connections (ADR 0014 per aperture;
        /// design 26 § 2): four relays at once through a capped aperture and four through an
        /// uncapped one on the same port. The uncapped one passes every byte, and the capped one
        /// stops within one relay buffer per live relay of its own ceiling — not of the sum of
        /// both apertures' bytes.
        #[test]
        fn concurrent_relays_on_two_apertures_keep_independent_ceilings() {
            const STREAM: usize = 1 << 20;
            const CEILING: u64 = 64 * 1024;
            const RELAYS: u64 = 4;
            let scratch = tempfile::tempdir().unwrap();
            let capped_hose = Destination::serving(vec![b'c'; STREAM]);
            let open_hose = Destination::serving(vec![b'o'; STREAM]);
            let declared = [
                aperture(
                    "capped",
                    "capped.example.invalid",
                    capped_hose.address,
                    Some(CEILING),
                ),
                aperture("open", "open.example.invalid", open_hose.address, None),
            ];
            let script = "for i in 1 2 3 4; do \
                            (exec 3<>/dev/tcp/127.0.0.1/443 && cat <&3 >/dev/null) 2>/dev/null & \
                            (exec 4<>/dev/tcp/127.0.0.2/443 && cat <&4 >/dev/null) 2>/dev/null & \
                          done; wait; printf done";
            let Some(sandbox) = ScriptedSandbox::open(scratch.path(), None, script) else {
                return;
            };
            let installed =
                install_all(&declared, sandbox.pid, None).expect("install both apertures");
            let printed = sandbox.finish(Duration::from_mins(1));
            assert_eq!(printed, "done", "the concurrent clients did not all finish");
            let [capped, open] = installed.apertures() else {
                panic!("two apertures installed {}", installed.apertures().len());
            };
            let expected_open = RELAYS * STREAM as u64;
            let settle = Instant::now();
            while open.applied().bytes.from_destination < expected_open
                && settle.elapsed() < Duration::from_secs(5)
            {
                std::thread::sleep(Duration::from_millis(10));
            }
            let open_bytes = open.applied().bytes;
            let capped_bytes = capped.applied().bytes;
            assert_eq!(
                open_bytes.from_destination, expected_open,
                "the uncapped aperture lost bytes beside a capped one: {open_bytes:?}"
            );
            let capped_total = capped_bytes.to_destination + capped_bytes.from_destination;
            let per_relay = RELAY_BUFFER as u64;
            assert!(
                (CEILING..=CEILING + RELAYS * per_relay).contains(&capped_total),
                "the capped aperture's concurrent relays did not stop within one buffer each of \
                 its ceiling: {capped_total}"
            );
            assert!(capped.ceiling_exceeded());
            assert!(!open.ceiling_exceeded());
            drop(installed);
        }
    }
}
