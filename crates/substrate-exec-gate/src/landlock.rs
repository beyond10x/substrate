//! The ruleset: opening each path as its class says, and the three Landlock system calls.
//!
//! Every descriptor opened here is close-on-exec, and the kernel creates the ruleset's descriptor
//! close-on-exec, so the run's program inherits none of them.

use std::ffi::{CStr, CString};
use std::io;
use std::os::fd::{AsRawFd as _, FromRawFd as _, OwnedFd, RawFd};

use crate::place::{MountTable, Opened, Place, Unestablished};
use crate::{Class, Lists, Located};

/// `LANDLOCK_ACCESS_FS_EXECUTE`.
const ACCESS_FS_EXECUTE: u64 = 1;
/// `LANDLOCK_ACCESS_FS_REFER`, which Landlock ABI 2 (Linux 5.19) added.
const ACCESS_FS_REFER: u64 = 1 << 13;
/// `LANDLOCK_RULE_PATH_BENEATH`.
const RULE_PATH_BENEATH: libc::c_int = 1;

/// `struct landlock_ruleset_attr` as the first ABI defines it. Passing this size leaves the
/// members later ABIs added, `handled_access_net` and `scoped`, at zero (design 35 D6).
#[repr(C)]
struct RulesetAttr {
    handled_access_fs: u64,
}

/// `struct landlock_path_beneath_attr`, which the kernel declares packed.
#[repr(C, packed)]
struct PathBeneathAttr {
    allowed_access: u64,
    parent_fd: i32,
}

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

fn c_path(text: &str) -> Result<CString, String> {
    CString::new(text).map_err(|_| format!("{text:?} holds a NUL byte"))
}

fn open_at(directory: RawFd, path: &CStr, flags: libc::c_int) -> io::Result<OwnedFd> {
    // SAFETY: `path` is a NUL-terminated string that outlives the call.
    let descriptor = unsafe {
        libc::openat(
            directory,
            path.as_ptr(),
            libc::O_PATH | libc::O_CLOEXEC | flags,
        )
    };
    if descriptor < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `openat` returned a descriptor nothing else owns.
    Ok(unsafe { OwnedFd::from_raw_fd(descriptor) })
}

fn status(descriptor: &OwnedFd) -> io::Result<libc::stat> {
    // SAFETY: `fstat` fills the zeroed struct for a live descriptor.
    let mut status: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: as above; `status` is a valid out pointer.
    if unsafe { libc::fstat(descriptor.as_raw_fd(), &raw mut status) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(status)
}

const fn kind(status: &libc::stat) -> libc::mode_t {
    status.st_mode & libc::S_IFMT
}

/// Opens the `<root>` of a `beneath` path: the mount itself, never a link in its place.
fn open_root(root: &str, directory: bool) -> Result<OwnedFd, String> {
    let flags = libc::O_NOFOLLOW | if directory { libc::O_DIRECTORY } else { 0 };
    let descriptor = open_at(libc::AT_FDCWD, &c_path(root)?, flags)
        .map_err(|error| format!("cannot open {root:?}: {error}"))?;
    // `O_PATH | O_NOFOLLOW` opens a symbolic link itself rather than refusing it.
    let status = status(&descriptor).map_err(|error| format!("cannot stat {root:?}: {error}"))?;
    if kind(&status) == libc::S_IFLNK {
        return Err(format!("{root:?} is a symbolic link"));
    }
    Ok(descriptor)
}

/// Opens a `beneath` path: `<rel>` beneath `<root>`, no part of it a symbolic link (D24).
fn open_beneath(located: &Located) -> Result<OwnedFd, String> {
    if located.rel() == "." {
        return open_root(located.root(), false);
    }
    let root = open_root(located.root(), true)?;
    let rel = c_path(located.rel())?;
    let how = OpenHow {
        flags: u64::try_from(libc::O_PATH | libc::O_CLOEXEC).expect("open flags are positive"),
        mode: 0,
        resolve: RESOLVE_BENEATH | RESOLVE_NO_SYMLINKS | RESOLVE_NO_MAGICLINKS,
    };
    // SAFETY: `rel` is NUL-terminated, `how` is a live `open_how` of the size passed, and both
    // outlive the call.
    let descriptor = unsafe {
        libc::syscall(
            libc::SYS_openat2,
            root.as_raw_fd(),
            rel.as_ptr(),
            &raw const how,
            std::mem::size_of::<OpenHow>(),
        )
    };
    if descriptor < 0 {
        let error = io::Error::last_os_error();
        return Err(format!(
            "cannot open {:?} beneath {:?} without following a symbolic link: {error}",
            located.rel(),
            located.root()
        ));
    }
    let descriptor = RawFd::try_from(descriptor).expect("a descriptor fits its type");
    // SAFETY: `openat2` returned a descriptor nothing else owns.
    Ok(unsafe { OwnedFd::from_raw_fd(descriptor) })
}

fn create_ruleset() -> Result<OwnedFd, String> {
    let attributes = RulesetAttr {
        handled_access_fs: ACCESS_FS_EXECUTE | ACCESS_FS_REFER,
    };
    // SAFETY: `attributes` is a live `landlock_ruleset_attr` prefix of the size passed.
    let descriptor = unsafe {
        libc::syscall(
            libc::SYS_landlock_create_ruleset,
            &raw const attributes,
            std::mem::size_of::<RulesetAttr>(),
            0,
        )
    };
    if descriptor < 0 {
        // `ENOSYS` without Landlock, `EOPNOTSUPP` where it is disabled at boot, `EINVAL` on ABI 1,
        // which has no refer right (design 35 D8).
        return Err(format!(
            "Landlock is unavailable with the execute and refer rights: {}",
            io::Error::last_os_error()
        ));
    }
    let descriptor = RawFd::try_from(descriptor).expect("a descriptor fits its type");
    // SAFETY: the kernel returned a descriptor nothing else owns.
    Ok(unsafe { OwnedFd::from_raw_fd(descriptor) })
}

fn add_rule(ruleset: &OwnedFd, target: &OwnedFd, access: u64, what: &str) -> Result<(), String> {
    let rule = PathBeneathAttr {
        allowed_access: access,
        parent_fd: target.as_raw_fd(),
    };
    // SAFETY: `rule` is a live `landlock_path_beneath_attr` that outlives the call.
    let added = unsafe {
        libc::syscall(
            libc::SYS_landlock_add_rule,
            ruleset.as_raw_fd(),
            RULE_PATH_BENEATH,
            &raw const rule,
            0,
        )
    };
    if added != 0 {
        return Err(format!(
            "the kernel refused the rule for {what}: {}",
            io::Error::last_os_error()
        ));
    }
    Ok(())
}

/// The most rules one ruleset is given (design 35 § 8.2). The walk of D11 stops there, and the
/// helper executes nothing.
const MAX_RULES: usize = 65_536;

/// Every path that receives the execute right, opened as its class says: the program, the loader
/// and each entry. Each goes through the deny check afterwards (D23).
fn execute_targets(
    lists: &Lists,
    view: &mut Writable<'_>,
) -> Result<Vec<(String, OwnedFd)>, String> {
    let mut targets = Vec::new();
    for (option, path) in std::iter::once(("--program", &lists.program))
        .chain(lists.allow.iter().map(|entry| ("--allow", entry)))
    {
        let descriptor = match path.class() {
            Class::Beneath => open_beneath(path)?,
            Class::Follow => open_follow(option, &path.path(), view)?,
        };
        targets.push((path.path(), descriptor));
    }
    if let Some(loader) = &lists.loader {
        targets.push((loader.clone(), open_follow("--loader", loader, view)?));
    }
    Ok(targets)
}

/// The mounts of the view the run can write, and once they were needed the view's mount table
/// with the place of each.
struct Writable<'a> {
    roots: &'a [String],
    placed: Option<(MountTable, Vec<Place>)>,
}

impl Writable<'_> {
    /// The table and the places, established at the first call.
    fn placed(&mut self, path: &str) -> Result<&(MountTable, Vec<Place>), Unestablished> {
        if self.placed.is_none() {
            // The table before any path: D26 names its causes first.
            let table = MountTable::read().map_err(|cause| Unestablished {
                path: path.to_owned(),
                cause,
            })?;
            let places = self
                .roots
                .iter()
                .map(|root| table.place_of(&Opened::of_path(root)?))
                .collect::<Result<Vec<_>, _>>()?;
            self.placed = Some((table, places));
        }
        Ok(self.placed.as_ref().expect("established above"))
    }

    /// The `--writable` root `place` lies in, if any (D24, D25).
    fn holding(&mut self, path: &str, place: &Place) -> Result<Option<&str>, Unestablished> {
        let roots = self.roots;
        let (_, places) = self.placed(path)?;
        Ok(roots
            .iter()
            .zip(places)
            .find(|(_, writable)| place.lies_in(writable))
            .map(|(root, _)| root.as_str()))
    }
}

/// Opens a `follow` path, following symbolic links, and refuses one that resolves into a mount
/// the run can write (D24): what such a path names is a file the run can replace. Where the
/// path lies is decided by its place (D25): the mount the kernel reports for the descriptor,
/// in the view's mount table. A place that cannot be established refuses too, with its cause
/// (D26); the text of the two paths decides nothing.
fn open_follow(option: &str, path: &str, view: &mut Writable<'_>) -> Result<OwnedFd, String> {
    let descriptor = open_at(libc::AT_FDCWD, &c_path(path)?, 0)
        .map_err(|error| format!("cannot open {path:?}: {error}"))?;
    if view.roots.is_empty() {
        return Ok(descriptor);
    }
    let opened = Opened::of_descriptor(descriptor.as_raw_fd(), path);
    let (table, _) = view.placed(path).map_err(|why| why.to_string())?;
    let place = table.place_of(&opened).map_err(|why| why.to_string())?;
    if let Some(root) = view.holding(path, &place).map_err(|why| why.to_string())? {
        // The path the kernel holds for the descriptor: what the links resolved to.
        let resolved = opened.descriptor_path.as_deref().unwrap_or(path);
        return Err(format!(
            "{option} {path} resolves to {resolved}, in the writable mount {root}"
        ));
    }
    Ok(descriptor)
}

/// One entry of the deny-list, where it is: its place, and for a file that exists the device
/// and inode another name of it has too (D11 step 1).
struct Denied {
    place: Place,
    file: Option<(libc::dev_t, libc::ino_t)>,
}

/// Opens a `beneath` deny path, or its nearest ancestor beneath `<root>` that exists, and
/// returns the names after it: a deny entry need not exist (design 35 § 8.2). No part that
/// exists may be a symbolic link.
fn open_beneath_or_ancestor(located: &Located) -> Result<(OwnedFd, Vec<String>), String> {
    let mut names: Vec<&str> = match located.rel() {
        "." => Vec::new(),
        rel => rel.split('/').collect(),
    };
    let mut remaining = Vec::new();
    loop {
        let rel = if names.is_empty() {
            ".".to_owned()
        } else {
            names.join("/")
        };
        let ancestor = Located::new(Class::Beneath, located.root(), &rel)?;
        if names.is_empty() {
            return Ok((open_beneath(&ancestor)?, remaining));
        }
        // Asked of the kernel directly, so "does not exist" is told from every other failure.
        let root = open_root(located.root(), true)?;
        let how = OpenHow {
            flags: u64::try_from(libc::O_PATH | libc::O_CLOEXEC).expect("open flags are positive"),
            mode: 0,
            resolve: RESOLVE_BENEATH | RESOLVE_NO_SYMLINKS | RESOLVE_NO_MAGICLINKS,
        };
        let text = c_path(&rel)?;
        // SAFETY: `text` is NUL-terminated, `how` is a live `open_how` of the size passed, and
        // both outlive the call.
        let descriptor = unsafe {
            libc::syscall(
                libc::SYS_openat2,
                root.as_raw_fd(),
                text.as_ptr(),
                &raw const how,
                std::mem::size_of::<OpenHow>(),
            )
        };
        if descriptor >= 0 {
            let descriptor = RawFd::try_from(descriptor).expect("a descriptor fits its type");
            // SAFETY: `openat2` returned a descriptor nothing else owns.
            return Ok((unsafe { OwnedFd::from_raw_fd(descriptor) }, remaining));
        }
        let error = io::Error::last_os_error();
        if !matches!(error.raw_os_error(), Some(libc::ENOENT | libc::ENOTDIR)) {
            return Err(format!(
                "cannot open {rel:?} beneath {:?} without following a symbolic link: {error}",
                located.root()
            ));
        }
        remaining.insert(0, names.pop().expect("not empty").to_owned());
    }
}

/// Where every entry of the deny-list is (D11, D25). A `follow` entry that resolves into a
/// mount the run can write is refused, as every followed path is (D24).
fn denied_paths(lists: &Lists, view: &mut Writable<'_>) -> Result<Vec<Denied>, String> {
    let mut denied = Vec::new();
    for entry in &lists.deny {
        let path = entry.path();
        let (opened, file) = match entry.class() {
            Class::Beneath => {
                let (descriptor, remaining) = open_beneath_or_ancestor(entry)?;
                let mut opened = Opened::of_descriptor(descriptor.as_raw_fd(), &path);
                opened.remaining = remaining;
                let status = status(&descriptor)
                    .map_err(|error| format!("cannot stat {path:?}: {error}"))?;
                (opened, (status.st_dev, status.st_ino, kind(&status)))
            }
            Class::Follow => {
                let opened = Opened::of_path(&path).map_err(|why| why.to_string())?;
                let c_text = c_path(&path)?;
                // SAFETY: `stat` fills the zeroed struct for a NUL-terminated path.
                let mut status: libc::stat = unsafe { std::mem::zeroed() };
                // SAFETY: as above; `status` is a valid out pointer. A path that does not
                // exist leaves it zeroed, and `opened.remaining` says so below.
                unsafe { libc::stat(c_text.as_ptr(), &raw mut status) };
                (opened, (status.st_dev, status.st_ino, kind(&status)))
            }
        };
        let (table, _) = view.placed(&path).map_err(|why| why.to_string())?;
        let place = table.place_of(&opened).map_err(|why| why.to_string())?;
        if entry.class() == Class::Follow
            && let Some(root) = view.holding(&path, &place).map_err(|why| why.to_string())?
        {
            let resolved = opened.descriptor_path.as_deref().unwrap_or(&path);
            return Err(format!(
                "--deny {path} resolves to {resolved}, in the writable mount {root}"
            ));
        }
        let (device, inode, kind) = file;
        denied.push(Denied {
            place,
            file: (opened.remaining.is_empty() && kind != libc::S_IFDIR && kind != 0)
                .then_some((device, inode)),
        });
    }
    Ok(denied)
}

/// The ruleset being written, and how many rules it holds.
struct Rules<'a> {
    ruleset: &'a OwnedFd,
    written: usize,
}

impl Rules<'_> {
    fn execute(&mut self, target: &OwnedFd, path: &str) -> Result<(), String> {
        if self.written == MAX_RULES {
            return Err(format!(
                "the lists need more than {MAX_RULES} rules: a deny entry lies in a directory \
                 with too many entries"
            ));
        }
        self.written += 1;
        add_rule(
            self.ruleset,
            target,
            ACCESS_FS_EXECUTE,
            &format!("{path:?}"),
        )
    }
}

/// The names of the directory `descriptor` is, but for `.` and `..`.
fn entries(descriptor: &OwnedFd, path: &str) -> Result<Vec<CString>, String> {
    let unreadable = |error: io::Error| format!("cannot read the directory {path:?}: {error}");
    // An `O_PATH` descriptor cannot be read: the directory is opened again through it.
    // SAFETY: `c"."` is a NUL-terminated string that outlives the call.
    let readable = unsafe {
        libc::openat(
            descriptor.as_raw_fd(),
            c".".as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
        )
    };
    if readable < 0 {
        return Err(unreadable(io::Error::last_os_error()));
    }
    // SAFETY: `readable` is a directory descriptor nothing else owns; the stream owns it now.
    let stream = unsafe { libc::fdopendir(readable) };
    if stream.is_null() {
        let error = io::Error::last_os_error();
        // SAFETY: the stream was not created, so the descriptor is still this function's.
        unsafe { libc::close(readable) };
        return Err(unreadable(error));
    }
    let mut names = Vec::new();
    let read = loop {
        // SAFETY: `__errno_location` is this thread's `errno`.
        unsafe { *libc::__errno_location() = 0 };
        // SAFETY: `stream` is a live directory stream.
        let entry = unsafe { libc::readdir(stream) };
        if entry.is_null() {
            let error = io::Error::last_os_error();
            break match error.raw_os_error() {
                Some(0) | None => Ok(()),
                Some(_) => Err(unreadable(error)),
            };
        }
        // SAFETY: `readdir` returned an entry whose name is NUL-terminated.
        let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) };
        if !matches!(name.to_bytes(), b"." | b"..") {
            names.push(name.to_owned());
        }
    };
    // SAFETY: `stream` is a live directory stream, closed once.
    unsafe { libc::closedir(stream) };
    read.map(|()| names)
}

/// One path that is to receive the execute right, and where it is.
struct Granted {
    path: String,
    descriptor: OwnedFd,
    place: Place,
}

/// D11: the three steps for `granted`, each entry of an expanded directory going through them
/// in turn. Every comparison is of places (D25), and of device and inode for another name of a
/// denied file.
fn grant(
    rules: &mut Rules<'_>,
    table: &MountTable,
    denied: &[Denied],
    granted: &Granted,
) -> Result<(), String> {
    let Granted {
        path,
        descriptor,
        place,
    } = granted;
    let status = status(descriptor).map_err(|error| format!("cannot stat {path:?}: {error}"))?;
    let directory = kind(&status) == libc::S_IFDIR;
    // Step 1: it lies in a denied path, or is another name of a denied file.
    let same_file =
        |entry: &Denied| !directory && entry.file == Some((status.st_dev, status.st_ino));
    if denied
        .iter()
        .any(|entry| place.lies_in(&entry.place) || same_file(entry))
    {
        return Ok(());
    }
    // Step 2: no denied path lies in it.
    if !directory || !denied.iter().any(|entry| entry.place.lies_in(place)) {
        return rules.execute(descriptor, path);
    }
    // Step 3: no rule on the directory, and the same steps for each entry.
    let mount = crate::place::mount_id_of(descriptor.as_raw_fd());
    for name in entries(descriptor, path)? {
        let child_path = format!("{}/{}", path.trim_end_matches('/'), name.to_string_lossy());
        let child = match open_at(descriptor.as_raw_fd(), &name, libc::O_NOFOLLOW) {
            Ok(child) => child,
            // Gone since the directory was read: nothing to grant.
            Err(error) if error.raw_os_error() == Some(libc::ENOENT) => continue,
            Err(error) => return Err(format!("cannot open {child_path:?}: {error}")),
        };
        let child_status =
            self::status(&child).map_err(|error| format!("cannot stat {child_path:?}: {error}"))?;
        // A symbolic link gets no rule: what it points at has its own place in the walk, or
        // is outside the list.
        if kind(&child_status) == libc::S_IFLNK {
            continue;
        }
        // In the directory's own mount an entry has the directory's place and its own name;
        // a mount on the entry has the place its own row gives it.
        let child_mount = crate::place::mount_id_of(child.as_raw_fd());
        let child_place = match (name.to_str(), child_mount) {
            (Ok(name), Some(_)) if child_mount == mount => place.entry(name),
            _ => table
                .place_of(&Opened::of_descriptor(child.as_raw_fd(), &child_path))
                .map_err(|why| why.to_string())?,
        };
        grant(
            rules,
            table,
            denied,
            &Granted {
                path: child_path,
                descriptor: child,
                place: child_place,
            },
        )?;
    }
    Ok(())
}

/// Applies the ruleset of `lists` to this process: refer on `/`, execute on the program, the
/// loader and each entry, less what the deny-list takes away (D11). `Ok` only after
/// `landlock_restrict_self` returned 0.
pub(crate) fn restrict(lists: &Lists) -> Result<(), String> {
    let mut view = Writable {
        roots: &lists.writable,
        placed: None,
    };
    let targets = execute_targets(lists, &mut view)?;
    let denied = denied_paths(lists, &mut view)?;
    let ruleset = create_ruleset()?;
    let root = open_at(libc::AT_FDCWD, c"/", libc::O_DIRECTORY)
        .map_err(|error| format!("cannot open \"/\": {error}"))?;
    add_rule(&ruleset, &root, ACCESS_FS_REFER, "\"/\"")?;
    let mut rules = Rules {
        ruleset: &ruleset,
        written: 0,
    };
    if denied.is_empty() {
        for (path, descriptor) in &targets {
            rules.execute(descriptor, path)?;
        }
    } else {
        for (path, descriptor) in targets {
            // The table was read for the first deny entry.
            let (table, _) = view.placed(&path).map_err(|why| why.to_string())?;
            let place = table
                .place_of(&Opened::of_descriptor(descriptor.as_raw_fd(), &path))
                .map_err(|why| why.to_string())?;
            grant(
                &mut rules,
                table,
                &denied,
                &Granted {
                    path,
                    descriptor,
                    place,
                },
            )?;
        }
    }
    // SAFETY: `prctl` with plain integer arguments.
    if unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) } != 0 {
        return Err(format!(
            "cannot set no_new_privs: {}",
            io::Error::last_os_error()
        ));
    }
    // SAFETY: `ruleset` is a live ruleset descriptor.
    if unsafe { libc::syscall(libc::SYS_landlock_restrict_self, ruleset.as_raw_fd(), 0) } != 0 {
        return Err(format!(
            "the kernel refused to apply the ruleset: {}",
            io::Error::last_os_error()
        ));
    }
    Ok(())
}
