//! The executable gate on the driver's side (design 35).
//!
//! `HostConfig.exec_gate` names `substrate-exec-gate`, the helper that holds a run to its
//! executable allow-list. This module decides whether that file can be the authority (D21), reads
//! it once and seals its bytes in memory (D20), and turns one start's lists into the helper's
//! arguments, refusing an entry the view cannot carry before anything is dispatched (D2, D3,
//! D24). The deny-list (§ 8) goes the same way: each entry is located as an allow entry is,
//! except that it need not exist, and a start whose program lies in a denied path, or is the
//! same file as a denied one, is refused before dispatch (D23).
//!
//! What the open answers, in the order it asks (D27):
//!
//! | step | situation at open | answer |
//! |---|---|---|
//! | | `exec_gate` is not set | no gate: the adapter proves no executable-list fact (D19) |
//! | 1 | not absolute | `config.exec-gate-invalid`, 422 |
//! | 2 | missing, unreadable, not a regular file or over the size bound | `config.exec-gate-unserved`, 501 |
//! | 3 | a place the open needs cannot be established, for a cause that refuses (D26) | `config.exec-gate-unserved`, 501, with the cause and the path |
//! | 3 | every such cause is `file-system-unlisted` or `mount-id-unreported` | **the adapter opens and the facts are withheld** (D27): the helper is not sealed, bound or executed, and no probe runs |
//! | 4 | in the workspace root or a declared writable root, or a file with more than one link | `config.exec-gate-invalid`, 422 |
//! | 5 | not a statically linked ELF image of the driver's own class, byte order and machine that the kernel starts: no such image, a header the kernel's loader refuses, a program header table not read whole, or a `PT_INTERP` (D29) | `config.exec-gate-invalid`, 422 |
//! | 6 | not of the driver's protocol | `config.exec-gate-unserved`, 501 |
//!
//! **Step 5 is asked of the bytes that are then sealed and executed**, read from the one
//! descriptor of the open, so no file can be swapped between the check and a start. A helper
//! with a `PT_INTERP` is started by a loader, which runs before the helper's first instruction
//! and reads files a profile can mount into the view (`/etc/ld.so.preload`, the loader's
//! cache, a library): an object named there would run before the ruleset exists. A static-pie
//! image has a `PT_DYNAMIC` header and no `PT_INTERP`, and passes; `PT_DYNAMIC` is not asked.
//! Bytes that are no ELF image of this machine, a `#!` script among them, and an image whose
//! header table was not read whole are refused as well: what starts first there is another
//! program, or is not known. So is an image the kernel's loader answers `ENOEXEC` for: one whose
//! type is neither `ET_EXEC` nor `ET_DYN`, whose `e_phentsize` is not the program header size
//! of its class, or which declares no program header or more than one 4 KiB page of them
//! (a kernel with larger pages reads more; such an image is refused on every host). Bubblewrap
//! starts the helper with `execvp`, which answers `ENOEXEC` by running `/bin/sh` with the file
//! as its script: a dynamically linked program of the view, before any ruleset.
//!
//! After the open the file is never read again: every start binds the sealed bytes, so a file
//! replaced or removed on disk changes nothing until the adapter is opened again.
//!
//! **Whether one path lies in another is decided by place, never by text** (D25): the mount the
//! kernel reports for the opened path, in the driver's own mount table, through
//! [`substrate_exec_gate::place`], the source file the helper uses on the view's table. Where a
//! place cannot be established there is a named cause, one of nine (D26):
//! `mount-table-unreadable`, `mount-table-unparsable`, `path-unopenable`, `mount-id-unreported`,
//! `mount-row-missing`, `mount-row-repeated`, `mount-root-deleted`, `mount-point-mismatch` and
//! `file-system-unlisted`, the last for a file-system type other than `ext4`, `overlay` and
//! `tmpfs`. A start answers [`PLACE_UNSERVED`] for each of them. The open places every path
//! before it answers ([`open_places`] returns what could not be placed as data): one cause that
//! refuses anywhere is [`EXEC_GATE_UNSERVED`]; with none, every path whose cause is
//! `file-system-unlisted` or `mount-id-unreported` becomes an entry of the adapter's proof
//! ([`FactWithheld`]), and a profile with a list is refused `confinement.fact-unserved` with it.
//!
//! A start without a list reaches nothing here: no table is read and no place is asked.

use std::collections::VecDeque;
use std::ffi::CString;
use std::io::{self, Read as _, Write as _};
use std::os::fd::{AsRawFd as _, FromRawFd as _, OwnedFd, RawFd};
use std::os::unix::fs::{MetadataExt as _, OpenOptionsExt as _};
use std::path::Path;

use sha2::{Digest as _, Sha256};
use substrate_exec_gate::place::{
    Cause, LISTED_FILE_SYSTEMS, MountTable, MountTree, Opened, Place, Reach, Unestablished,
    Unlisted,
};
use substrate_exec_gate::{Class, Linkage, Lists, Located, PROTOCOL};

use crate::confinement::{Fact, FactWithheld};
use crate::read_only_files::PinnedFile;
use crate::read_only_roots::PinnedRoot;
use crate::seatbelt::{EXEC_SUBPATH_UNDECLARED, PATH_UNREPRESENTABLE, PROGRAM_UNLISTED};
use crate::writable_roots::PinnedWritableRoot;
use crate::{DriverError, HostConfig};

/// Where the helper's bytes are bound in a run's view: one fixed path outside every location an
/// entry may name. It is bound after every other mount, so no declared mount shadows it.
pub(crate) const GATE_MOUNT: &str = "/.substrate-exec-gate";

/// `HostConfig.exec_gate` cannot be the authority over what a run executes (D21).
pub(crate) const EXEC_GATE_INVALID: &str = "config.exec-gate-invalid";
/// `HostConfig.exec_gate` is set and this host cannot use it (D19, D26): the file is missing,
/// unreadable, not a regular file, over the size bound or not of the driver's protocol, or a
/// place the open needs cannot be established. The message then names the path, or the mount
/// point of a mount beneath a writable location (D30), and one of eight causes:
/// `mount-table-unreadable`, `mount-table-unparsable`, `path-unopenable`, `mount-row-missing`,
/// `mount-row-repeated`, `mount-root-deleted`, `mount-point-mismatch` or `mount-covered`. The
/// other two of the ten, `file-system-unlisted` (a file-system type other than `ext4`,
/// `overlay` and `tmpfs`) and `mount-id-unreported`, do not refuse the open: they withhold the
/// facts (D27, [`FactWithheld`]).
pub(crate) const EXEC_GATE_UNSERVED: &str = "config.exec-gate-unserved";
/// A start whose profile carries an executable list needs a place this host cannot establish
/// (D26): of the program, of an entry, or of the host source of a declared mount. 501, before
/// dispatch, and never for a start without a list. The message names the path, or the mount
/// point of a mount beneath a writable location or a declared mount's host source (D30), and
/// one of the ten causes: `mount-table-unreadable`, `mount-table-unparsable`,
/// `path-unopenable`, `mount-id-unreported`, `mount-row-missing`, `mount-row-repeated`,
/// `mount-root-deleted`, `mount-point-mismatch`, `mount-covered`, or `file-system-unlisted`
/// with the type, for a file-system type other than `ext4`, `overlay` and `tmpfs`. The
/// address is the field at fault.
pub(crate) const PLACE_UNSERVED: &str = "exec.sandbox-place-unserved";

const ADDRESS: &str = "config.exec_gate";
const WORKSPACE_ROOT: &str = "config.workspace_root";
const WRITABLE_ROOTS: &str = "config.writable_roots";
/// The address of a refused allow-list entry.
const ENTRY: &str = "exec_subpaths";
/// The address of a refused deny-list entry.
const DENIED: &str = "exec_denied_paths";
/// The address of a refused program.
const PROGRAM: &str = "argv";
/// The address of a refused mount a run can write.
const WRITABLE_MOUNT: &str = "writable_roots";
/// The address of a start whose workspace has no place.
const WORKSPACE: &str = "workspace";

/// The facts the helper proves, in the `Fact` order: what an open that cannot use it withholds
/// (D27). A fact the helper comes to prove is added here, and to nothing else of the open.
const GATED_FACTS: [Fact; 2] = [Fact::ExecSubpaths, Fact::ExecDeniedPaths];

/// The base system directories of every view, bound from the host's own at the same paths.
const BASE_MOUNTS: [&str; 4] = ["/usr", "/bin", "/lib", "/lib64"];
/// What the base view carries of `/etc` (ADR 0049 decision 1). A link of a base directory may
/// resolve through it; no entry may name it.
const ALTERNATIVES_MOUNT: &str = "/etc/alternatives";
/// The mounts of every view a run can write that bind no host directory.
const TMP_MOUNT: &str = "/tmp";
const DEV_MOUNT: &str = "/dev";
/// The kernel's own tree: nothing in it is a file of a mount an entry may name.
const PROC_MOUNT: &str = "/proc";
/// As many symbolic links as the kernel follows in one resolution.
const MAX_LINKS: usize = 40;
/// `PATH_MAX`: the longest target a symbolic link has.
const LINK_BYTES: usize = 4096;

/// The seals on the helper's bytes: no write, no resize, and no further seal change (D20).
const SEALS: libc::c_int =
    libc::F_SEAL_WRITE | libc::F_SEAL_GROW | libc::F_SEAL_SHRINK | libc::F_SEAL_SEAL;

/// `struct open_how` of `openat2(2)`.
#[repr(C)]
struct OpenHow {
    flags: u64,
    mode: u64,
    resolve: u64,
}

const RESOLVE_NO_MAGICLINKS: u64 = 0x02;
const RESOLVE_NO_SYMLINKS: u64 = 0x04;
const RESOLVE_BENEATH: u64 = 0x08;

fn invalid(path: &Path, why: impl std::fmt::Display) -> DriverError {
    DriverError::refused(
        EXEC_GATE_INVALID,
        format!("The executable gate {} {why}.", path.display()),
        ADDRESS,
    )
}

fn unserved(path: &Path, why: impl std::fmt::Display) -> DriverError {
    DriverError::unserved(
        EXEC_GATE_UNSERVED,
        format!("The executable gate {} {why}.", path.display()),
        ADDRESS,
    )
}

fn unrepresentable(text: &str, why: impl std::fmt::Display, address: &str) -> DriverError {
    DriverError::refused(PATH_UNREPRESENTABLE, format!("{text:?} {why}"), address)
}

/// The path and the cause, and for a file-system type outside the list the types in it.
fn describe(why: &Unestablished) -> String {
    match why.cause {
        Cause::FileSystemUnlisted(_) => format!(
            "{why}; a place is established on {} only",
            LISTED_FILE_SYSTEMS.join(", ")
        ),
        _ => why.to_string(),
    }
}

/// [`PLACE_UNSERVED`] for the field at `address`: the host cannot serve a path that is well
/// formed, so the class is `Unserved`, as `exec.sandbox-unavailable` is.
pub(crate) fn start_unplaced(address: &str, why: &Unestablished) -> DriverError {
    DriverError::unserved(
        PLACE_UNSERVED,
        format!(
            "This host cannot hold the run to its executable list: {}.",
            describe(why)
        ),
        address,
    )
}

/// [`start_unplaced`] for an entry of the deny-list, or its nearest existing ancestor: address
/// `exec_denied_paths`.
pub(crate) fn denied_unplaced(why: &Unestablished) -> DriverError {
    start_unplaced(DENIED, why)
}

/// A place the open needs and could not establish, as data (D26): whose configuration field
/// the path is, the path, and the cause, which carries the file-system type where that is the
/// cause. The caller decides what the open answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OpenUnplaced {
    /// `config.exec_gate`, `config.workspace_root` or `config.writable_roots`, by whose path it
    /// was; `config.exec_gate` for a base directory, the probes' temporary directory and the
    /// mount table.
    pub(crate) address: &'static str,
    pub(crate) why: Unestablished,
}

impl OpenUnplaced {
    /// The mount table itself has a row whose attachment cannot be established (D30): the
    /// helper's own field, as for a table that cannot be read.
    pub(crate) const fn table(why: Unestablished) -> Self {
        Self {
            address: ADDRESS,
            why,
        }
    }

    /// The refusal of the open: [`EXEC_GATE_UNSERVED`], with the cause and the path.
    pub(crate) fn refusal(&self, gate: &Path) -> DriverError {
        DriverError::unserved(
            EXEC_GATE_UNSERVED,
            format!(
                "The executable gate {} cannot be used on this host: {}.",
                gate.display(),
                describe(&self.why)
            ),
            self.address,
        )
    }

    /// Whether the cause withholds the facts and lets the adapter open (D27): the path lies on
    /// a file-system type outside the list, or the kernel reports no mount ID for it. Each of
    /// the other eight causes says the driver could not read the host's table, a configured
    /// path, or what is mounted beneath one, and refuses the open.
    pub(crate) fn withholds(&self) -> bool {
        matches!(
            self.why.cause,
            Cause::FileSystemUnlisted(_) | Cause::MountIdUnreported
        )
    }

    /// The entry of the adapter's proof that says why `fact` is withheld.
    fn withheld(&self, fact: Fact) -> FactWithheld {
        FactWithheld {
            fact,
            cause: self.why.cause.name(),
            address: self.address,
            path: self.why.path.clone(),
            file_system: match &self.why.cause {
                Cause::FileSystemUnlisted(file_system) => Some(file_system.clone()),
                _ => None,
            },
        }
    }
}

/// The entries of the adapter's proof for an open that withholds (D27): for each path in the
/// open's order, one per fact the helper would have proved.
pub(crate) fn withheld_facts(unplaced: &[OpenUnplaced]) -> Vec<FactWithheld> {
    unplaced
        .iter()
        .flat_map(|unplaced| {
            GATED_FACTS
                .iter()
                .map(|fact| unplaced.withheld(fact.clone()))
        })
        .collect()
}

/// The helper as the driver holds it after the open: its bytes sealed in memory, their SHA-256,
/// and the host's ELF loader.
#[derive(Debug)]
pub(crate) struct ExecGate {
    memory: OwnedFd,
    sha256: String,
    loader: Option<String>,
}

impl ExecGate {
    /// Copies `bytes` into a memfd, seals it and hashes the sealed bytes.
    pub(crate) fn seal(bytes: &[u8], loader: Option<String>) -> io::Result<Self> {
        // SAFETY: `memfd_create` with a NUL-terminated literal name and plain flags.
        let descriptor = unsafe {
            libc::memfd_create(
                c"substrate-exec-gate".as_ptr(),
                libc::MFD_CLOEXEC | libc::MFD_ALLOW_SEALING,
            )
        };
        if descriptor < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `memfd_create` returned a descriptor nothing else owns.
        let memory = unsafe { OwnedFd::from_raw_fd(descriptor) };
        let mut file = std::fs::File::from(memory);
        file.write_all(bytes)?;
        let memory = OwnedFd::from(file);
        // SAFETY: `fcntl` adds seals to a live memfd.
        if unsafe { libc::fcntl(memory.as_raw_fd(), libc::F_ADD_SEALS, SEALS) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let mut gate = Self {
            memory,
            sha256: String::new(),
            loader,
        };
        // The seals as the kernel holds them, not the return value above.
        if gate.seals() & SEALS != SEALS {
            return Err(io::Error::other("the memfd does not carry the seals"));
        }
        let mut sealed = Vec::with_capacity(bytes.len());
        std::fs::File::from(gate.reopen()?).read_to_end(&mut sealed)?;
        gate.sha256 = hex::encode(Sha256::digest(&sealed));
        Ok(gate)
    }

    /// The seals the kernel holds on the bytes.
    pub(crate) fn seals(&self) -> libc::c_int {
        // SAFETY: `fcntl` reads the seals of a live memfd.
        unsafe { libc::fcntl(self.memory.as_raw_fd(), libc::F_GET_SEALS) }
    }

    /// A fresh read-only descriptor of the sealed bytes, positioned at their start: what one
    /// sandbox's `--ro-bind-data` reads to its end, so no two sandboxes share one (D20, K3).
    /// Close-on-exec; the spawn that needs it clears the flag.
    pub(crate) fn reopen(&self) -> io::Result<OwnedFd> {
        std::fs::File::open(format!("/proc/self/fd/{}", self.memory.as_raw_fd())).map(OwnedFd::from)
    }

    /// The SHA-256 of the sealed bytes, which the backend binding carries.
    pub(crate) fn sha256(&self) -> &str {
        &self.sha256
    }

    /// The host's ELF loader, read once at open from `/usr/bin/env` (D7).
    pub(crate) fn loader(&self) -> Option<&str> {
        self.loader.as_deref()
    }
}

/// The bubblewrap options that bind the helper's bytes, read from `descriptor`, at
/// [`GATE_MOUNT`]: a read-only file of mode 0555 that is not the host's file.
pub(crate) fn bind_args(descriptor: RawFd) -> [String; 5] {
    [
        "--perms".to_owned(),
        "0555".to_owned(),
        "--ro-bind-data".to_owned(),
        descriptor.to_string(),
        GATE_MOUNT.to_owned(),
    ]
}

/// A host location a run can write, and what it reaches (D30): its own place first, then the
/// root of every mount that is mounted beneath it. What is mounted beneath a location a run
/// can write is carried into the run's view with it and is the run's to write, although its
/// place is on another file system.
#[derive(Debug, Clone)]
pub(crate) struct Location {
    pub(crate) path: String,
    pub(crate) reaches: Reach,
}

impl Location {
    /// The location at `path`, whose place is `place`, with what it reaches on `tree`: the
    /// side that fails closed, because a mount missing here would be a writable place called
    /// read-only. A mount beneath it of a file-system type outside the list is not compared
    /// as a place: whatever is asked against a writable place is placed first, a path's end
    /// and every symbolic link it reads, and has no place on such a type.
    fn reaching(path: &str, place: &Place, tree: &MountTree) -> Result<Self, Unestablished> {
        Ok(Self {
            path: path.to_owned(),
            reaches: tree.reaches(place, Unlisted::Skipped)?,
        })
    }
}

/// What the kernel reports for a configured path: opened, with its symbolic links followed, or
/// its nearest existing ancestor (D25). Both sides of every comparison are resolved this way.
fn opened_path(path: &Path) -> Result<Opened, Unestablished> {
    let unopenable = || Unestablished {
        path: path.display().to_string(),
        cause: Cause::PathUnopenable,
        asked: substrate_exec_gate::place::Asked::Place,
    };
    // The configured text as it is, made absolute and not normalized: `std::path::absolute`
    // would remove a `.` as text, and a `.` or `..` among names that do not exist yet is the
    // place function's to refuse (D25).
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|_| unopenable())?
            .join(path)
    };
    Opened::of_path(absolute.to_str().ok_or_else(unopenable)?)
}

/// What the open asks the place of, as the kernel reported each (D26).
#[derive(Debug, Clone)]
pub(crate) struct OpenSightings {
    /// The helper's file: the opened descriptor.
    pub(crate) gate: Opened,
    /// `HostConfig.workspace_root`.
    pub(crate) workspace_root: Opened,
    /// Each `HostConfig.writable_roots` path.
    pub(crate) writable_roots: Vec<Opened>,
    /// The base directories the host has, of `/usr`, `/bin`, `/lib` and `/lib64`.
    pub(crate) base: Vec<Opened>,
    /// The directory the probes build their workspace in.
    pub(crate) temporary: Opened,
}

impl OpenSightings {
    fn of(config: &HostConfig, gate: &Opened) -> Result<Self, OpenUnplaced> {
        let at = |address: &'static str| move |why: Unestablished| OpenUnplaced { address, why };
        Ok(Self {
            gate: gate.clone(),
            workspace_root: opened_path(&config.workspace_root).map_err(at(WORKSPACE_ROOT))?,
            writable_roots: config
                .writable_roots
                .iter()
                .map(|root| opened_path(&root.path).map_err(at(WRITABLE_ROOTS)))
                .collect::<Result<_, _>>()?,
            base: BASE_MOUNTS
                .iter()
                .filter(|path| Path::new(path).exists())
                .map(|path| Opened::of_path(path).map_err(at(ADDRESS)))
                .collect::<Result<_, _>>()?,
            temporary: opened_path(&std::env::temp_dir()).map_err(at(ADDRESS))?,
        })
    }
}

/// The places the open established: the helper's, and those of the locations a run can write.
#[derive(Debug, Clone)]
pub(crate) struct OpenPlaces {
    pub(crate) gate: Place,
    pub(crate) writable: Vec<Location>,
}

/// What placing every path of the open came to (D26, D27).
#[derive(Debug, Clone)]
pub(crate) enum OpenPlacement {
    /// Every place was established.
    Placed(OpenPlaces),
    /// No cause refuses, and these paths have one that withholds the facts: every such path,
    /// in the open's order, not only the first.
    Withheld(Vec<OpenUnplaced>),
    /// A path has a cause that refuses the open: the first such path in the open's order,
    /// whatever the paths before it withhold.
    Refused(OpenUnplaced),
}

/// Establishes every place the open needs on `tree` (D26): the helper's file, the workspace
/// root, each declared writable root, the base directories and the probes' temporary
/// directory, in that order; and for each location a run can write the places it reaches
/// (D30). **Every path is placed before the answer** (D27), and what cannot be established is
/// returned as data: the configuration field, the path or the mount point, and the cause with
/// the file-system type where that is the cause.
pub(crate) fn place_open(sightings: &OpenSightings, tree: &MountTree) -> OpenPlacement {
    let mut unplaced = Vec::new();
    let mut place = |address: &'static str, opened: &Opened| match tree.table().place_of(opened) {
        Ok(place) => Some(place),
        Err(why) => {
            unplaced.push(OpenUnplaced { address, why });
            None
        }
    };
    let gate = place(ADDRESS, &sightings.gate);
    let mut placed = Vec::new();
    for (address, root) in std::iter::once((WORKSPACE_ROOT, &sightings.workspace_root)).chain(
        sightings
            .writable_roots
            .iter()
            .map(|root| (WRITABLE_ROOTS, root)),
    ) {
        if let Some(place) = place(address, root) {
            placed.push((address, root, place));
        }
    }
    for directory in sightings
        .base
        .iter()
        .chain(std::iter::once(&sightings.temporary))
    {
        place(ADDRESS, directory);
    }
    // What each location reaches, after every path was placed: a mount beneath one whose root
    // cannot be taken as a place refuses, with the location's own field as its address.
    let mut writable = Vec::new();
    for (address, root, place) in placed {
        match Location::reaching(&root.path, &place, tree) {
            Ok(location) => writable.push(location),
            Err(why) => unplaced.push(OpenUnplaced { address, why }),
        }
    }
    if let Some(refusing) = unplaced.iter().find(|unplaced| !unplaced.withholds()) {
        return OpenPlacement::Refused(refusing.clone());
    }
    match gate {
        Some(gate) if unplaced.is_empty() => OpenPlacement::Placed(OpenPlaces { gate, writable }),
        _ => OpenPlacement::Withheld(unplaced),
    }
}

/// **The open's place check, whole** (D26, D27): reads the driver's mount table, opens every
/// path the open asks the place of, and establishes each. What cannot be established comes back
/// as [`OpenUnplaced`] values: the configuration field, the path, and the cause with the
/// file-system type where that is the cause. Nothing is refused or withheld here;
/// [`decide_open`] gives the answer.
pub(crate) fn open_places(config: &HostConfig, gate: &Opened) -> OpenPlacement {
    // The table before any path: D26 names its causes first. Neither an unreadable table nor a
    // path that cannot be opened is a cause that withholds.
    let table = match MountTable::read() {
        Ok(table) => table,
        Err(cause) => {
            return OpenPlacement::Refused(OpenUnplaced {
                address: ADDRESS,
                why: Unestablished {
                    path: gate.path.clone(),
                    cause,
                    asked: substrate_exec_gate::place::Asked::Place,
                },
            });
        }
    };
    // The table as a tree (D30): a row whose attachment cannot be established may be mounted
    // beneath anything, so it refuses before any location is asked what it reaches.
    let tree = match table.into_tree() {
        Ok(tree) => tree,
        Err(why) => return OpenPlacement::Refused(OpenUnplaced::table(why)),
    };
    match OpenSightings::of(config, gate) {
        Ok(sightings) => place_open(&sightings, &tree),
        Err(unplaced) => OpenPlacement::Refused(unplaced),
    }
}

/// What the open reads of the helper's file itself, from its one descriptor.
#[derive(Debug, Clone, Copy)]
pub(crate) struct FileFacts {
    pub(crate) regular: bool,
    pub(crate) bytes: u64,
    pub(crate) links: u64,
}

/// What the open does with a helper whose file and places it has seen.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum OpenDecision {
    /// The helper can be the authority: its bytes are sealed and the probes run.
    Use,
    /// The adapter opens without the helper (D27): these are the entries of its proof.
    Withheld(Vec<FactWithheld>),
}

/// Steps 2 to 4 of the open, decided on what it is given (D27): the file's own facts, then the
/// places, then whether the file can be trusted. An open that withholds answers before the
/// trust check, because it never uses the helper: a helper with two links is refused on the
/// first host where the facts could be proved, and not on one where they are withheld.
///
/// # Errors
///
/// [`EXEC_GATE_UNSERVED`] for a file that is not regular or is over the size bound and for a
/// place with a cause that refuses; [`EXEC_GATE_INVALID`] for a file in a location a run can
/// write or with more than one link.
pub(crate) fn decide_open(
    path: &Path,
    file: FileFacts,
    placement: OpenPlacement,
) -> Result<OpenDecision, DriverError> {
    check_file(path, file.regular, file.bytes)?;
    let places = match placement {
        OpenPlacement::Placed(places) => places,
        OpenPlacement::Withheld(unplaced) => {
            return Ok(OpenDecision::Withheld(withheld_facts(&unplaced)));
        }
        OpenPlacement::Refused(unplaced) => return Err(unplaced.refusal(path)),
    };
    check_location(path, &places)?;
    check_links(path, file.links)?;
    Ok(OpenDecision::Use)
}

/// D21: a helper in a location a run can write could have been written by a run. Both places
/// come from opened paths, so a symbolic link in either configured path hides nothing, and a
/// workspace root the host mounts at a second path is found (D25).
///
/// # Errors
///
/// [`EXEC_GATE_INVALID`] when the file lies in a writable location.
pub(crate) fn check_location(path: &Path, places: &OpenPlaces) -> Result<(), DriverError> {
    match places
        .writable
        .iter()
        .find(|location| location.reaches.holds(&places.gate))
    {
        Some(location) => Err(invalid(
            path,
            format_args!("lies in {}, which a run can write", location.path),
        )),
        None => Ok(()),
    }
}

/// D19: the file must be a regular file within the size bound.
pub(crate) fn check_file(path: &Path, regular: bool, bytes: u64) -> Result<(), DriverError> {
    if !regular {
        return Err(unserved(path, "is not a regular file"));
    }
    if bytes > crate::probe::MAX_BACKEND_BINARY_BYTES {
        return Err(unserved(
            path,
            format_args!(
                "is {bytes} bytes, over the bound of {}",
                crate::probe::MAX_BACKEND_BINARY_BYTES
            ),
        ));
    }
    Ok(())
}

/// D21: a second link to the file may lie where a run can write, which no path check sees.
pub(crate) fn check_links(path: &Path, links: u64) -> Result<(), DriverError> {
    if links > 1 {
        return Err(invalid(
            path,
            format_args!(
                "has {links} links, and another name of it may lie where a run can write; \
                 install a copy"
            ),
        ));
    }
    Ok(())
}

/// D19: what the helper printed for `--version` in a throwaway sandbox must be the protocol line.
pub(crate) fn check_protocol(
    path: &Path,
    succeeded: bool,
    stdout: &[u8],
) -> Result<(), DriverError> {
    if succeeded && stdout == format!("{PROTOCOL}\n").as_bytes() {
        return Ok(());
    }
    Err(unserved(
        path,
        format_args!("does not answer --version with the protocol line {PROTOCOL} in a sandbox"),
    ))
}

/// The loader the host's base programs name: the `PT_INTERP` of `/usr/bin/env`, a base program
/// of every view. Never a loader read from a run's program (D7).
fn host_loader() -> Option<String> {
    let mut image = Vec::new();
    std::fs::File::open("/usr/bin/env")
        .ok()?
        .take(crate::probe::MAX_BACKEND_BINARY_BYTES)
        .read_to_end(&mut image)
        .ok()?;
    substrate_exec_gate::elf_interpreter(&image)
}

/// What `HostConfig.exec_gate` came to at the open.
#[derive(Debug)]
pub(crate) enum Opening {
    /// Not set: nothing was opened and no place was asked.
    Unset,
    /// Set, and the adapter opens without it (D27): the entries of its proof. The file was
    /// opened and its bytes were neither read, sealed nor executed.
    Withheld(Vec<FactWithheld>),
    /// Set, and read once and sealed.
    Gate(ExecGate),
}

impl Opening {
    /// The helper the driver holds, and what its proof says of an open that does not use the
    /// configured one. **Visible by default** (D27): each such entry is written once at `warn`,
    /// so the host's operator learns why the lists are not served without reading a log at
    /// debug level.
    pub(crate) fn announced(self) -> (Option<std::sync::Arc<ExecGate>>, Vec<FactWithheld>) {
        match self {
            Self::Unset => (None, Vec::new()),
            Self::Gate(gate) => (Some(std::sync::Arc::new(gate)), Vec::new()),
            Self::Withheld(entries) => {
                for entry in &entries {
                    tracing::warn!(
                        fact = ?entry.fact,
                        cause = entry.cause,
                        address = entry.address,
                        path = %entry.path,
                        file_system = entry.file_system.as_deref().unwrap_or("unreported"),
                        "executable lists withheld: the helper is configured and is not used \
                         on this host; a profile with a list is refused confinement.fact-unserved"
                    );
                }
                (None, entries)
            }
        }
    }
}

/// Checks `HostConfig.exec_gate` and reads it once (D19 to D21, D26, D27), in the order of the
/// module's table.
///
/// **The driver calls this before it creates `workspace_root`**, so a refusal leaves nothing
/// written. A root that does not exist yet has the place of its nearest existing ancestor
/// joined with the names after it; where one of those names is `..` or `.` there is no place
/// (D25, `path-unopenable`) and the open is refused [`EXEC_GATE_UNSERVED`]: the driver does not
/// create a directory beneath a path it has not checked in order to place it.
///
/// # Errors
///
/// [`EXEC_GATE_INVALID`] or [`EXEC_GATE_UNSERVED`], naming the path and what failed.
pub(crate) fn open(config: &HostConfig) -> Result<Opening, DriverError> {
    let Some(path) = config.exec_gate.as_deref() else {
        return Ok(Opening::Unset);
    };
    if !path.is_absolute() {
        return Err(invalid(path, "is not an absolute path"));
    }
    // One descriptor: every later fact about the file comes from it. Non-blocking, so a FIFO
    // under the name is answered as not a regular file instead of waiting for a writer.
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_NOCTTY)
        .open(path)
        .map_err(|error| unserved(path, format_args!("cannot be opened ({error})")))?;
    let metadata = file
        .metadata()
        .map_err(|error| unserved(path, format_args!("cannot be read ({error})")))?;
    let facts = FileFacts {
        regular: metadata.is_file(),
        bytes: metadata.len(),
        links: metadata.nlink(),
    };
    // Step 2 before any place is asked: what is under the name is not a helper.
    check_file(path, facts.regular, facts.bytes)?;
    let sighted = Opened::of_descriptor(file.as_raw_fd(), &path.display().to_string());
    // The one place the open decides what an unestablished place means (D26, D27).
    match decide_open(path, facts, open_places(config, &sighted))? {
        OpenDecision::Use => {}
        OpenDecision::Withheld(entries) => return Ok(Opening::Withheld(entries)),
    }
    let mut bytes = Vec::new();
    (&mut file)
        .take(crate::probe::MAX_BACKEND_BINARY_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| unserved(path, format_args!("cannot be read ({error})")))?;
    check_file(path, true, u64::try_from(bytes.len()).unwrap_or(u64::MAX))?;
    // Step 5, on the bytes about to be sealed: nothing is read from the name again.
    check_linkage(path, &bytes)?;
    ExecGate::seal(&bytes, host_loader())
        .map(Opening::Gate)
        .map_err(|error| {
            unserved(
                path,
                format_args!("could not be sealed in memory ({error})"),
            )
        })
}

/// D29, rule 1: the helper is the first program of a listed run's view, so nothing may run
/// before its first instruction. Decided on the bytes the driver will seal and execute, in this
/// order: they are an ELF image of the driver's own class, byte order and machine; the header
/// is one the kernel's loader takes (the type `ET_EXEC` or `ET_DYN`, a program header size of
/// the class, a header count the kernel reads); the whole program header table was read; and no
/// header has the type `PT_INTERP`. A script's interpreter, a registered handler of another
/// machine's images and a loader each run first, and for an image the kernel answers `ENOEXEC`
/// for, bubblewrap's `execvp` runs a shell of the view with the file as its script.
///
/// # Errors
///
/// [`EXEC_GATE_INVALID`], naming the path and which of the four it is.
pub(crate) fn check_linkage(path: &Path, image: &[u8]) -> Result<(), DriverError> {
    match substrate_exec_gate::elf_linkage(image) {
        Linkage::Static => Ok(()),
        Linkage::Interpreted => Err(invalid(
            path,
            "is dynamically linked (its program headers hold PT_INTERP): a loader would run \
             before the helper and read files a profile can mount, so the helper must be \
             statically linked",
        )),
        Linkage::Foreign => Err(invalid(
            path,
            "is not an ELF image of the driver's class, byte order and machine: the helper must \
             be a statically linked executable built for this host, and not a script",
        )),
        Linkage::Unstartable => Err(invalid(
            path,
            "is an ELF image the kernel does not start (its type is neither ET_EXEC nor ET_DYN, \
             its program header size is not its class's, or it declares no program header or \
             more than the kernel reads): a shell would run it as a script, so the helper must \
             be a statically linked executable",
        )),
        Linkage::Unreadable => Err(invalid(
            path,
            "is an ELF image whose program header table cannot be read whole: the helper must \
             be a statically linked executable",
        )),
    }
}

/// Refuses an entry of an executable list whose text no adapter can carry: the lexical rule
/// `macos` applies, so a profile one adapter accepts is one the other accepts (D3).
///
/// # Errors
///
/// `exec.sandbox-path-unrepresentable`, address `exec_subpaths`.
pub(crate) fn check_literal(entry: &str) -> Result<(), DriverError> {
    literal(entry, ENTRY)
}

/// [`check_literal`] for an entry of the deny-list.
///
/// # Errors
///
/// `exec.sandbox-path-unrepresentable`, address `exec_denied_paths`.
pub(crate) fn check_denied_literal(entry: &str) -> Result<(), DriverError> {
    literal(entry, DENIED)
}

fn literal(text: &str, address: &str) -> Result<(), DriverError> {
    let checked = if text.starts_with('/') {
        crate::seatbelt::absolute_literal(text)
    } else {
        crate::seatbelt::relative_literal(text)
    };
    checked.map_err(|error| DriverError::refused(PATH_UNREPRESENTABLE, error.message, address))
}

/// D24: the class of a mount follows the host source it binds, not the mode it is bound with in
/// this run. `beneath` when the source lies in a location a run can write, and when one lies in
/// it: the first is writable as a whole, the second shows that location through the mount.
///
/// Both are asked through the mounts (D30): `source` is what the host source reaches, and a
/// writable location is what it reaches. So a source on a mount beneath a writable location
/// is `beneath`, and so is a source that holds a mount on which one lies.
pub(crate) fn class_of(source: &Reach, writable: &[Location]) -> Class {
    if writable
        .iter()
        .any(|location| location.reaches.overlaps(source))
    {
        Class::Beneath
    } else {
        Class::Follow
    }
}

/// **The host source `source` as a location a run can write, where it is one** (D24, D30):
/// `None` for a source classed `follow`.
///
/// Whether it is one is read from the table alone, which never refuses: a mount beneath the
/// source that is covered, or whose mount point this user cannot open, is counted by the root
/// its row names. That can only class the source `beneath` where the kernel's answer would
/// have said `follow`, never the reverse, and a source that overlaps nothing writable starts,
/// whatever is mounted beneath it.
///
/// A source that does overlap a writable location is from then on a writable location
/// itself: its mount is one the helper is told the run can write, and the helper asks the
/// kernel about every mount beneath such a mount and fails closed. So the same question is
/// asked here, before dispatch, and the start is refused where it cannot be answered, in
/// place of a run the helper ends with status 125.
///
/// # Errors
///
/// [`PLACE_UNSERVED`], with `address` naming the field the mount was declared in: for a source
/// whose own place cannot be established, which is never classed `follow` for want of an
/// answer (D24, D26), and for a source classed `beneath` with a mount beneath it whose root
/// cannot be taken as a place.
pub(crate) fn writable_source(
    address: &str,
    source: &Opened,
    writable: &[Location],
    tree: &MountTree,
) -> Result<Option<Location>, DriverError> {
    let unplaced = |why: Unestablished| start_unplaced(address, &why);
    let place = tree.table().place_of(source).map_err(unplaced)?;
    if class_of(&tree.reaches_as_written(&place), writable) == Class::Follow {
        return Ok(None);
    }
    Location::reaching(&source.path, &place, tree)
        .map(Some)
        .map_err(unplaced)
}

/// The class of the host source the kernel reported as `source`, on `tree`
/// ([`writable_source`]).
///
/// # Errors
///
/// As [`writable_source`].
#[cfg(test)]
pub(crate) fn classed(
    address: &str,
    source: &Opened,
    writable: &[Location],
    tree: &MountTree,
) -> Result<Class, DriverError> {
    Ok(match writable_source(address, source, writable, tree)? {
        Some(_) => Class::Beneath,
        None => Class::Follow,
    })
}

/// The mounts of one run's view an executable path may lie in, as the start pinned them.
pub(crate) struct View<'a> {
    pub(crate) workspace: &'a OwnedFd,
    pub(crate) read_only_roots: &'a [PinnedRoot],
    pub(crate) read_only_files: &'a [PinnedFile],
    pub(crate) writable_roots: &'a [PinnedWritableRoot],
    /// Whether the view has `/scratch`.
    pub(crate) scratch: bool,
}

/// What a mount of the view binds.
enum Source<'a> {
    /// A descriptor the start pinned and will bind.
    Pinned(&'a OwnedFd),
    /// A base directory of the host, opened here at its canonical path.
    Host(OwnedFd),
    /// Nothing the driver can walk: a file system of the view's own.
    Opaque,
}

struct Mount<'a> {
    /// The absolute view path, and its names.
    path: String,
    names: Vec<String>,
    source: Source<'a>,
    /// A single file, with nothing beneath it.
    file: bool,
    class: Class,
    /// Whether an entry or the program may be written in it (design 35 § 5).
    eligible: bool,
    /// Whether the helper is told the run can write it (`--writable`).
    writable: bool,
}

impl Mount<'_> {
    fn descriptor(&self) -> Option<&OwnedFd> {
        match &self.source {
            Source::Pinned(descriptor) => Some(*descriptor),
            Source::Host(descriptor) => Some(descriptor),
            Source::Opaque => None,
        }
    }
}

fn names(path: &str) -> Vec<String> {
    path.split('/')
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .collect()
}

fn join(names: &[String]) -> String {
    format!("/{}", names.join("/"))
}

fn open_beneath(directory: &OwnedFd, rel: &str, flags: libc::c_int) -> io::Result<OwnedFd> {
    let rel = CString::new(rel).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
    let how = OpenHow {
        flags: u64::try_from(libc::O_PATH | libc::O_CLOEXEC | flags)
            .expect("open flags are positive"),
        mode: 0,
        resolve: RESOLVE_BENEATH | RESOLVE_NO_SYMLINKS | RESOLVE_NO_MAGICLINKS,
    };
    // SAFETY: `rel` is NUL-terminated, `how` is a live `open_how` of the size passed, and both
    // outlive the call.
    let descriptor = unsafe {
        libc::syscall(
            libc::SYS_openat2,
            directory.as_raw_fd(),
            rel.as_ptr(),
            &raw const how,
            std::mem::size_of::<OpenHow>(),
        )
    };
    if descriptor < 0 {
        return Err(io::Error::last_os_error());
    }
    let descriptor = RawFd::try_from(descriptor).expect("a descriptor fits its type");
    // SAFETY: `openat2` returned a descriptor nothing else owns.
    Ok(unsafe { OwnedFd::from_raw_fd(descriptor) })
}

/// The target of the symbolic link `descriptor` is, or `None` for any other file.
fn link_target(descriptor: &OwnedFd) -> io::Result<Option<String>> {
    // SAFETY: `fstat` fills the zeroed struct for a live descriptor.
    let mut status: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: as above; `status` is a valid out pointer.
    if unsafe { libc::fstat(descriptor.as_raw_fd(), &raw mut status) } != 0 {
        return Err(io::Error::last_os_error());
    }
    if status.st_mode & libc::S_IFMT != libc::S_IFLNK {
        return Ok(None);
    }
    let mut target = vec![0_u8; LINK_BYTES];
    // SAFETY: the empty path names the link the descriptor is; the buffer is live and its length
    // is the one passed.
    let length = unsafe {
        libc::readlinkat(
            descriptor.as_raw_fd(),
            c"".as_ptr(),
            target.as_mut_ptr().cast(),
            target.len(),
        )
    };
    let length = usize::try_from(length).map_err(|_| io::Error::last_os_error())?;
    target.truncate(length);
    String::from_utf8(target)
        .map(Some)
        .map_err(|_| io::Error::from(io::ErrorKind::InvalidData))
}

/// Why a path that did not open is refused: a symbolic link is named as one.
fn why_unopened(error: &io::Error) -> String {
    match error.raw_os_error() {
        Some(libc::ELOOP) => {
            "has a symbolic link in it, in a mount that shows something a run can write".to_owned()
        }
        Some(libc::ENOENT | libc::ENOTDIR) => "does not exist at the start".to_owned(),
        Some(libc::EXDEV) => "leaves the mount it lies in".to_owned(),
        _ => format!("cannot be opened ({error})"),
    }
}

/// Why a followed path has no end the driver can judge.
enum Unfollowed {
    /// The path itself is at fault: what follows the entry's text in the refusal.
    Refused(String),
    /// A name of the path does not exist in the mount it was looked up in. A fault of an
    /// entry of the allow-list and of the program; what an entry of the deny-list may be
    /// (design 35 § 8.2).
    Absent(String),
    /// A symbolic link the walk read, or a directory it passed through, has no place (D26,
    /// D30).
    Unplaced(Unestablished),
}

impl From<String> for Unfollowed {
    fn from(why: String) -> Self {
        Self::Refused(why)
    }
}

impl Unfollowed {
    fn other(why: String) -> Self {
        Self::Refused(why)
    }

    /// A name that did not open: absent where the kernel says it does not exist.
    fn unopened(why: String, error: &io::Error) -> Self {
        if matches!(error.raw_os_error(), Some(libc::ENOENT | libc::ENOTDIR)) {
            Self::Absent(why)
        } else {
            Self::Refused(why)
        }
    }

    /// The refusal of the start: `exec.sandbox-path-unrepresentable` for a path at fault, and
    /// [`PLACE_UNSERVED`] with the cause for a link or a directory that has no place.
    fn refusal(self, text: &str, address: &str) -> DriverError {
        match self {
            Self::Refused(why) | Self::Absent(why) => unrepresentable(text, why, address),
            Self::Unplaced(why) => start_unplaced(address, &why),
        }
    }
}

/// **Every symbolic link the walk of a followed entry reads is placed** (D30). `link` is what
/// the kernel reports for the link itself, named by its path in the view.
///
/// A link that has no place refuses the start: its mount is of a file-system type outside the
/// list, or another cause holds. Such a mount is compared with nothing, on either side, so
/// whether a run can write the link cannot be said; a run that could would point it anywhere.
/// A link that has a place is judged like the path's end: in a place a writable location
/// reaches, a run can replace it.
fn judge_link(link: &Opened, tree: &MountTree, locations: &[Location]) -> Result<(), Unfollowed> {
    let place = tree.table().place_of(link).map_err(Unfollowed::Unplaced)?;
    match shown_location(&place, locations) {
        Some(location) => Err(Unfollowed::Refused(format!(
            "resolves through the symbolic link {}, which lies in {}, a location a run can write",
            link.path, location.path
        ))),
        None => Ok(()),
    }
}

/// **Every directory the walk of a followed entry passes through is placed** (D30): why one
/// refuses the path, or `None`. `directory` is what the kernel reports for it, named by its
/// path in the view.
///
/// A run that can write a directory on the way can rename it, or what it holds, while the
/// helper resolves the same path in the run's view, and move the path's end to a directory
/// neither side judged. A path that never passes through a place a run can write cannot be
/// moved by a run. A directory that has no place refuses like a link that has none: whether
/// a run can write it cannot be said.
///
/// The caller holds the answer until the path's end was judged, so a path that ends where a
/// run can write, or reads a link that refuses, keeps the answer that names the end or the
/// link: the walk only opens and reads on its way there.
fn judge_passed(
    directory: &Opened,
    tree: &MountTree,
    locations: &[Location],
) -> Option<Unfollowed> {
    match tree.table().place_of(directory) {
        Err(why) => Some(Unfollowed::Unplaced(why)),
        Ok(place) => shown_location(&place, locations).map(|location| {
            Unfollowed::Refused(format!(
                "passes through {}, which lies in {}, a location a run can write",
                directory.path, location.path
            ))
        }),
    }
}

/// The device and inode of the file `descriptor` is, or `None` for a directory: what another
/// name of the same file has too.
fn file_identity(descriptor: &OwnedFd) -> Option<(libc::dev_t, libc::ino_t)> {
    // SAFETY: `fstat` fills the zeroed struct for a live descriptor.
    let mut status: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: as above; `status` is a valid out pointer.
    if unsafe { libc::fstat(descriptor.as_raw_fd(), &raw mut status) } != 0 {
        return None;
    }
    (status.st_mode & libc::S_IFMT != libc::S_IFDIR).then_some((status.st_dev, status.st_ino))
}

/// Where a path of the view is on the host: its place, and for a file its device and inode.
struct Sighted {
    place: Place,
    file: Option<(libc::dev_t, libc::ino_t)>,
}

/// Where a followed path ends in the view.
struct Reached {
    /// The device and inode of the host object, where it is a file the driver opened.
    file: Option<(libc::dev_t, libc::ino_t)>,
    /// The view path with every symbolic link resolved.
    view: String,
    /// The mount it lies in.
    mount: usize,
    /// What the kernel reports for the host object it names; `None` in a mount the driver
    /// cannot walk.
    object: Option<Opened>,
    /// The first writable mount a symbolic link of the resolution was read in.
    crossed: Option<usize>,
    /// Why the first directory the resolution passed through refuses the path: a run can
    /// write it, or it has no place.
    through: Option<Unfollowed>,
}

/// One start's view, as the driver will mount it, and what it needs to place a host path.
struct Mounts<'a> {
    list: Vec<Mount<'a>>,
    /// Every host location a run can write: the workspace root and each declared writable root,
    /// whether or not this start mounts them, what this start binds writable, and the source
    /// of every mount classed `beneath`.
    locations: Vec<Location>,
    tree: MountTree,
}

/// A mount of the view that is the run's own to write, or a file system of the view's own.
fn fixed_mount<'a>(path: &str, source: Source<'a>, eligible: bool, writable: bool) -> Mount<'a> {
    Mount {
        path: path.to_owned(),
        names: names(path),
        source,
        file: false,
        class: Class::Beneath,
        eligible,
        writable,
    }
}

/// A mount that shows something of the host, before its source is classed (D24): the field
/// it was declared in, and what the kernel reports for its source.
struct Shown<'a> {
    address: &'static str,
    path: String,
    source: Source<'a>,
    file: bool,
    eligible: bool,
    opened: Opened,
}

impl<'a> Shown<'a> {
    fn of(
        address: &'static str,
        path: &str,
        source: Source<'a>,
        file: bool,
        eligible: bool,
    ) -> Self {
        let descriptor = match &source {
            Source::Pinned(descriptor) => descriptor.as_raw_fd(),
            Source::Host(descriptor) => descriptor.as_raw_fd(),
            Source::Opaque => unreachable!("a shown mount binds a descriptor"),
        };
        Self {
            address,
            path: path.to_owned(),
            opened: Opened::of_descriptor(descriptor, path),
            source,
            file,
            eligible,
        }
    }

    fn mount(self, class: Class) -> Mount<'a> {
        Mount {
            names: names(&self.path),
            path: self.path,
            source: self.source,
            file: self.file,
            class,
            eligible: self.eligible,
            writable: class == Class::Beneath,
        }
    }
}

/// **The class of every shown mount, together** (D24, D30). A source that overlaps a location
/// a run can write is classed `beneath` and joins `locations` ([`writable_source`]); the
/// sources still classed `follow` are then asked again, against it too, until none changes.
/// So the answer does not depend on the order the mounts were declared in, and what the
/// helper will be told the run can write is what the driver judges every path against.
///
/// # Errors
///
/// As [`writable_source`].
fn class_together(
    shown: &[Shown<'_>],
    locations: &mut Vec<Location>,
    tree: &MountTree,
) -> Result<Vec<Class>, DriverError> {
    let mut classes = vec![Class::Follow; shown.len()];
    loop {
        let mut changed = false;
        for (mount, class) in shown.iter().zip(&mut classes) {
            if *class == Class::Beneath {
                continue;
            }
            if let Some(location) = writable_source(mount.address, &mount.opened, locations, tree)?
            {
                locations.push(location);
                *class = Class::Beneath;
                changed = true;
            }
        }
        if !changed {
            return Ok(classes);
        }
    }
}

/// Every host location a run can write, placed on `tree` with what it reaches (D30): the
/// workspace root and each declared writable root, then the workspace and the writable roots
/// this start binds.
fn writable_locations(
    view: &View<'_>,
    config: &HostConfig,
    tree: &MountTree,
) -> Result<Vec<Location>, DriverError> {
    let located = |address: &str, opened: Result<Opened, Unestablished>| {
        let unplaced = |why: Unestablished| start_unplaced(address, &why);
        let opened = opened.map_err(unplaced)?;
        let place = tree.table().place_of(&opened).map_err(unplaced)?;
        Location::reaching(&opened.path, &place, tree).map_err(unplaced)
    };
    let mut locations = vec![located(WORKSPACE, opened_path(&config.workspace_root))?];
    for root in &config.writable_roots {
        locations.push(located("writable_roots", opened_path(&root.path))?);
    }
    locations.push(located(
        WORKSPACE,
        Ok(Opened::of_descriptor(
            view.workspace.as_raw_fd(),
            substrate_wire::EXEC_WORKSPACE_MOUNT,
        )),
    )?);
    for root in view.writable_roots {
        locations.push(located(
            "writable_roots",
            Ok(Opened::of_descriptor(root.raw_fd(), root.mount())),
        )?);
    }
    Ok(locations)
}

impl<'a> Mounts<'a> {
    fn of(view: &View<'a>, config: &HostConfig) -> Result<Self, DriverError> {
        // The table before any path: D26 names its causes first.
        let table = MountTable::read().map_err(|cause| {
            start_unplaced(
                ENTRY,
                &Unestablished {
                    path: substrate_wire::EXEC_WORKSPACE_MOUNT.to_owned(),
                    cause,
                    asked: substrate_exec_gate::place::Asked::Place,
                },
            )
        })?;
        // The table as a tree (D30), asked whole before any location is asked what it reaches.
        let tree = table
            .into_tree()
            .map_err(|why| start_unplaced(ENTRY, &why))?;
        let mut locations = writable_locations(view, config, &tree)?;
        // In the order D24 lists the writable mounts: the workspace, the writable roots, the
        // mounts classed `beneath`, then the view's own file systems.
        let mut list = vec![fixed_mount(
            substrate_wire::EXEC_WORKSPACE_MOUNT,
            Source::Pinned(view.workspace),
            true,
            true,
        )];
        for root in view.writable_roots {
            list.push(fixed_mount(
                root.mount(),
                Source::Pinned(root.descriptor()),
                true,
                true,
            ));
        }
        let mut shown = Vec::new();
        for (path, eligible) in BASE_MOUNTS
            .iter()
            .map(|path| (*path, true))
            .chain(std::iter::once((ALTERNATIVES_MOUNT, false)))
        {
            // Opened through the name, as bubblewrap binds it; a directory the host does not
            // have is not in the view.
            let Ok(directory) = std::fs::File::open(path) else {
                continue;
            };
            let source = Source::Host(OwnedFd::from(directory));
            shown.push(Shown::of(ENTRY, path, source, false, eligible));
        }
        for root in view.read_only_roots {
            let source = Source::Pinned(root.descriptor());
            shown.push(Shown::of(
                "read_only_roots",
                root.mount(),
                source,
                false,
                true,
            ));
        }
        for file in view.read_only_files {
            let source = Source::Pinned(file.descriptor());
            shown.push(Shown::of(
                "read_only_files",
                file.mount(),
                source,
                true,
                true,
            ));
        }
        let classes = class_together(&shown, &mut locations, &tree)?;
        list.extend(
            shown
                .into_iter()
                .zip(classes)
                .map(|(mount, class)| mount.mount(class)),
        );
        list.push(fixed_mount(TMP_MOUNT, Source::Opaque, false, true));
        list.push(fixed_mount(DEV_MOUNT, Source::Opaque, false, true));
        if view.scratch {
            list.push(fixed_mount(
                substrate_wire::EXEC_SCRATCH_MOUNT,
                Source::Opaque,
                false,
                true,
            ));
        }
        list.push(fixed_mount(PROC_MOUNT, Source::Opaque, false, false));
        Ok(Self {
            list,
            locations,
            tree,
        })
    }

    /// The mounts the helper is told the run can write (`--writable`).
    fn writable(&self) -> Vec<String> {
        self.list
            .iter()
            .filter(|mount| mount.writable)
            .map(|mount| mount.path.clone())
            .collect()
    }

    /// The mount `path` is, or lies beneath, and its names beneath it: the longest such mount
    /// of the view as the driver builds it. This is the driver's own map of what it mounts
    /// where, not a comparison of two host paths.
    fn locate(&self, path: &[String]) -> Option<(usize, Vec<String>)> {
        self.list
            .iter()
            .enumerate()
            .filter(|(_, mount)| path.starts_with(&mount.names))
            .max_by_key(|(_, mount)| mount.names.len())
            .map(|(index, mount)| (index, path[mount.names.len()..].to_vec()))
    }

    /// Resolves `path` as the view will: its symbolic links read from the objects the mounts
    /// bind, an absolute target starting again at the view's root, not the host's. `asked` is
    /// the path as the profile wrote it, which a refusal names.
    ///
    /// Every symbolic link read is placed first ([`judge_link`]), and refuses where it is
    /// read. So is every directory passed through ([`judge_passed`]); what the first of them
    /// refuses for is returned with the end, for the caller to hold against the path once the
    /// end was judged. `..` drops the last name of the path so far, which holds no link: the
    /// helper resolves the same way.
    fn follow(&self, path: &str, asked: &str) -> Result<Reached, Unfollowed> {
        let mut pending: VecDeque<String> = names(path).into();
        let mut current: Vec<String> = Vec::new();
        let mut crossed = None;
        let mut through = None;
        let mut links = 0;
        while let Some(name) = pending.pop_front() {
            match name.as_str() {
                "." => continue,
                ".." => {
                    current.pop();
                    continue;
                }
                _ => current.push(name),
            }
            let Some((index, rel)) = self.locate(&current) else {
                // The view's own root holds nothing but the parents of its mount points.
                if self
                    .list
                    .iter()
                    .any(|mount| mount.names.starts_with(&current))
                {
                    continue;
                }
                return Err(Unfollowed::Refused(format!(
                    "names {}, which the view does not have",
                    join(&current)
                )));
            };
            let mount = &self.list[index];
            let Some(directory) = mount.descriptor() else {
                current.extend(pending);
                return Ok(Reached {
                    file: None,
                    view: join(&current),
                    mount: index,
                    object: None,
                    crossed,
                    through,
                });
            };
            let here = join(&current);
            // A mount the run can write, entered and not ended in: by the mount's own name.
            if mount.writable && !pending.is_empty() && through.is_none() {
                through = Some(Unfollowed::Refused(format!(
                    "passes through {here}, in the writable mount {}",
                    mount.path
                )));
            }
            if rel.is_empty() {
                continue;
            }
            if mount.file {
                return Err(Unfollowed::Refused(format!(
                    "names {here}, beneath a mount that is a single file"
                )));
            }
            let opened =
                open_beneath(directory, &rel.join("/"), libc::O_NOFOLLOW).map_err(|error| {
                    Unfollowed::unopened(
                        format!("names {here}, which {}", why_unopened(&error)),
                        &error,
                    )
                })?;
            let target = link_target(&opened).map_err(|error| {
                Unfollowed::other(format!("names {here}, which cannot be read ({error})"))
            })?;
            if let Some(target) = target {
                links += 1;
                if links > MAX_LINKS {
                    return Err(Unfollowed::Refused(format!(
                        "resolves through more than {MAX_LINKS} symbolic links"
                    )));
                }
                // A link in a mount the run can write is refused for that below, by the
                // mount's own name; any other is placed, and judged by where it lies.
                if mount.writable {
                    crossed.get_or_insert(index);
                } else {
                    let link = Opened::of_descriptor(opened.as_raw_fd(), &here);
                    judge_link(&link, &self.tree, &self.locations)?;
                }
                current.pop();
                if target.starts_with('/') {
                    current.clear();
                }
                for name in names(&target).into_iter().rev() {
                    pending.push_front(name);
                }
            } else if !mount.writable && !pending.is_empty() && through.is_none() {
                // A directory on the way, in a mount no class calls writable: by its place.
                let passed = Opened::of_descriptor(opened.as_raw_fd(), &here);
                through = judge_passed(&passed, &self.tree, &self.locations);
            }
        }
        let mut reached = self.end(&current, asked, crossed)?;
        reached.through = through;
        Ok(reached)
    }

    /// Where a walk ended: the mount `current` lies in, and what the kernel reports for the
    /// host object it names there.
    fn end(
        &self,
        current: &[String],
        asked: &str,
        crossed: Option<usize>,
    ) -> Result<Reached, Unfollowed> {
        let Some((index, rel)) = self.locate(current) else {
            return Err(Unfollowed::Refused(format!(
                "resolves to {}, which lies in no mount of the view",
                join(current)
            )));
        };
        let mount = &self.list[index];
        let (object, file) = match mount.descriptor() {
            None => (None, None),
            Some(directory) => {
                let object = if rel.is_empty() {
                    None
                } else {
                    Some(open_beneath(directory, &rel.join("/"), 0).map_err(|error| {
                        Unfollowed::unopened(
                            format!(
                                "resolves to {}, which {}",
                                join(current),
                                why_unopened(&error)
                            ),
                            &error,
                        )
                    })?)
                };
                let descriptor = object.as_ref().unwrap_or(directory);
                (
                    Some(Opened::of_descriptor(descriptor.as_raw_fd(), asked)),
                    file_identity(descriptor),
                )
            }
        };
        Ok(Reached {
            file,
            view: join(current),
            mount: index,
            object,
            crossed,
            through: None,
        })
    }

    /// D24: a followed path that ends where a run can write names a file the run can replace.
    /// Asked of the view's mounts as the driver built them, and of the place of what the path
    /// names on the host (D25), so a host mount beneath a read-only root is seen too.
    fn check_followed(&self, text: &str, path: &str, address: &str) -> Result<(), DriverError> {
        let reached = self
            .follow(path, text)
            .map_err(|why| why.refusal(text, address))?;
        self.check_reached(text, reached, address).map(drop)
    }

    /// The checks of [`Self::check_followed`] on a path that was followed to its end, and where
    /// what it names is on the host.
    fn check_reached(
        &self,
        text: &str,
        reached: Reached,
        address: &str,
    ) -> Result<Sighted, DriverError> {
        let refuse = |why: String| unrepresentable(text, why, address);
        if let Some(crossed) = reached.crossed {
            return Err(refuse(format!(
                "resolves through a symbolic link in the writable mount {}",
                self.list[crossed].path
            )));
        }
        let mount = &self.list[reached.mount];
        if mount.writable {
            return Err(refuse(format!(
                "resolves to {}, in the writable mount {}",
                reached.view, mount.path
            )));
        }
        let Some(object) = &reached.object else {
            return Err(refuse(format!(
                "resolves to {}, in {}, where no executable path can be named",
                reached.view, mount.path
            )));
        };
        let place = self
            .tree
            .table()
            .place_of(object)
            .map_err(|why| start_unplaced(address, &why))?;
        if let Some(location) = shown_location(&place, &self.locations) {
            return Err(refuse(format!(
                "resolves to {}, which shows {}, a location a run can write",
                reached.view, location.path
            )));
        }
        // The end is where no run can write; the way to it is judged last, so a path that
        // ends where a run can write keeps the answer it had.
        if let Some(why) = reached.through {
            return Err(why.refusal(text, address));
        }
        Ok(Sighted {
            place,
            file: reached.file,
        })
    }

    /// The eligible mount `text` lies in, the view path of `text`, and its names beneath the
    /// mount; the refusal of a path that lies in none.
    fn eligible(
        &self,
        text: &str,
        address: &str,
    ) -> Result<(&Mount<'a>, String, Vec<String>), DriverError> {
        literal(text, address)?;
        // A relative entry lies beneath the workspace mount.
        let path = if text.starts_with('/') {
            text.to_owned()
        } else {
            format!("{}/{text}", substrate_wire::EXEC_WORKSPACE_MOUNT)
        };
        let Some((mount, rel)) = self
            .locate(&names(&path))
            .map(|(index, rel)| (&self.list[index], rel))
            .filter(|(mount, _)| mount.eligible)
        else {
            return Err(if address == PROGRAM {
                unrepresentable(
                    text,
                    "lies in no mount an executable list can name",
                    address,
                )
            } else {
                DriverError::refused(
                    EXEC_SUBPATH_UNDECLARED,
                    format!(
                        "{text:?} lies outside /usr, /bin, /lib, /lib64, /workspace and the \
                         declared mounts of this run"
                    ),
                    address,
                )
            });
        };
        if mount.file && !rel.is_empty() {
            return Err(unrepresentable(
                text,
                "lies beneath a mount that is a single file",
                address,
            ));
        }
        Ok((mount, path, rel))
    }

    /// One entry of the deny-list, located in the view (design 35 § 8), and where it is on the
    /// host when it exists. An entry need not exist at the start: it is then kept as a name,
    /// its nearest existing ancestor is held to the rules of an entry, and the place of that
    /// ancestor must be one this host can establish (D26).
    fn denied(&self, text: &str) -> Result<(Located, Option<Sighted>), DriverError> {
        let (mount, path, rel) = self.eligible(text, DENIED)?;
        let refuse = |why: String| unrepresentable(text, why, DENIED);
        let unplaced = |why: Unestablished| denied_unplaced(&why);
        let sighted = match mount.class {
            Class::Beneath => {
                let directory = mount.descriptor().expect("an eligible mount is walked");
                let mut existing = rel.clone();
                loop {
                    if existing.is_empty() {
                        let opened = Opened::of_descriptor(directory.as_raw_fd(), text);
                        let place = self.tree.table().place_of(&opened).map_err(unplaced)?;
                        break rel.is_empty().then(|| Sighted {
                            place,
                            file: file_identity(directory),
                        });
                    }
                    match open_beneath(directory, &existing.join("/"), 0) {
                        Ok(object) => {
                            let opened = Opened::of_descriptor(object.as_raw_fd(), text);
                            let place = self.tree.table().place_of(&opened).map_err(unplaced)?;
                            break (existing.len() == rel.len()).then(|| Sighted {
                                place,
                                file: file_identity(&object),
                            });
                        }
                        Err(error)
                            if matches!(
                                error.raw_os_error(),
                                Some(libc::ENOENT | libc::ENOTDIR)
                            ) =>
                        {
                            existing.pop();
                        }
                        Err(error) => return Err(refuse(why_unopened(&error))),
                    }
                }
            }
            Class::Follow => {
                let all = names(&path);
                let mut existing = all.clone();
                loop {
                    match self.follow(&join(&existing), text) {
                        Ok(reached) => {
                            let sighted = self.check_reached(text, reached, DENIED)?;
                            break (existing.len() == all.len()).then_some(sighted);
                        }
                        Err(Unfollowed::Absent(_)) if existing.len() > mount.names.len() => {
                            existing.pop();
                        }
                        Err(unfollowed) => return Err(unfollowed.refusal(text, DENIED)),
                    }
                }
            }
        };
        let rel = if rel.is_empty() {
            ".".to_owned()
        } else {
            rel.join("/")
        };
        let located = Located::new(mount.class, &mount.path, &rel)
            .map_err(|why| refuse(format!("is not a path of the gate: {why}")))?;
        check_openable(&located).map_err(refuse)?;
        Ok((located, sighted))
    }

    /// Where the program is on the host, after [`Self::located`] accepted it: what the deny
    /// check compares (D23).
    fn program_sighted(&self, text: &str) -> Result<Sighted, DriverError> {
        let (mount, path, rel) = self.eligible(text, PROGRAM)?;
        let refuse = |why: String| unrepresentable(text, why, PROGRAM);
        match mount.class {
            Class::Follow => {
                let reached = self
                    .follow(&path, text)
                    .map_err(|unfollowed| unfollowed.refusal(text, PROGRAM))?;
                self.check_reached(text, reached, PROGRAM)
            }
            Class::Beneath => {
                let directory = mount.descriptor().expect("an eligible mount is walked");
                let object = if rel.is_empty() {
                    None
                } else {
                    Some(
                        open_beneath(directory, &rel.join("/"), 0)
                            .map_err(|error| refuse(why_unopened(&error)))?,
                    )
                };
                let descriptor = object.as_ref().unwrap_or(directory);
                Ok(Sighted {
                    place: self
                        .tree
                        .table()
                        .place_of(&Opened::of_descriptor(descriptor.as_raw_fd(), text))
                        .map_err(|why| start_unplaced(PROGRAM, &why))?,
                    file: file_identity(descriptor),
                })
            }
        }
    }

    /// One path of a list, located in the view and held to the rules of design 35 § 5.
    fn located(&self, text: &str, address: &str) -> Result<Located, DriverError> {
        let (mount, path, rel) = self.eligible(text, address)?;
        let refuse = |why: String| unrepresentable(text, why, address);
        match mount.class {
            Class::Beneath => {
                let directory = mount.descriptor().expect("an eligible mount is walked");
                // No part may be a symbolic link: a run could have left one.
                let opened = if rel.is_empty() {
                    None
                } else {
                    Some(
                        open_beneath(directory, &rel.join("/"), 0)
                            .map_err(|error| refuse(why_unopened(&error)))?,
                    )
                };
                // Every entry is placed where it ends, whatever its class (D26): an end on a
                // mount of a type outside the list, beneath a mount a run can write, is
                // compared with nothing, and is refused here as under a `follow` mount.
                let end = opened.as_ref().unwrap_or(directory);
                self.tree
                    .table()
                    .place_of(&Opened::of_descriptor(end.as_raw_fd(), &path))
                    .map_err(|why| start_unplaced(address, &why))?;
            }
            Class::Follow => self.check_followed(text, &path, address)?,
        }
        let rel = if rel.is_empty() {
            ".".to_owned()
        } else {
            rel.join("/")
        };
        let located = Located::new(mount.class, &mount.path, &rel)
            .map_err(|why| refuse(format!("is not a path of the gate: {why}")))?;
        check_openable(&located).map_err(refuse)?;
        Ok(located)
    }
}

/// The location a run can write that `place` lies in, if any: where a followed path ends on the
/// host, asked of every writable location by place and never by text (D24, D25). A host mount
/// beneath a read-only root, and a second mount of the workspace root, are found this way.
fn shown_location<'l>(place: &Place, locations: &'l [Location]) -> Option<&'l Location> {
    locations
        .iter()
        .find(|location| location.reaches.holds(place))
}

/// The longest path the kernel takes: `PATH_MAX` less its terminator.
const MAX_PATH_BYTES: usize = 4095;

/// Whether the kernel takes `path`, a path of the run's view the helper is given (design 35
/// § 5, *A view path the kernel takes*). No bound below the kernel's is set.
fn check_length(path: &str) -> Result<(), String> {
    let bytes = path.len();
    if bytes > MAX_PATH_BYTES {
        return Err(format!(
            "is {bytes} bytes long in the run's view, over the {MAX_PATH_BYTES} bytes of a path \
             the kernel takes"
        ));
    }
    Ok(())
}

/// Whether the helper can open what `located` names in the run's view: the text of
/// `<root>/<rel>`, which it opens for a `follow` path and executes the program by. The driver
/// checked the entry through the descriptor of its mount, which a longer view path does not
/// hinder, so an entry over the limit would be dispatched and end the run with the helper's
/// status; it is refused here by name. One measure for both classes, as § 5 writes the rule.
fn check_openable(located: &Located) -> Result<(), String> {
    check_length(&located.path())
}

/// One start's lists as the helper takes them: the program, the host's loader, every entry of
/// the allow-list and of the deny-list, and the mounts the run can write, each located in `view`
/// (D24).
///
/// **A denied program is refused here** (D10, D23): `exec.sandbox-program-unlisted` when the
/// program lies in a denied path, by place (D25) on the driver's own mount table, or is the
/// same file, by device and inode, as a denied file. A program that is a link to a denied
/// program is that case. The helper is the second line: it writes no rule for such a program.
///
/// # Errors
///
/// Before anything is dispatched, each with nothing run:
/// `exec.sandbox-path-unrepresentable` for a program that is not an absolute path of the view
/// (address `argv`), and for an entry that is not normal, does not exist at the start, has a
/// symbolic link in a mount classed `beneath`, or resolves into a mount the run can write;
/// `exec.sandbox-exec-subpath-undeclared` for an absolute entry outside the eligible locations;
/// [`PROGRAM_UNLISTED`] for a denied program; and [`PLACE_UNSERVED`] where a place cannot be
/// established, of the program, of an entry of either list or of a deny entry's nearest
/// existing ancestor.
pub(crate) fn plan(
    gate: &ExecGate,
    entries: &[String],
    denied: &[String],
    program: &str,
    view: &View<'_>,
    config: &HostConfig,
) -> Result<Lists, DriverError> {
    if !program.starts_with('/') {
        return Err(unrepresentable(
            program,
            "is not an absolute path, which a run with an executable list needs",
            PROGRAM,
        ));
    }
    let mounts = Mounts::of(view, config)?;
    let asked = program;
    let program = mounts.located(asked, PROGRAM)?;
    let allow = entries
        .iter()
        .map(|entry| mounts.located(entry, ENTRY))
        .collect::<Result<Vec<_>, _>>()?;
    let mut deny = Vec::with_capacity(denied.len());
    let mut sighted = Vec::new();
    for entry in denied {
        let (located, existing) = mounts.denied(entry)?;
        deny.push(located);
        sighted.extend(existing.map(|existing| (entry, existing)));
    }
    if !sighted.is_empty() {
        let program = mounts.program_sighted(asked)?;
        for (entry, denied) in &sighted {
            // What a denied directory holds is what it reaches (D30): its own place and the
            // root of every mount beneath it. Asked as of a directory that must not be
            // missed: a mount beneath it that cannot be compared refuses the start.
            let reaches = mounts
                .tree
                .reaches(&denied.place, Unlisted::Refused)
                .map_err(|why| denied_unplaced(&why))?;
            if reaches.holds(&program.place)
                || (program.file.is_some() && program.file == denied.file)
            {
                return Err(DriverError::refused(
                    PROGRAM_UNLISTED,
                    format!(
                        "{asked:?} is denied by the deny-list entry {entry:?}: it lies in it, \
                         or is another name of the same file"
                    ),
                    PROGRAM,
                ));
            }
        }
    }
    // The helper opens the loader as a `follow` path and refuses one in a writable mount; the
    // same answer here, before dispatch, instead of exit status 125 after it.
    if let Some(loader) = gate.loader() {
        check_length(loader).map_err(|why| unrepresentable(loader, why, ENTRY))?;
        mounts.check_followed(loader, loader, ENTRY)?;
    }
    let writable = mounts.writable();
    for root in &writable {
        check_length(root).map_err(|why| unrepresentable(root, why, WRITABLE_MOUNT))?;
    }
    Ok(Lists {
        program,
        loader: gate.loader().map(str::to_owned),
        allow,
        deny,
        writable,
    })
}

#[cfg(test)]
#[path = "exec_gate_tests.rs"]
mod tests;
