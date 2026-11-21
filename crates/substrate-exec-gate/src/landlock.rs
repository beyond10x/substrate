//! The ruleset: opening each path as its class says, and the three Landlock system calls.
//!
//! Every descriptor opened here is close-on-exec, and the kernel creates the ruleset's descriptor
//! close-on-exec, so the run's program inherits none of them.

use std::collections::VecDeque;
use std::ffi::{CStr, CString};
use std::io;
use std::os::fd::{AsRawFd as _, FromRawFd as _, OwnedFd, RawFd};

use crate::place::{MountTable, MountTree, Opened, Reach, Unestablished, Unlisted};
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

/// Every path that receives the execute right, opened as its class says: the program, the loader
/// and each entry. The next story's deny check belongs here, on every one of them (D23).
fn execute_targets(lists: &Lists) -> Result<Vec<(String, OwnedFd)>, String> {
    // The view's mount table and the place of every mount the run can write, established once
    // and before the first rule (D25). Not read where no followed path could resolve into a
    // writable mount: there is nothing to ask it.
    let mut view = Writable {
        roots: &lists.writable,
        placed: None,
    };
    let mut targets = Vec::new();
    for (option, path) in std::iter::once(("--program", &lists.program))
        .chain(lists.allow.iter().map(|entry| ("--allow", entry)))
    {
        let descriptor = match path.class() {
            Class::Beneath => open_beneath(path)?,
            Class::Follow => open_follow(option, &path.path(), &mut view)?,
        };
        targets.push((path.path(), descriptor));
    }
    if let Some(loader) = &lists.loader {
        targets.push((loader.clone(), open_follow("--loader", loader, &mut view)?));
    }
    Ok(targets)
}

/// The mounts of the view the run can write, and once they were needed the view's mount table
/// with what each reaches (D30): its own place, and the root of every mount that is mounted
/// beneath it.
struct Writable<'a> {
    roots: &'a [String],
    placed: Option<(MountTree, Vec<Reach>)>,
}

impl Writable<'_> {
    /// The table and what each root reaches, established at the first call.
    fn placed(&mut self, path: &str) -> Result<&(MountTree, Vec<Reach>), Unestablished> {
        if self.placed.is_none() {
            // The table before any path: D26 names its causes first.
            let table = MountTable::read().map_err(|cause| Unestablished {
                path: path.to_owned(),
                cause,
                asked: crate::place::Asked::Place,
            })?;
            // The table as a tree, asked whole before any root is asked what it reaches.
            let tree = table.into_tree()?;
            // A mount beneath a writable root of a type outside the list is not compared and
            // does not refuse: bubblewrap binds each device node of `/dev` as one. A followed
            // path that ends on such a type, or reads a symbolic link there, has no place and
            // is refused for that.
            let places = self
                .roots
                .iter()
                .map(|root| {
                    let place = tree.table().place_of(&Opened::of_path(root)?)?;
                    tree.reaches(&place, Unlisted::Skipped)
                })
                .collect::<Result<Vec<_>, _>>()?;
            self.placed = Some((tree, places));
        }
        Ok(self.placed.as_ref().expect("established above"))
    }
}

/// The symbolic links one path may resolve through: the kernel's own bound.
const MAX_LINKS: usize = 40;

/// The longest target of a symbolic link: `PATH_MAX`.
const LINK_BYTES: usize = 4096;

/// The target of the symbolic link `link` is, read from the link itself.
fn link_target(link: &OwnedFd) -> io::Result<Vec<u8>> {
    let mut target = vec![0_u8; LINK_BYTES];
    // SAFETY: the empty path names the link the descriptor is; the buffer is live and its length
    // is the one passed.
    let length = unsafe {
        libc::readlinkat(
            link.as_raw_fd(),
            c"".as_ptr(),
            target.as_mut_ptr().cast(),
            target.len(),
        )
    };
    let length = usize::try_from(length).map_err(|_| io::Error::last_os_error())?;
    if length == 0 {
        return Err(io::Error::from_raw_os_error(libc::ENOENT));
    }
    target.truncate(length);
    Ok(target)
}

/// Opens `path` as the kernel resolves it, one name at a time and with no link followed by the
/// kernel, so that **every symbolic link the walk reads is seen before its target is taken**:
/// `link` is given the link's own descriptor, and the target is read from that descriptor. An
/// absolute target starts again at the root of the view.
fn walk(
    path: &str,
    mut link: impl FnMut(&OwnedFd) -> Result<(), String>,
) -> Result<OwnedFd, String> {
    let unopened = |error: io::Error| format!("cannot open {path:?}: {error}");
    let names = |text: &[u8]| -> Vec<Vec<u8>> {
        text.split(|byte| *byte == b'/')
            .filter(|name| !name.is_empty())
            .map(<[u8]>::to_vec)
            .collect()
    };
    let start = if path.starts_with('/') { c"/" } else { c"." };
    let mut current = open_at(libc::AT_FDCWD, start, libc::O_DIRECTORY).map_err(unopened)?;
    let mut pending: VecDeque<Vec<u8>> = names(path.as_bytes()).into();
    let mut links = 0;
    while let Some(name) = pending.pop_front() {
        let name = CString::new(name).map_err(|_| format!("{path:?} holds a NUL byte"))?;
        let next = open_at(current.as_raw_fd(), &name, libc::O_NOFOLLOW).map_err(unopened)?;
        if kind(&status(&next).map_err(unopened)?) != libc::S_IFLNK {
            current = next;
            continue;
        }
        links += 1;
        if links > MAX_LINKS {
            return Err(unopened(io::Error::from_raw_os_error(libc::ELOOP)));
        }
        link(&next)?;
        let target = link_target(&next).map_err(unopened)?;
        if target.starts_with(b"/") {
            current = open_at(libc::AT_FDCWD, c"/", libc::O_DIRECTORY).map_err(unopened)?;
        }
        for name in names(&target).into_iter().rev() {
            pending.push_front(name);
        }
    }
    Ok(current)
}

/// Opens a `follow` path, following symbolic links, and refuses one that resolves into a mount
/// the run can write (D24): what such a path names is a file the run can replace. Where the
/// path lies is decided by its place (D25): the mount the kernel reports for the descriptor,
/// in the view's mount table. A writable mount is every place it reaches (D30), so a path that
/// resolves onto a mount beneath one is refused as well.
///
/// **Every symbolic link the path resolves through is placed too**, and refused in a place a
/// writable mount reaches: the run can replace the link. A place that cannot be established
/// refuses, with its cause (D26), for a link as for the path's end: a link on a mount of a
/// file-system type outside the list has no place, and such a mount beneath a writable mount
/// is compared with nothing, so the link may be the run's. The text of two paths decides
/// nothing.
fn open_follow(option: &str, path: &str, view: &mut Writable<'_>) -> Result<OwnedFd, String> {
    if view.roots.is_empty() {
        // Nothing of the view is the run's to write: no table is read and no link is placed.
        return open_at(libc::AT_FDCWD, &c_path(path)?, 0)
            .map_err(|error| format!("cannot open {path:?}: {error}"));
    }
    let roots = view.roots;
    let (tree, reached) = view.placed(path).map_err(|why| why.to_string())?;
    // The writable mount `opened` lies in, by place.
    let within = |opened: &Opened| -> Result<Option<String>, String> {
        let place = tree
            .table()
            .place_of(opened)
            .map_err(|why| why.to_string())?;
        Ok(roots
            .iter()
            .zip(reached)
            .find(|(_, reach)| reach.holds(&place))
            .map(|(root, _)| root.clone()))
    };
    let descriptor = walk(path, |link| {
        // Named by the path the kernel holds for the link itself.
        let mut opened = Opened::of_descriptor(link.as_raw_fd(), path);
        if let Some(own) = &opened.descriptor_path {
            opened.path.clone_from(own);
        }
        match within(&opened)? {
            Some(root) => Err(format!(
                "{option} {path} resolves through the symbolic link {}, in the writable mount \
                 {root}",
                opened.path
            )),
            None => Ok(()),
        }
    })?;
    let opened = Opened::of_descriptor(descriptor.as_raw_fd(), path);
    if let Some(root) = within(&opened)? {
        // The path the kernel holds for the descriptor: what the links resolved to.
        let resolved = opened.descriptor_path.as_deref().unwrap_or(path);
        return Err(format!(
            "{option} {path} resolves to {resolved}, in the writable mount {root}"
        ));
    }
    Ok(descriptor)
}

/// Applies the ruleset of `lists` to this process: refer on `/`, execute on the program, the
/// loader and each entry. `Ok` only after `landlock_restrict_self` returned 0.
pub(crate) fn restrict(lists: &Lists) -> Result<(), String> {
    let targets = execute_targets(lists)?;
    let ruleset = create_ruleset()?;
    let root = open_at(libc::AT_FDCWD, c"/", libc::O_DIRECTORY)
        .map_err(|error| format!("cannot open \"/\": {error}"))?;
    add_rule(&ruleset, &root, ACCESS_FS_REFER, "\"/\"")?;
    for (path, descriptor) in &targets {
        add_rule(
            &ruleset,
            descriptor,
            ACCESS_FS_EXECUTE,
            &format!("{path:?}"),
        )?;
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
