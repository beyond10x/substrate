//! The ruleset: opening each path as its class says, and the three Landlock system calls.
//!
//! Every descriptor opened here is close-on-exec, and the kernel creates the ruleset's descriptor
//! close-on-exec, so the run's program inherits none of them.

use std::collections::VecDeque;
use std::ffi::{CStr, CString};
use std::io;
use std::os::fd::{AsRawFd as _, FromRawFd as _, OwnedFd, RawFd};

use crate::place::{MountTable, MountTree, Opened, Place, Reach, Unestablished, Unlisted};
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
            Class::Beneath => {
                let descriptor = open_beneath(path)?;
                place_end(&descriptor, &path.path(), view)?;
                descriptor
            }
            Class::Follow => open_follow(option, &path.path(), view)?,
        };
        targets.push((path.path(), descriptor));
    }
    if let Some(loader) = &lists.loader {
        targets.push((loader.clone(), open_follow("--loader", loader, view)?));
    }
    Ok(targets)
}

/// **Every path is placed where it ends, whatever its class** (D26). A `beneath` path is one
/// in a mount the run can write, so nothing is asked of its place but that it has one: an end
/// on a mount of a file-system type outside the list is compared with nothing, and is refused
/// as it is for a `follow` path. Where the view has no writable mount no table is read, as for
/// a `follow` path.
fn place_end(descriptor: &OwnedFd, path: &str, view: &mut Writable<'_>) -> Result<(), String> {
    if view.roots.is_empty() {
        return Ok(());
    }
    let (tree, _) = view.placed(path).map_err(|why| why.to_string())?;
    tree.table()
        .place_of(&Opened::of_descriptor(descriptor.as_raw_fd(), path))
        .map(drop)
        .map_err(|why| why.to_string())
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

/// What the walk of a followed path read on its way to the path's end.
enum Read<'a> {
    /// A symbolic link, before its target is taken.
    Link(&'a OwnedFd),
    /// A directory the path passes through: more names follow it.
    Passed(&'a OwnedFd),
}

/// Opens `path` one name at a time, with no link followed by the kernel, so that **everything
/// the walk reads is seen before the walk goes on**: `read` is given the descriptor of every
/// symbolic link, before its target is read from that descriptor, and of every directory the
/// path passes through. An absolute target starts again at the root of the view.
///
/// **`..` asks the kernel nothing.** The walk holds the descriptor of every directory from
/// its start to where it stands, none of them reached through a link, and `..` drops the
/// last. The driver resolves the same text the same way before dispatch, so the two end at
/// the same file. A `..` asked of the kernel from a held descriptor names whatever the
/// directory's parent is at that moment: a second run that shares a writable mount could
/// rename the directory between the two system calls, and the walk would end one level off.
fn walk(
    path: &str,
    mut read: impl FnMut(Read<'_>) -> Result<(), String>,
) -> Result<OwnedFd, String> {
    let unopened = |error: io::Error| format!("cannot open {path:?}: {error}");
    let names = |text: &[u8]| -> Vec<Vec<u8>> {
        text.split(|byte| *byte == b'/')
            .filter(|name| !name.is_empty())
            .map(<[u8]>::to_vec)
            .collect()
    };
    let root = || open_at(libc::AT_FDCWD, c"/", libc::O_DIRECTORY).map_err(unopened);
    let mut rooted = path.starts_with('/');
    // The directories from the start of the walk to where it stands.
    let mut held = vec![if rooted {
        root()?
    } else {
        open_at(libc::AT_FDCWD, c".", libc::O_DIRECTORY).map_err(unopened)?
    }];
    let mut pending: VecDeque<Vec<u8>> = names(path.as_bytes()).into();
    let mut links = 0;
    while let Some(name) = pending.pop_front() {
        match name.as_slice() {
            b"." => continue,
            b".." => {
                if held.len() > 1 {
                    held.pop();
                } else if !rooted {
                    // Above the directory a relative path starts in, which the walk does
                    // not hold: only the kernel could say what that is.
                    return Err(format!(
                        "cannot open {path:?}: it leaves the directory it starts in"
                    ));
                }
                continue;
            }
            _ => {}
        }
        let name = CString::new(name).map_err(|_| format!("{path:?} holds a NUL byte"))?;
        let here = held.last().expect("the start of the walk is never dropped");
        let next = open_at(here.as_raw_fd(), &name, libc::O_NOFOLLOW).map_err(unopened)?;
        if kind(&status(&next).map_err(unopened)?) != libc::S_IFLNK {
            if !pending.is_empty() {
                read(Read::Passed(&next))?;
            }
            held.push(next);
            continue;
        }
        links += 1;
        if links > MAX_LINKS {
            return Err(unopened(io::Error::from_raw_os_error(libc::ELOOP)));
        }
        read(Read::Link(&next))?;
        let target = link_target(&next).map_err(unopened)?;
        if target.starts_with(b"/") {
            rooted = true;
            held.clear();
            held.push(root()?);
        }
        for name in names(&target).into_iter().rev() {
            pending.push_front(name);
        }
    }
    Ok(held.pop().expect("the start of the walk is never dropped"))
}

/// Opens a `follow` path, following symbolic links, and refuses one that resolves into a mount
/// the run can write (D24): what such a path names is a file the run can replace. Where the
/// path lies is decided by its place (D25): the mount the kernel reports for the descriptor,
/// in the view's mount table. A writable mount is every place it reaches (D30), so a path that
/// resolves onto a mount beneath one is refused as well.
///
/// **Everything the path resolves through is placed too.** A symbolic link in a place a
/// writable mount reaches is refused: the run can replace the link. So is a path that passes
/// through a directory there, although no link lies in it and the path leaves it again: the
/// run can rename what the directory holds while a path is resolved through it. A path that
/// never passes through a place the run can write cannot be moved by the run.
///
/// A place that cannot be established refuses, with its cause (D26), for a link and for a
/// directory on the way as for the path's end: on a mount of a file-system type outside the
/// list neither has a place, and such a mount beneath a writable mount is compared with
/// nothing, so either may be the run's. The text of two paths decides nothing.
///
/// A link refuses where the walk reads it. A directory on the way refuses once the walk
/// ended and the end was judged, so the line names the link or the end where one of them is
/// refused too: the walk only opens and reads on its way there, and nothing is executed.
fn open_follow(option: &str, path: &str, view: &mut Writable<'_>) -> Result<OwnedFd, String> {
    if view.roots.is_empty() {
        // Nothing of the view is the run's to write: no table is read and nothing is placed.
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
    // Why the first directory on the way refuses the path: the run can write it, or it has
    // no place. Held until the end was judged, so a path that ends where the run can write,
    // or on a mount where nothing has a place, keeps the line it had.
    let mut through = None;
    let descriptor = walk(path, |read| {
        let (descriptor, link) = match read {
            Read::Link(descriptor) => (descriptor, true),
            Read::Passed(descriptor) => (descriptor, false),
        };
        if !link && through.is_some() {
            return Ok(());
        }
        // Named by the path the kernel holds for the link or the directory itself.
        let mut opened = Opened::of_descriptor(descriptor.as_raw_fd(), path);
        if let Some(own) = &opened.descriptor_path {
            opened.path.clone_from(own);
        }
        let root = match within(&opened) {
            Ok(Some(root)) => root,
            Ok(None) => return Ok(()),
            Err(unplaced) if link => return Err(unplaced),
            Err(unplaced) => {
                through = Some(unplaced);
                return Ok(());
            }
        };
        if link {
            return Err(format!(
                "{option} {path} resolves through the symbolic link {}, in the writable mount \
                 {root}",
                opened.path
            ));
        }
        through = Some(format!(
            "{option} {path} passes through {}, in the writable mount {root}",
            opened.path
        ));
        Ok(())
    })?;
    let opened = Opened::of_descriptor(descriptor.as_raw_fd(), path);
    if let Some(root) = within(&opened)? {
        // The path the kernel holds for the descriptor: what the links resolved to.
        let resolved = opened.descriptor_path.as_deref().unwrap_or(path);
        return Err(format!(
            "{option} {path} resolves to {resolved}, in the writable mount {root}"
        ));
    }
    through.map_or(Ok(descriptor), Err)
}

/// A file by its device and inode: what another name of it has too.
type FileId = (libc::dev_t, libc::ino_t);

/// One entry of the deny-list, where it is: **every place it reaches** (D30), which is its own
/// place and the root of every mount that is mounted beneath it, and for a file that exists
/// the device and inode another name of it has too (D11 step 1).
#[derive(Debug)]
struct Denied {
    reaches: Reach,
    file: Option<FileId>,
}

impl Denied {
    /// The deny entry at `place`. What it reaches is asked as of a directory whose files the
    /// profile denies wherever they show: a mount beneath it that cannot be compared refuses,
    /// and so does one of a file-system type outside the list (D30's table).
    fn at(tree: &MountTree, place: &Place, file: Option<FileId>) -> Result<Self, Unestablished> {
        Ok(Self {
            reaches: tree.reaches(place, Unlisted::Refused)?,
            file,
        })
    }
}

/// Whether `file` is a denied file under another name: the same device and the same inode. A
/// directory, and a deny entry that is a directory or does not exist, has no such identity.
fn another_name(denied: &[Denied], file: Option<FileId>) -> bool {
    file.is_some() && denied.iter().any(|entry| entry.file == file)
}

/// What D11 does with one path that is to receive the execute right.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    /// Step 1: no rule.
    Denied,
    /// Step 2: one rule on the path.
    Rule,
    /// Step 3: no rule on the directory, and the same steps for each of its entries.
    Expand,
}

/// **D11 with D30**: the step for the path at `place`, which is the file `file` or, with
/// `None`, a directory.
///
/// 1. No rule where the place lies in a denied place, or the file is a denied file under
///    another name. The denied places are what each deny entry reaches: a listed mount
///    beneath a denied directory lies in one.
/// 2. One rule for a file, and for a directory none of whose places overlaps a denied place.
///    The places of a directory are what it reaches: its own, and the root of every mount
///    beneath it, because the kernel's walk from a file in such a mount passes the directory.
/// 3. Otherwise the directory is expanded.
///
/// With no deny entry every path gets its rule and nothing is asked of the table.
///
/// # Errors
///
/// [`Unestablished`] where what the directory reaches cannot be established (D30's table): a
/// mount of a listed type beneath it that is covered, whose root was removed or whose mount
/// point cannot be asked. A mount of a type outside the list beneath a listed directory is
/// compared with nothing and refuses nothing.
fn step(
    tree: &MountTree,
    denied: &[Denied],
    place: &Place,
    file: Option<FileId>,
) -> Result<Step, Unestablished> {
    if denied.is_empty() {
        return Ok(Step::Rule);
    }
    if another_name(denied, file) || denied.iter().any(|entry| entry.reaches.holds(place)) {
        return Ok(Step::Denied);
    }
    if file.is_some() {
        return Ok(Step::Rule);
    }
    let reaches = tree.reaches(place, Unlisted::Skipped)?;
    Ok(
        if denied.iter().any(|entry| reaches.overlaps(&entry.reaches)) {
            Step::Expand
        } else {
            Step::Rule
        },
    )
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

/// Opens a `follow` deny path, or its nearest ancestor that exists, and returns the names
/// after it. What exists is opened as every followed path is ([`open_follow`]): a path that
/// resolves into a mount the run can write, reads a symbolic link there or passes through a
/// directory there is refused, and so is a link or a directory without a place. A name is
/// absent where the kernel says so, `ENOENT` or `ENOTDIR`; every other failure refuses.
fn open_follow_or_ancestor(
    path: &str,
    view: &mut Writable<'_>,
) -> Result<(OwnedFd, Vec<String>), String> {
    let mut existing = path.trim_end_matches('/');
    let mut remaining = Vec::new();
    loop {
        let name = if existing.is_empty() { "/" } else { existing };
        let text = c_path(name)?;
        // SAFETY: `stat` fills the zeroed struct for a NUL-terminated path.
        let mut seen: libc::stat = unsafe { std::mem::zeroed() };
        // SAFETY: as above; `seen` is a valid out pointer.
        if unsafe { libc::stat(text.as_ptr(), &raw mut seen) } == 0 {
            remaining.reverse();
            return Ok((open_follow("--deny", name, view)?, remaining));
        }
        let error = io::Error::last_os_error();
        let absent = matches!(error.raw_os_error(), Some(libc::ENOENT | libc::ENOTDIR));
        let Some((parent, last)) = existing.rsplit_once('/') else {
            return Err(format!("cannot open {path:?}: {error}"));
        };
        if !absent || existing.is_empty() {
            return Err(format!("cannot open {path:?}: {error}"));
        }
        remaining.push(last.to_owned());
        existing = parent;
    }
}

/// Where every entry of the deny-list is, and what it reaches (D11, D25, D30). A `follow`
/// entry that resolves into a mount the run can write is refused, as every followed path is
/// (D24).
fn denied_paths(lists: &Lists, view: &mut Writable<'_>) -> Result<Vec<Denied>, String> {
    let mut denied = Vec::new();
    for entry in &lists.deny {
        let path = entry.path();
        let (descriptor, remaining) = match entry.class() {
            Class::Beneath => open_beneath_or_ancestor(entry)?,
            Class::Follow => open_follow_or_ancestor(&path, view)?,
        };
        let status =
            status(&descriptor).map_err(|error| format!("cannot stat {path:?}: {error}"))?;
        // An entry that does not exist is a name: what was opened is its ancestor, another
        // file, and a file of that ancestor's identity is not what the entry denies.
        let file = (remaining.is_empty() && kind(&status) != libc::S_IFDIR)
            .then_some((status.st_dev, status.st_ino));
        let mut opened = Opened::of_descriptor(descriptor.as_raw_fd(), &path);
        opened.remaining = remaining;
        let (tree, _) = view.placed(&path).map_err(|why| why.to_string())?;
        let place = tree
            .table()
            .place_of(&opened)
            .map_err(|why| why.to_string())?;
        denied.push(Denied::at(tree, &place, file).map_err(|why| why.to_string())?);
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

/// D11: the three steps for `granted` ([`step`]), each entry of an expanded directory going
/// through them in turn. Every comparison is of places (D25), through what a directory
/// reaches in the view's mounts (D30), and of device and inode for another name of a denied
/// file.
fn grant(
    rules: &mut Rules<'_>,
    tree: &MountTree,
    denied: &[Denied],
    granted: &Granted,
) -> Result<(), String> {
    let Granted {
        path,
        descriptor,
        place,
    } = granted;
    let status = status(descriptor).map_err(|error| format!("cannot stat {path:?}: {error}"))?;
    let file = (kind(&status) != libc::S_IFDIR).then_some((status.st_dev, status.st_ino));
    match step(tree, denied, place, file).map_err(|why| why.to_string())? {
        Step::Denied => return Ok(()),
        Step::Rule => return rules.execute(descriptor, path),
        Step::Expand => {}
    }
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
        let own = child_mount.is_some() && child_mount == mount;
        let child_place = match name.to_str() {
            Ok(name) if own => place.entry(name),
            Err(_) if own => {
                // **A name that is not text**, in the directory's own mount. No place has
                // such a name: an entry of the deny-list is text, and so is every row of a
                // table that was parsed. So nothing denied is this entry, lies in it or is
                // mounted beneath it, and what is left of step 1 is the file itself under
                // another name. It gets its rule like any other entry, a directory one rule.
                let file = (kind(&child_status) != libc::S_IFDIR)
                    .then_some((child_status.st_dev, child_status.st_ino));
                if !another_name(denied, file) {
                    rules.execute(&child, &child_path)?;
                }
                continue;
            }
            _ => tree
                .table()
                .place_of(&Opened::of_descriptor(child.as_raw_fd(), &child_path))
                .map_err(|why| why.to_string())?,
        };
        grant(
            rules,
            tree,
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
            let (tree, _) = view.placed(&path).map_err(|why| why.to_string())?;
            let place = tree
                .table()
                .place_of(&Opened::of_descriptor(descriptor.as_raw_fd(), &path))
                .map_err(|why| why.to_string())?;
            grant(
                &mut rules,
                tree,
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

#[cfg(test)]
mod tests {
    use std::os::fd::{AsRawFd as _, OwnedFd};
    use std::path::PathBuf;

    use super::{Denied, FileId, Read, Step, step, walk};
    use crate::place::{Cause, MountTable, MountTree, Place, Unestablished};

    /// A view with what the reviews of the deny-list staged (design 35 § 3.13, E18 to E22),
    /// written as a mount table. Beneath the listed `/t`: another file system at `/t/held`
    /// (E18) and `/t/bin` bound a second time at `/t/one/deep` (E21 (a)). Beneath `/d`: a
    /// third file system at `/d/sub` (E19), mounted a second time at `/elsewhere`. A chain
    /// of three at `/c`, `/c/b` and `/c/b/a`. Beneath `/u` a `vfat` mount, beneath `/s` two
    /// mounts stacked on one mount point, and beneath `/g` a mount whose root was removed.
    const VIEW: &str = "\
10 1 8:1 / / rw - ext4 /dev/a rw
20 10 8:2 / /t/held rw - tmpfs tmpfs rw
21 10 8:1 /t/bin /t/one/deep rw - ext4 /dev/a rw
22 10 8:3 / /d/sub rw - ext4 /dev/c rw
23 10 8:3 / /elsewhere rw - ext4 /dev/c rw
24 10 8:6 / /c rw - ext4 /dev/g rw
25 24 8:7 / /c/b rw - ext4 /dev/h rw
26 25 8:8 / /c/b/a rw - ext4 /dev/i rw
27 10 0:40 / /u/boot rw - vfat /dev/d rw
30 10 8:4 / /s/m rw - ext4 /dev/e rw
31 30 8:5 / /s/m rw - ext4 /dev/f rw
32 10 8:1 /gone//deleted /g/m rw - ext4 /dev/a rw
";

    fn view() -> MountTree {
        MountTable::parse(VIEW)
            .expect("the table")
            .into_tree_as_written()
            .expect("the tree")
    }

    /// The place of `path`, which lies on the mount `id`.
    fn on(tree: &MountTree, id: u64, path: &str) -> Place {
        tree.table()
            .place(Some(id), Some(path))
            .expect("the place of a path of the table")
    }

    /// A deny entry at `path` on the mount `id`: a directory, or the file `file`.
    fn deny(tree: &MountTree, id: u64, path: &str, file: Option<FileId>) -> Denied {
        Denied::at(tree, &on(tree, id, path), file).expect("what the deny entry reaches")
    }

    fn unreached(result: Result<Step, Unestablished>) -> (String, Cause) {
        let why = result.expect_err("a directory whose reach cannot be established");
        assert!(
            why.to_string().starts_with("cannot establish where "),
            "the line names an attachment: {why}"
        );
        (why.path, why.cause)
    }

    /// **D11 with D30, the three steps on a table written as text** (design 35 § 16, *the
    /// six checks*): a listed directory with a denied path in a mount beneath it is expanded,
    /// a listed mount beneath a denied directory gets no rule, and a listed directory with
    /// no mount beneath it and no denied place in it gets one rule. A comparison of places
    /// alone answers the first two with one rule: the place of a file in the mount at
    /// `/t/held` is on another device than `/t`.
    #[test]
    fn the_three_steps_follow_the_views_mounts() {
        let tree = view();
        let file = Some((8, 1));
        let directory = None;

        // E18: `/t` listed, `/t/held/tool` denied, `/t/held` another file system.
        let denied = [deny(&tree, 20, "/t/held/tool", file)];
        let step_of = |id: u64, path: &str, file: Option<FileId>| {
            step(&tree, &denied, &on(&tree, id, path), file)
        };
        assert_eq!(step_of(10, "/t", directory), Ok(Step::Expand));
        // The mount point among the entries has the place its own row gives it.
        assert_eq!(step_of(20, "/t/held", directory), Ok(Step::Expand));
        assert_eq!(step_of(20, "/t/held/tool", file), Ok(Step::Denied));
        assert_eq!(step_of(20, "/t/held/other", Some((8, 2))), Ok(Step::Rule));
        // A directory of `/t` that holds no mount and no denied place: one rule.
        assert_eq!(step_of(10, "/t/bin", directory), Ok(Step::Rule));
        // And a listed directory beside it all.
        assert_eq!(step_of(10, "/plain", directory), Ok(Step::Rule));

        // E21 (a): `/t/one` listed, `/t/bin/tool` denied, `/t/bin` bound at `/t/one/deep`.
        // The deny entry is not on the nested mount; the mount's root holds its place.
        let denied = [deny(&tree, 10, "/t/bin/tool", file)];
        let step_of = |id: u64, path: &str, file: Option<FileId>| {
            step(&tree, &denied, &on(&tree, id, path), file)
        };
        assert_eq!(step_of(10, "/t/one", directory), Ok(Step::Expand));
        assert_eq!(step_of(21, "/t/one/deep", directory), Ok(Step::Expand));
        assert_eq!(step_of(21, "/t/one/deep/tool", file), Ok(Step::Denied));
        assert_eq!(step_of(10, "/t/held", directory), Ok(Step::Rule));

        // E19, K8 across a mount: `/d` denied, the mount at `/d/sub` listed. No rule, and
        // none for a path in a second mount of that file system elsewhere (D23).
        let denied = [deny(&tree, 10, "/d", directory)];
        let step_of = |id: u64, path: &str, file: Option<FileId>| {
            step(&tree, &denied, &on(&tree, id, path), file)
        };
        assert_eq!(step_of(22, "/d/sub", directory), Ok(Step::Denied));
        assert_eq!(step_of(22, "/d/sub/tool", Some((8, 3))), Ok(Step::Denied));
        assert_eq!(step_of(23, "/elsewhere/bin", directory), Ok(Step::Denied));
        assert_eq!(step_of(10, "/t", directory), Ok(Step::Rule));
        // And the reverse of it: the mount listed where it shows a second time, the deny
        // entry a directory of it at the first. The listed root holds the denied place.
        let denied = [deny(&tree, 22, "/d/sub/bin", directory)];
        let step_of = |id: u64, path: &str| step(&tree, &denied, &on(&tree, id, path), directory);
        assert_eq!(step_of(23, "/elsewhere"), Ok(Step::Expand));
        assert_eq!(step_of(23, "/elsewhere/bin"), Ok(Step::Denied));
        assert_eq!(step_of(23, "/elsewhere/lib"), Ok(Step::Rule));
        assert_eq!(step_of(10, "/d"), Ok(Step::Expand));

        // A chain of three mounts: a file denied in the third expands each directory above.
        let denied = [deny(&tree, 26, "/c/b/a/tool", file)];
        let step_of = |id: u64, path: &str| step(&tree, &denied, &on(&tree, id, path), directory);
        assert_eq!(step_of(24, "/c"), Ok(Step::Expand));
        assert_eq!(step_of(25, "/c/b"), Ok(Step::Expand));
        assert_eq!(step_of(26, "/c/b/a"), Ok(Step::Expand));
        assert_eq!(step_of(24, "/c/x"), Ok(Step::Rule));
        assert_eq!(step_of(10, "/t"), Ok(Step::Rule));
    }

    /// **Another name of a denied file is the same device and the same inode** (D11 step 1).
    /// Two files with one inode number on two devices are two files. A directory has no such
    /// identity, and neither has a deny entry that is a directory or does not exist: `None`
    /// is never "the same file" as `None`.
    #[test]
    fn another_name_of_a_denied_file_is_its_device_and_its_inode() {
        let tree = view();
        let denied = [
            deny(&tree, 10, "/t/bin/tool", Some((8, 7))),
            deny(&tree, 10, "/t/bin/absent", None),
        ];
        let beside = on(&tree, 10, "/plain/name");
        let step_of = |file: Option<FileId>| step(&tree, &denied, &beside, file);
        assert_eq!(step_of(Some((8, 7))), Ok(Step::Denied));
        assert_eq!(step_of(Some((9, 7))), Ok(Step::Rule), "another device");
        assert_eq!(step_of(Some((8, 9))), Ok(Step::Rule), "another inode");
        assert_eq!(step_of(None), Ok(Step::Rule), "a directory");
    }

    /// **Failing closed, by D30's table.** What a denied directory reaches, and what a listed
    /// directory of a start with a deny-list reaches, must be established: a covered mount
    /// and a removed root beneath either refuse with their cause. A mount of a type outside
    /// the list refuses beneath a denied directory only; beneath a listed directory it is
    /// compared with nothing. An allow-list alone asks none of it.
    #[test]
    fn a_reach_that_cannot_be_established_is_a_cause_and_never_a_rule() {
        let tree = view();
        let at = |id: u64, path: &str| on(&tree, id, path);
        let unlisted =
            Denied::at(&tree, &at(10, "/u"), None).expect_err("a vfat mount in a denied directory");
        assert_eq!(
            (unlisted.path.as_str(), &unlisted.cause),
            ("/u/boot", &Cause::FileSystemUnlisted("vfat".to_owned()))
        );
        assert_eq!(
            unlisted.to_string(),
            "cannot establish where /u/boot is attached: file-system-unlisted vfat"
        );
        let covered = Denied::at(&tree, &at(10, "/s"), None).expect_err("a stack beneath it");
        assert_eq!(
            (covered.path.as_str(), &covered.cause),
            ("/s/m", &Cause::MountCovered)
        );
        let deleted = Denied::at(&tree, &at(10, "/g"), None).expect_err("a removed root");
        assert_eq!(
            (deleted.path.as_str(), &deleted.cause),
            ("/g/m", &Cause::MountRootDeleted)
        );

        // A listed directory of a start with a deny-list, whatever the entry denies.
        let denied = [deny(&tree, 10, "/t/bin/tool", Some((8, 7)))];
        let listed = |path: &str| step(&tree, &denied, &at(10, path), None);
        assert_eq!(
            unreached(listed("/s")),
            ("/s/m".to_owned(), Cause::MountCovered)
        );
        assert_eq!(
            unreached(listed("/g")),
            ("/g/m".to_owned(), Cause::MountRootDeleted)
        );
        assert_eq!(
            listed("/u"),
            Ok(Step::Rule),
            "a type outside the list beneath a listed directory is not compared"
        );
        // A file is asked nothing of the table, and a path a deny entry reaches is refused
        // its rule before anything else is asked.
        assert_eq!(
            step(&tree, &denied, &at(10, "/s/file"), Some((8, 1))),
            Ok(Step::Rule)
        );
        // An allow-list alone: one rule per entry on the same table, and no attachment asked.
        for path in ["/s", "/g", "/u", "/t"] {
            assert_eq!(step(&tree, &[], &at(10, path), None), Ok(Step::Rule));
        }

        // A row whose parent no row has refuses the table as a whole, before any step.
        let unattached =
            MountTable::parse(&format!("{VIEW}40 99 8:9 / /lost rw - ext4 /dev/j rw\n"))
                .expect("the table")
                .into_tree_as_written()
                .expect_err("a row without its parent row");
        assert_eq!(
            (unattached.path.as_str(), &unattached.cause),
            ("/lost", &Cause::MountRowMissing)
        );
    }

    /// The path the kernel holds for `descriptor`.
    fn held(descriptor: &OwnedFd) -> PathBuf {
        std::fs::read_link(format!("/proc/self/fd/{}", descriptor.as_raw_fd()))
            .expect("the path of a descriptor")
    }

    /// **`..` steps back to the directory the walk came through, and asks the kernel
    /// nothing.** The path is `a/b/../../c`. Nothing moved, it ends at `c`, where the kernel
    /// ends. With `a/b` moved to `deep/er/b` once the walk holds `b`, and before the `..`
    /// that follows, it ends at `c` all the same: a `..` asked of the kernel from `b` would
    /// name `deep/er`, and the walk would end at `deep/c`, a directory the path does not
    /// name. The driver resolves the same text before dispatch and judged `c`.
    #[test]
    fn dot_dot_steps_back_to_the_directory_the_walk_came_through() {
        let scratch = tempfile::tempdir().expect("scratch");
        let root = scratch.path().canonicalize().expect("canonical scratch");
        for name in ["a/b", "c", "deep/er", "deep/c"] {
            std::fs::create_dir_all(root.join(name)).expect(name);
        }
        let path = root.join("a/b/../../c");
        let path = path.to_str().expect("UTF-8");
        let end = walk(path, |_| Ok(())).expect("the path with nothing moved");
        assert_eq!(held(&end), root.join("c"));
        let mut moved = false;
        let end = walk(path, |read| {
            let holds_b =
                matches!(read, Read::Passed(directory) if held(directory) == root.join("a/b"));
            if holds_b {
                std::fs::rename(root.join("a/b"), root.join("deep/er/b")).expect("b moved");
                moved = true;
            }
            Ok(())
        })
        .expect("the path with a directory moved under the walk");
        assert!(moved, "the walk never held a/b");
        assert_eq!(
            held(&end),
            root.join("c"),
            "the walk ended where the kernel's `..` leads after the move"
        );
        // Above the root there is nothing: `..` stays, as the kernel's does.
        let end = walk("/../..", |_| Ok(())).expect("the root");
        assert_eq!(held(&end), PathBuf::from("/"));
        // A relative path may not leave the directory it starts in: the walk does not hold
        // what is above it.
        let error = walk("..", |_| Ok(())).expect_err("above the start");
        assert!(
            error.contains("it leaves the directory it starts in"),
            "{error}"
        );
    }
}
