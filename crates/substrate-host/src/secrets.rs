//! Sealed secret slots: an operator-declared credential reaches a child as a sealed `memfd`
//! descriptor and never as a byte substrate can emit (ADR 0012,
//! `docs/design/11-sealed-secret-slots.md`).
//!
//! The whole mechanism is four steps and the order is the guarantee. Read the declared file into a
//! buffer that zeroes itself; copy it into an anonymous `memfd`; seal that memfd with exactly
//! `F_SEAL_WRITE | F_SEAL_SHRINK | F_SEAL_GROW | F_SEAL_SEAL` and read the seals back; stage the
//! descriptor above the declared ceiling so a request can never name it. Everything after that is
//! `dup2` in `pre_exec` and closing the daemon's own copy the moment the child exists.
//!
//! `F_SEAL_SEAL` is what makes this different from handing over a file: it closes the seal set, so
//! no later holder — substrate included — can add or remove a seal on the memory it gave away.

use std::io::{Read as _, Write as _};
use std::os::fd::{AsRawFd as _, FromRawFd as _, OwnedFd, RawFd};
use std::os::unix::fs::MetadataExt as _;
use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;

#[cfg(test)]
use sha2::{Digest as _, Sha256};
use substrate_wire::{
    MAX_SECRET_SLOT_BYTES, MAX_SECRET_SLOT_FD, SecretSlotRequest, secret_slot_environment,
    validate_secret_slots,
};
use zeroize::Zeroizing;

use crate::{DriverError, SecretSlot};

/// Exactly the seal set ADR 0012 names, and exactly `0xf`. No more and no less: a slot carrying a
/// seal substrate did not declare is a slot whose guarantees nobody wrote down.
pub(crate) const SEAL_SET: i32 =
    libc::F_SEAL_WRITE | libc::F_SEAL_SHRINK | libc::F_SEAL_GROW | libc::F_SEAL_SEAL;

/// The lowest descriptor a staged source may occupy.
///
/// Above [`MAX_SECRET_SLOT_FD`], so a request can never name a descriptor the driver is already
/// using as a source and `dup2` can never overwrite one placement with another.
const STAGE_FLOOR: RawFd = MAX_SECRET_SLOT_FD as RawFd + 1;

/// One slot acquired for one start: the sealed memory, and where the child must find it.
struct StagedSlot {
    target: RawFd,
    /// Staged above [`STAGE_FLOOR`] with `F_DUPFD_CLOEXEC`, so the fork inherits it and any
    /// concurrent start's fork does not.
    source: OwnedFd,
    /// SHA-256 of the sealed bytes. Never emitted anywhere a client can read; the tests use it to
    /// prove two acquisitions delivered different material without ever naming it.
    #[cfg(test)]
    digest: String,
}

/// Every slot one start acquired, released together.
///
/// Dropping this closes the daemon's copies. That is the whole cleanup story: a memfd has no name
/// in any filesystem, so once no descriptor refers to it the kernel frees it and there is no
/// residue for a restart to sweep.
pub(crate) struct SecretSlotSet {
    staged: Vec<StagedSlot>,
    requested: Vec<SecretSlotRequest>,
}

/// Names and descriptors, deliberately hand-written.
///
/// A derived `Debug` would print whatever a future field holds, and the one place a credential
/// escapes an otherwise careful design is a diagnostic somebody added later. This one cannot.
impl std::fmt::Debug for SecretSlotSet {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SecretSlotSet")
            .field("slots", &self.environment().unwrap_or_default())
            .finish()
    }
}

impl SecretSlotSet {
    /// Reads, seals and stages every named slot.
    ///
    /// Call this **after** every admission check and the backend recheck: nothing is read until the
    /// dispatch is otherwise certain, so a refused start never touches the material.
    ///
    /// # Errors
    ///
    /// Refuses an undeclared name or an illegal descriptor, and fails a file it cannot read within
    /// its bounds or memory it cannot prove sealed. Every message names the slot and nothing else.
    pub(crate) fn acquire(
        declarations: &[SecretSlot],
        requested: &[SecretSlotRequest],
    ) -> Result<Self, DriverError> {
        validate_secret_slots(requested).map_err(|error| {
            DriverError::refused(
                "exec.secret-slot-descriptor-invalid",
                format!("A named secret slot is not admissible: {error}"),
                "secret_slots",
            )
        })?;
        let mut staged = Vec::with_capacity(requested.len());
        for request in requested {
            let declaration = declarations
                .iter()
                .find(|declaration| declaration.name == request.slot)
                .ok_or_else(|| {
                    DriverError::refused(
                        "exec.secret-slot-unknown",
                        format!("Secret slot {} is not declared on this host.", request.slot),
                        "secret_slots",
                    )
                })?;
            let material = read_declared(&declaration.name, &declaration.path)?;
            #[cfg(test)]
            let digest = hex::encode(Sha256::digest(material.as_slice()));
            let sealed = seal(&declaration.name, &material)?;
            drop(material);
            staged.push(StagedSlot {
                target: RawFd::try_from(request.fd).map_err(|_| {
                    DriverError::refused(
                        "exec.secret-slot-descriptor-invalid",
                        format!("Secret slot {} names an unusable descriptor.", request.slot),
                        "secret_slots",
                    )
                })?,
                source: stage(&declaration.name, &sealed)?,
                #[cfg(test)]
                digest,
            });
        }
        Ok(Self {
            staged,
            requested: requested.to_vec(),
        })
    }

    /// `(staged source, declared target)` for each slot, as plain integers.
    ///
    /// Integers on purpose: this list crosses `fork` into `pre_exec`, where nothing may allocate
    /// and nothing may run a destructor.
    pub(crate) fn placements(&self) -> Vec<(RawFd, RawFd)> {
        self.staged
            .iter()
            .map(|slot| (slot.source.as_raw_fd(), slot.target))
            .collect()
    }

    /// The descriptors the child keeps: stdio, the launch barrier, and every declared slot.
    ///
    /// Sorted and deduplicated, because [`gaps`] walks it as a ladder.
    pub(crate) fn retained(&self, barrier: Option<RawFd>) -> Vec<u32> {
        let mut retained = vec![0_u32, 1, 2];
        retained.extend(
            self.staged
                .iter()
                .filter_map(|slot| u32::try_from(slot.target).ok()),
        );
        retained.extend(barrier.and_then(|fd| u32::try_from(fd).ok()));
        retained.sort_unstable();
        retained.dedup();
        retained
    }

    /// The `SUBSTRATE_SECRET_SLOTS` value, or `None` when this start named no slot.
    pub(crate) fn environment(&self) -> Option<String> {
        secret_slot_environment(&self.requested)
    }

    /// What was placed, for the applied confinement record.
    pub(crate) fn applied(&self) -> Vec<SecretSlotRequest> {
        self.requested.clone()
    }

    /// A digest over the sealed material, in slot order. Test-only evidence that two acquisitions
    /// carried different bytes; never emitted anywhere a client can read.
    #[cfg(test)]
    pub(crate) fn acquired_digest(&self) -> String {
        let mut hasher = Sha256::new();
        for slot in &self.staged {
            hasher.update(slot.digest.as_bytes());
        }
        hex::encode(hasher.finalize())
    }
}

/// Reads one declared slot file: one bounded regular file with private workload ownership.
///
/// The same predicate the TCP bearer file already carries
/// (`crates/substrate-daemon/src/runtime.rs`), for the same reason: a credential readable by the
/// group is a credential the group has.
fn read_declared(name: &str, path: &Path) -> Result<Zeroizing<Vec<u8>>, DriverError> {
    let unreadable = |reason: &str| {
        DriverError::failed(
            "exec.secret-slot-unreadable",
            format!("Secret slot {name} is not readable as declared: {reason}."),
        )
    };
    let mut file = std::fs::File::open(path).map_err(|_| unreadable("open refused"))?;
    let metadata = file
        .metadata()
        .map_err(|_| unreadable("cannot be inspected"))?;
    let mode = metadata.permissions().mode();
    // SAFETY-adjacent: `geteuid` is infallible and has no side effect.
    let effective_user = unsafe { libc::geteuid() };
    if !metadata.is_file() {
        return Err(unreadable("not a regular file"));
    }
    if metadata.len() == 0 || metadata.len() > MAX_SECRET_SLOT_BYTES {
        return Err(unreadable("outside its admitted byte bound"));
    }
    if metadata.uid() != effective_user || mode & 0o077 != 0 {
        return Err(unreadable("not owner-private"));
    }
    let mut material = Zeroizing::new(Vec::new());
    std::io::Read::by_ref(&mut file)
        .take(MAX_SECRET_SLOT_BYTES + 1)
        .read_to_end(&mut material)
        .map_err(|_| unreadable("read refused"))?;
    if material.is_empty() || material.len() as u64 > MAX_SECRET_SLOT_BYTES {
        return Err(unreadable("outside its admitted byte bound"));
    }
    Ok(material)
}

/// Copies `material` into an anonymous `memfd` and seals it, then proves the seal from the outside.
///
/// The proof is deliberately not the return value of `F_ADD_SEALS`: the seals are read back and a
/// write is attempted, because a guarantee substrate only asserts is not a guarantee (invariant 3).
fn seal(name: &str, material: &[u8]) -> Result<OwnedFd, DriverError> {
    let unsealed = |reason: &str| {
        DriverError::failed(
            "exec.secret-slot-unsealed",
            format!("Secret slot {name} could not be proven sealed: {reason}."),
        )
    };
    let memfd_name = std::ffi::CString::new(format!("substrate-slot-{name}"))
        .map_err(|_| unsealed("has an unusable name"))?;
    // SAFETY: `memfd_name` is a NUL-terminated C string that outlives the call, and the returned
    // descriptor is taken into an `OwnedFd` immediately.
    let raw = unsafe {
        libc::memfd_create(
            memfd_name.as_ptr(),
            libc::MFD_CLOEXEC | libc::MFD_ALLOW_SEALING,
        )
    };
    if raw < 0 {
        return Err(unsealed("anonymous memory is unavailable"));
    }
    // SAFETY: `raw` is a fresh descriptor this call owns and nothing else holds.
    let memfd = unsafe { OwnedFd::from_raw_fd(raw) };
    {
        // SAFETY: the file is owned by `memfd` for the length of this block and is not closed by
        // the borrowed `File`, which is forgotten rather than dropped.
        let mut sink =
            std::mem::ManuallyDrop::new(unsafe { std::fs::File::from_raw_fd(memfd.as_raw_fd()) });
        sink.write_all(material)
            .map_err(|_| unsealed("could not be written before sealing"))?;
        sink.flush()
            .map_err(|_| unsealed("could not be written before sealing"))?;
    }
    // The child inherits this *open file description*, offset included, so a descriptor left at
    // EOF hands over a slot that reads as empty. Rewind before sealing: after `F_SEAL_WRITE` the
    // offset is still the daemon's to set, and a reader that has to seek first is a contract nobody
    // wrote down.
    //
    // SAFETY: a seek on a descriptor this function owns.
    if unsafe { libc::lseek(memfd.as_raw_fd(), 0, libc::SEEK_SET) } != 0 {
        return Err(unsealed("could not be rewound before sealing"));
    }
    // SAFETY: `memfd` is a live descriptor this function owns.
    if unsafe { libc::fcntl(memfd.as_raw_fd(), libc::F_ADD_SEALS, SEAL_SET) } != 0 {
        return Err(unsealed("the kernel refused the seal set"));
    }
    // SAFETY: as above.
    let readback = unsafe { libc::fcntl(memfd.as_raw_fd(), libc::F_GET_SEALS) };
    if readback != SEAL_SET {
        return Err(unsealed("the seals read back are not the declared set"));
    }
    let refused = b"\0";
    // SAFETY: a one-byte write into a descriptor this function owns; it must fail.
    let written = unsafe {
        libc::pwrite(
            memfd.as_raw_fd(),
            refused.as_ptr().cast::<libc::c_void>(),
            refused.len(),
            0,
        )
    };
    if written >= 0 {
        return Err(unsealed("sealed memory still accepted a write"));
    }
    Ok(memfd)
}

/// Moves a sealed descriptor above the declared ceiling, keeping it close-on-exec.
///
/// `MFD_CLOEXEC` is not decoration and neither is this: starts overlap
/// (`max_concurrent_execs` defaults to 16), so an inheritable slot descriptor would be inherited by
/// *another subject's* child between its fork and its `close_range`.
fn stage(name: &str, memfd: &OwnedFd) -> Result<OwnedFd, DriverError> {
    // SAFETY: `memfd` is live for the call and the duplicate is taken into an `OwnedFd`.
    let raw = unsafe { libc::fcntl(memfd.as_raw_fd(), libc::F_DUPFD_CLOEXEC, STAGE_FLOOR) };
    if raw < 0 {
        return Err(DriverError::failed(
            "exec.secret-slot-unsealed",
            format!("Secret slot {name} could not be staged above the declared range."),
        ));
    }
    // SAFETY: `raw` is a fresh descriptor nothing else holds.
    Ok(unsafe { OwnedFd::from_raw_fd(raw) })
}

/// The closed ranges *between* the retained descriptors, ascending, ending at `u32::MAX`.
///
/// With no slot declared, `{0,1,2,barrier}` produces exactly the two windows the driver closed
/// before ADR 0012, so today's behaviour is this function's special case rather than a branch.
pub(crate) fn gaps(retained: &[u32]) -> Vec<(u32, u32)> {
    let mut ranges = Vec::new();
    let mut previous: Option<u32> = None;
    for &descriptor in retained {
        if let Some(last) = previous {
            if descriptor > last + 1 {
                ranges.push((last + 1, descriptor - 1));
            }
        } else if descriptor > 0 {
            ranges.push((0, descriptor - 1));
        }
        previous = Some(descriptor);
    }
    match previous {
        Some(last) if last < u32::MAX => ranges.push((last + 1, u32::MAX)),
        Some(_) => {}
        None => ranges.push((0, u32::MAX)),
    }
    ranges
}

/// Places every slot at its declared descriptor and closes everything else above stdio.
///
/// Runs inside `pre_exec`, after the fork: async-signal-safe calls only, no allocation, no
/// destructor. `dup2` is what makes the target inheritable — it clears `FD_CLOEXEC` on that one
/// descriptor and on nothing else.
///
/// # Errors
///
/// Any failing syscall aborts the child before `execve`, so a slot that could not be placed never
/// becomes a process that ran without it.
pub(crate) fn place_and_close(
    placements: &[(RawFd, RawFd)],
    retained: &[u32],
) -> std::io::Result<()> {
    for &(source, target) in placements {
        // SAFETY: both descriptors are plain integers the parent held open across the fork.
        if unsafe { libc::dup2(source, target) } < 0 {
            return Err(std::io::Error::last_os_error());
        }
    }
    for (first, last) in gaps(retained) {
        // SAFETY: `close_range` takes two descriptor numbers and no memory.
        if unsafe { libc::syscall(libc::SYS_close_range, first, last, 0_u32) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
    }
    Ok(())
}

/// The `secrets.slots` capability fact, or `None`.
///
/// Every clause is a proof obligation, and the fact is absent unless all of them hold. There is no
/// weaker delivery to fall back to, so absence is the whole refusal (invariant 3).
pub(crate) fn secret_slots_fact(
    declarations: &[SecretSlot],
    sealing_proven: bool,
    passthrough_proven: bool,
) -> Option<Vec<String>> {
    if declarations.is_empty() || !sealing_proven || !passthrough_proven {
        return None;
    }
    let mut names: Vec<String> = declarations
        .iter()
        .map(|declaration| declaration.name.clone())
        .collect();
    names.sort();
    names.dedup();
    Some(names)
}

/// Proves in this process that a `memfd` can be created, sealed with exactly [`SEAL_SET`], read
/// back as that set, and then refuses a write.
pub(crate) fn sealing_is_provable() -> bool {
    let probe = b"substrate-secret-slot-probe";
    match seal("probe", probe) {
        Ok(memfd) => {
            let mut readback = [0_u8; 32];
            // SAFETY: a bounded read from a descriptor this function owns.
            let read = unsafe {
                libc::pread(
                    memfd.as_raw_fd(),
                    readback.as_mut_ptr().cast::<libc::c_void>(),
                    probe.len(),
                    0,
                )
            };
            read == probe.len().cast_signed() && &readback[..probe.len()] == probe
        }
        Err(_) => false,
    }
}

/// A sealed descriptor placed for a child, for the pass-through probe.
///
/// Returns the staged source and the number the child must find it at.
pub(crate) fn probe_slot(sentinel: &str) -> Option<(OwnedFd, RawFd)> {
    let memfd = seal("probe", sentinel.as_bytes()).ok()?;
    let staged = stage("probe", &memfd).ok()?;
    Some((staged, PROBE_DESCRIPTOR))
}

/// The descriptor the pass-through probe places its sealed memfd at.
pub(crate) const PROBE_DESCRIPTOR: RawFd = 7;

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::io::{BufRead as _, BufReader, Write as _};
    use std::os::unix::fs::PermissionsExt as _;
    use std::os::unix::process::CommandExt as _;
    use std::path::Path;
    use std::process::{Child, Command, Stdio};

    use substrate_wire::{
        AppliedConfinement, AppliedFilesystem, AppliedNetwork, ConfinementRequest, ExecEnvironment,
        ExecLimits, ExecStartInput, NetworkMode, SandboxProfile, SecretSlotRequest,
        canonical_request_hash_v2,
    };

    use super::{SEAL_SET, SecretSlotSet, secret_slots_fact};
    use crate::{DriverErrorClass, HostConfig, SecretSlot};

    /// The value the tests hunt for. High entropy, so a substring hit is never a coincidence.
    const SENTINEL: &str = "s3nt1nel-4f2c9a17b6d84e05-vendor-material";
    /// Set on the re-executed test binary to make it act as the observed child.
    const CHILD_FD: &str = "SUBSTRATE_SLOT_TEST_CHILD_FD";
    const CHILD_LINE: &str = "SLOTPROBE";

    fn declare(directory: &Path, name: &str, value: &str) -> SecretSlot {
        let path = directory.join(name);
        std::fs::write(&path, value).expect("write declared slot file");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
            .expect("restrict declared slot file");
        SecretSlot {
            name: name.to_owned(),
            path,
        }
    }

    fn request(slot: &str, fd: u32) -> SecretSlotRequest {
        SecretSlotRequest {
            slot: slot.to_owned(),
            fd,
        }
    }

    fn exec_input(slots: Vec<SecretSlotRequest>) -> ExecStartInput {
        ExecStartInput {
            workspace: "ws_test".to_owned(),
            argv: vec!["/usr/bin/true".to_owned()],
            env: ExecEnvironment {
                allow: Vec::new(),
                set: BTreeMap::new(),
            },
            sandbox: ConfinementRequest {
                capability_snapshot: format!("sha256:{}", "7".repeat(64)),
                network: NetworkMode::None,
                aperture: None,
                profile: SandboxProfile::Workspace,
                required: true,
            },
            limits: ExecLimits {
                timeout_ms: 5_000,
                output_bytes: 65_536,
                processes: 16,
                memory_bytes: 67_108_864,
                cpu_millis: 1_000,
            },
            wait: false,
            read_only_roots: Vec::new(),
            secret_slots: slots,
            capsule: None,
            lease_ttl_ms: None,
        }
    }

    /// What the child saw, reported by the re-executed test binary from inside the child.
    struct ChildReport {
        seals: i32,
        write_errno: i32,
        value_sha256: String,
        inherited_sha256: String,
        link: String,
        descriptors: Vec<u32>,
        cmdline: String,
        environ: String,
        stdout: String,
    }

    /// Spawns the test binary as the observed child with `slots` placed the way a start places
    /// them, and keeps it alive until [`ChildReport::collect`] releases it.
    fn spawn_child(slots: &SecretSlotSet, fd: u32) -> Child {
        let mut command = Command::new(std::env::current_exe().expect("test binary"));
        command
            .args([
                "--exact",
                "secrets::tests::the_child_reports_its_declared_descriptor",
                "--nocapture",
            ])
            .env(CHILD_FD, fd.to_string())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let placements = slots.placements();
        let retained = slots.retained(None);
        // SAFETY: the closure runs after fork and calls only async-signal-safe libc entry points.
        unsafe {
            command.pre_exec(move || super::place_and_close(&placements, &retained));
        }
        command.spawn().expect("spawn the observed child")
    }

    impl ChildReport {
        fn collect(mut child: Child, pid: u32) -> Self {
            let cmdline = std::fs::read(format!("/proc/{pid}/cmdline")).unwrap_or_default();
            let environ = std::fs::read(format!("/proc/{pid}/environ")).unwrap_or_default();
            child
                .stdin
                .take()
                .expect("child stdin")
                .write_all(b"go\n")
                .expect("release the child");
            let stdout = child.stdout.take().expect("child stdout");
            let mut line = String::new();
            let mut reported = String::new();
            let mut reader = BufReader::new(stdout);
            while reader.read_line(&mut line).expect("read child stdout") > 0 {
                if line.starts_with(CHILD_LINE) {
                    reported = line.trim_end().to_owned();
                }
                line.clear();
            }
            let status = child.wait().expect("await the child");
            assert!(status.success(), "the observed child failed: {status}");
            let mut fields = BTreeMap::new();
            for field in reported
                .strip_prefix(CHILD_LINE)
                .unwrap_or_default()
                .split_whitespace()
            {
                if let Some((key, value)) = field.split_once('=') {
                    fields.insert(key.to_owned(), value.to_owned());
                }
            }
            let field = |key: &str| fields.get(key).cloned().unwrap_or_default();
            Self {
                seals: field("seals").parse().unwrap_or(-1),
                write_errno: field("write_errno").parse().unwrap_or(-1),
                value_sha256: field("value_sha256"),
                inherited_sha256: field("inherited_sha256"),
                link: field("link"),
                descriptors: field("fds")
                    .split(',')
                    .filter_map(|value| value.parse().ok())
                    .collect(),
                cmdline: String::from_utf8_lossy(&cmdline).into_owned(),
                environ: String::from_utf8_lossy(&environ).into_owned(),
                stdout: reported,
            }
        }
    }

    fn digest(value: &str) -> String {
        use sha2::{Digest as _, Sha256};
        hex::encode(Sha256::digest(value.as_bytes()))
    }

    /// The child half of every observation below. Inert unless the marker names a descriptor.
    #[test]
    fn the_child_reports_its_declared_descriptor() {
        let Ok(fd) = std::env::var(CHILD_FD) else {
            return;
        };
        let fd: i32 = fd.parse().expect("the marker names a descriptor");
        // SAFETY: every call reads or probes the inherited descriptor and allocates nothing the
        // kernel owns.
        let seals = unsafe { libc::fcntl(fd, libc::F_GET_SEALS) };
        let refused = b"x";
        let written = unsafe {
            libc::pwrite(
                fd,
                refused.as_ptr().cast::<libc::c_void>(),
                refused.len(),
                0,
            )
        };
        let write_errno = if written < 0 {
            std::io::Error::last_os_error().raw_os_error().unwrap_or(-1)
        } else {
            0
        };
        let value = std::fs::read_to_string(format!("/proc/self/fd/{fd}")).unwrap_or_default();
        // Read straight off the inherited descriptor too. `/proc/self/fd/<n>` opens a *new* file
        // description at offset zero, so it would happily pass a slot handed over positioned at
        // EOF; this is the read an ordinary child does.
        let mut inherited = [0_u8; 4096];
        // SAFETY: a bounded read into a local buffer from an inherited descriptor.
        let read = unsafe {
            libc::read(
                fd,
                inherited.as_mut_ptr().cast::<libc::c_void>(),
                inherited.len(),
            )
        };
        let inherited = if read > 0 {
            String::from_utf8_lossy(&inherited[..usize::try_from(read).unwrap_or(0)]).into_owned()
        } else {
            String::new()
        };
        let link = std::fs::read_link(format!("/proc/self/fd/{fd}"))
            .map(|path| path.display().to_string())
            .unwrap_or_default();
        // The listing's own directory descriptor is not an inherited one; it links back at
        // `/proc/<pid>/fd` and is the only entry excluded here.
        let mut descriptors: Vec<u32> = std::fs::read_dir("/proc/self/fd")
            .expect("the child can list its descriptors")
            .filter_map(|entry| {
                let entry = entry.ok()?;
                let target = std::fs::read_link(entry.path()).ok()?;
                (!target.ends_with("fd")).then_some(())?;
                entry.file_name().to_str()?.parse().ok()
            })
            .collect();
        descriptors.sort_unstable();
        let descriptors: Vec<String> = descriptors.iter().map(u32::to_string).collect();
        let mut release = String::new();
        std::io::stdin()
            .read_line(&mut release)
            .expect("wait for the parent");
        println!(
            "{CHILD_LINE} seals={seals} write_errno={write_errno} value_sha256={} inherited_sha256={} link={link} fds={}",
            digest(&value),
            digest(&inherited),
            descriptors.join(","),
        );
    }

    /// No surface substrate produces carries the material — argv, environment, request, ledger
    /// hash input, applied record, child output or refusal message.
    #[test]
    fn secret_slot_value_absent_from_argv_env_events_and_ledger() {
        let directory = tempfile::tempdir().expect("scratch");
        let declarations = vec![declare(directory.path(), "vendor_api_key", SENTINEL)];
        let requests = vec![request("vendor_api_key", 7)];
        let slots = SecretSlotSet::acquire(&declarations, &requests).expect("acquire the slot");

        let input = exec_input(requests.clone());
        let body = serde_json::to_value(&input).expect("serialize the request");
        let request_json = serde_json::to_string(&body).expect("request bytes");
        assert!(
            !request_json.contains(SENTINEL),
            "the request the ledger hashes carries the material"
        );
        let hash = canonical_request_hash_v2("POST", "/v1/execs", &body, None).expect("hash");
        assert!(!hash.contains(SENTINEL));

        let applied = AppliedConfinement {
            capability_snapshot: format!("sha256:{}", "7".repeat(64)),
            cgroup: "substrate-ex_test".to_owned(),
            filesystem: AppliedFilesystem::WorkspaceReadWriteSystemReadOnly,
            network: AppliedNetwork::None,
            profile: SandboxProfile::Workspace,
            capsule: None,
            read_only_roots: Vec::new(),
            secret_slots: requests.clone(),
        };
        let applied_json = serde_json::to_string(&applied).expect("applied record");
        assert!(
            !applied_json.contains(SENTINEL),
            "the record an event carries names the material"
        );
        assert!(applied_json.contains("vendor_api_key"));

        assert_eq!(
            slots.environment().as_deref(),
            Some("vendor_api_key=7"),
            "the shaped environment carries the mapping and nothing else"
        );

        let child = spawn_child(&slots, 7);
        let pid = child.id();
        let report = ChildReport::collect(child, pid);
        assert!(
            !report.cmdline.contains(SENTINEL),
            "argv carries the material"
        );
        assert!(
            !report.environ.contains(SENTINEL),
            "the child environment carries the material"
        );
        assert!(!report.stdout.contains(SENTINEL));
        assert_eq!(
            report.value_sha256,
            digest(SENTINEL),
            "the child did not read the declared bytes from its descriptor"
        );
        assert_eq!(
            report.inherited_sha256,
            digest(SENTINEL),
            "the descriptor was handed over positioned past the material"
        );

        let unknown = SecretSlotSet::acquire(&declarations, &[request("absent_slot", 7)])
            .expect_err("an undeclared slot refuses");
        assert!(!unknown.message.contains(SENTINEL));
        assert!(unknown.message.contains("absent_slot"));
    }

    /// The seal set is exactly the declared one, read back by the child itself.
    #[test]
    fn secret_slot_memfd_is_sealed() {
        let directory = tempfile::tempdir().expect("scratch");
        let declarations = vec![declare(directory.path(), "vendor_api_key", SENTINEL)];
        let slots = SecretSlotSet::acquire(&declarations, &[request("vendor_api_key", 9)])
            .expect("acquire the slot");
        let child = spawn_child(&slots, 9);
        let pid = child.id();
        let report = ChildReport::collect(child, pid);
        assert_eq!(
            report.seals,
            libc::F_SEAL_WRITE | libc::F_SEAL_SHRINK | libc::F_SEAL_GROW | libc::F_SEAL_SEAL,
            "the child reads back a seal set that is not the declared one"
        );
        assert_eq!(report.seals, SEAL_SET);
        assert_eq!(report.seals, 0xf);
        assert_eq!(
            report.write_errno,
            libc::EPERM,
            "a sealed slot accepted a write"
        );
        assert!(
            report.link.contains("memfd:substrate-slot-vendor_api_key"),
            "the descriptor is not the named anonymous memfd: {}",
            report.link
        );
        assert_eq!(
            report.descriptors,
            vec![0, 1, 2, 9],
            "the child holds descriptors beyond stdio and its declared slot"
        );
    }

    /// No weaker delivery: absent the proof, the fact is absent and a start naming a slot is
    /// unserved.
    #[test]
    fn secret_slot_refused_when_sealing_unavailable() {
        let directory = tempfile::tempdir().expect("scratch");
        let declarations = vec![declare(directory.path(), "vendor_api_key", SENTINEL)];

        assert_eq!(
            secret_slots_fact(&declarations, false, true),
            None,
            "sealing unprovable must publish no fact"
        );
        assert_eq!(
            secret_slots_fact(&declarations, true, false),
            None,
            "descriptor pass-through unprovable must publish no fact"
        );
        assert_eq!(
            secret_slots_fact(&[], true, true),
            None,
            "a daemon with no declared slot publishes no fact"
        );
        assert_eq!(
            secret_slots_fact(&declarations, true, true),
            Some(vec!["vendor_api_key".to_owned()]),
        );

        let mut config = HostConfig::minimum(directory.path());
        config.secret_slots = declarations;
        let unserved = crate::process::ProcessRuntime::admit_secret_slots(
            &config,
            None,
            &[request("vendor_api_key", 7)],
        )
        .expect_err("a slot without the capability is unserved");
        assert_eq!(unserved.class, DriverErrorClass::Unserved);
        assert_eq!(unserved.code, "exec.secret-slots-unserved");
        assert_eq!(unserved.address.as_deref(), Some("secret_slots"));
    }

    /// After spawn the only holder is the child.
    ///
    /// The slot name is this test's alone. `/proc/self/fd` is process-wide and the suite runs its
    /// cases in threads of one process, so scanning for *any* slot memfd would read a neighbouring
    /// case's descriptor and call it a leak.
    #[test]
    fn daemon_closes_its_copy_after_spawn() {
        let directory = tempfile::tempdir().expect("scratch");
        let slot = "closed_after_spawn";
        let declarations = vec![declare(directory.path(), slot, SENTINEL)];
        let slots =
            SecretSlotSet::acquire(&declarations, &[request(slot, 7)]).expect("acquire the slot");
        assert!(
            self_holds_a_slot_memfd(slot),
            "the acquired slot is not held before spawn"
        );
        let child = spawn_child(&slots, 7);
        let pid = child.id();
        drop(slots);
        assert!(
            !self_holds_a_slot_memfd(slot),
            "/proc/{}/fd still holds a slot memfd after spawn",
            std::process::id()
        );
        let report = ChildReport::collect(child, pid);
        assert_eq!(
            report.value_sha256,
            digest(SENTINEL),
            "the child could not read the value the daemon had already let go of"
        );
    }

    fn self_holds_a_slot_memfd(slot: &str) -> bool {
        let wanted = format!("memfd:substrate-slot-{slot}");
        std::fs::read_dir("/proc/self/fd")
            .expect("this process can list its descriptors")
            .filter_map(Result::ok)
            .any(|entry| {
                std::fs::read_link(entry.path())
                    .is_ok_and(|target| target.to_string_lossy().contains(&wanted))
            })
    }

    /// The ledger hash covers slot names only: rotating the material changes no request byte.
    #[test]
    fn ledger_request_hash_covers_slot_names_only() {
        let directory = tempfile::tempdir().expect("scratch");
        let before = vec![declare(directory.path(), "vendor_api_key", SENTINEL)];
        let requests = vec![request("vendor_api_key", 7)];
        let first = SecretSlotSet::acquire(&before, &requests).expect("acquire before rotation");
        let body = serde_json::to_value(exec_input(requests.clone())).expect("request");
        let hash_before =
            canonical_request_hash_v2("POST", "/v1/execs", &body, None).expect("hash before");

        let after = vec![declare(
            directory.path(),
            "vendor_api_key",
            "a-completely-other-value",
        )];
        let second = SecretSlotSet::acquire(&after, &requests).expect("acquire after rotation");
        let rotated = serde_json::to_value(exec_input(requests)).expect("request");
        let hash_after =
            canonical_request_hash_v2("POST", "/v1/execs", &rotated, None).expect("hash after");

        assert_eq!(body, rotated, "rotation changed a request byte");
        assert_eq!(
            hash_before, hash_after,
            "rotating the material invalidated an admitted operation"
        );
        assert_ne!(
            first.acquired_digest(),
            second.acquired_digest(),
            "the two runs delivered the same bytes, so the test proves nothing"
        );
        assert_eq!(first.environment(), second.environment());
        assert!(!hash_before.contains(SENTINEL));
    }

    /// Descriptors the wire admits, and the ones it refuses by name.
    #[test]
    fn slot_descriptors_are_bounded_and_distinct() {
        let directory = tempfile::tempdir().expect("scratch");
        let declarations = vec![declare(directory.path(), "vendor_api_key", SENTINEL)];
        for illegal in [0_u32, 1, 2, 64, 4096] {
            let error =
                SecretSlotSet::acquire(&declarations, &[request("vendor_api_key", illegal)])
                    .expect_err("an out-of-range descriptor refuses");
            assert_eq!(
                error.code, "exec.secret-slot-descriptor-invalid",
                "fd {illegal}"
            );
        }
        let repeated = SecretSlotSet::acquire(
            &declarations,
            &[request("vendor_api_key", 7), request("vendor_api_key", 7)],
        )
        .expect_err("a repeated descriptor refuses");
        assert_eq!(repeated.code, "exec.secret-slot-descriptor-invalid");
    }

    /// The `close_range` generalisation reduces to today's two windows when nothing is declared.
    #[test]
    fn the_retained_set_reduces_to_stdio_and_the_barrier() {
        let directory = tempfile::tempdir().expect("scratch");
        let declarations = vec![declare(directory.path(), "vendor_api_key", SENTINEL)];
        let empty = SecretSlotSet::acquire(&declarations, &[]).expect("no slot is not an error");
        assert_eq!(empty.retained(Some(5)), vec![0, 1, 2, 5]);
        assert_eq!(super::gaps(&[0, 1, 2, 5]), vec![(3, 4), (6, u32::MAX)]);
        assert_eq!(super::gaps(&[0, 1, 2, 3]), vec![(4, u32::MAX)]);
        assert_eq!(
            super::gaps(&[0, 1, 2, 7, 9, 63]),
            vec![(3, 6), (8, 8), (10, 62), (64, u32::MAX)]
        );

        let slots = SecretSlotSet::acquire(&declarations, &[request("vendor_api_key", 7)])
            .expect("acquire");
        assert_eq!(slots.retained(Some(5)), vec![0, 1, 2, 5, 7]);
        // Staged sources live above the declared ceiling, so a declaration can never collide.
        for (source, _) in slots.placements() {
            assert!(source > 63, "a staged source is inside the declared range");
        }
    }
}
