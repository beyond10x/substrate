//! The place of a path (design 35 D25, D26): whether one path lies in another is decided by where
//! their mounts put them, never by their text.
//!
//! - **The path's mount** is the one the kernel names: the path is opened, `statx` with
//!   `STATX_MNT_ID` reports the mount ID of the descriptor, and the mount is the one row of
//!   `/proc/self/mountinfo` whose first field is that ID. Never the row with the longest matching
//!   mount point: a mount hidden by a later one keeps its row.
//! - **The place** is that row's device, and its root joined with the rest of the descriptor's
//!   path below the row's mount point. A path that does not exist has the place of its nearest
//!   existing ancestor, joined with the remaining names. Only a name is ever joined: `.` and
//!   `..` are resolved by the kernel where the path exists, and are no place where it does not.
//! - **One path lies in another** when both places have the same device and the first place is
//!   the second or lies beneath it. So `/bin/id` lies in `/usr` where `/bin` is a mount of
//!   `/usr/bin`, and a directory the host mounts at a second path is found under both.
//!
//! **Where a place cannot be established, the answer is a [`Cause`], never a boolean and never a
//! comparison of path text.** The causes, in the order they are checked:
//!
//! | cause | when |
//! |---|---|
//! | `mount-table-unreadable` | `/proc/self/mountinfo` cannot be opened or read |
//! | `mount-table-unparsable` | a row lacks its ID, device, root or mount point, has no ` - ` separator or no type after it, or holds an escape that is not three octal digits; or the table was read and is not UTF-8 text, as with a mount point that holds other bytes |
//! | `path-unopenable` | the path cannot be opened for a reason other than not existing, no ancestor of it exists, or a name of it that does not exist yet is `.` or `..` |
//! | `mount-id-unreported` | `statx` fails, or its result does not have `STATX_MNT_ID` set |
//! | `mount-row-missing` | no row has the reported ID |
//! | `mount-row-repeated` | more than one row has it |
//! | `mount-root-deleted` | the row's root ends in `//deleted`: the directory the mount shows was removed |
//! | `mount-point-mismatch` | the descriptor's path cannot be read, or the row's mount point is not that path or a prefix of it |
//! | `file-system-unlisted` | the row's file-system type is not one of [`LISTED_FILE_SYSTEMS`] |
//!
//! [`place`] is the one function that establishes a place, from the reported mount ID, the
//! descriptor's path and the table's text. The helper and the driver compile this file, so the
//! two cannot disagree. The helper reads the table of the run's view; the driver reads its own,
//! for host paths.

use std::fmt;

/// The closed list of file-system types the place rule was observed to hold on (design 35
/// § 3.9), compared as exact text with the type field of a path's row. For a bind mount that is
/// the type of the file system the mount shows. Adding a type is a change to design 35.
pub const LISTED_FILE_SYSTEMS: [&str; 3] = ["ext4", "overlay", "tmpfs"];

/// `STATX_MNT_ID` (Linux 5.8).
#[cfg(target_os = "linux")]
const STATX_MNT_ID: u32 = 0x1000;

/// Why a place could not be established (D26).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cause {
    MountTableUnreadable,
    MountTableUnparsable,
    PathUnopenable,
    MountIdUnreported,
    MountRowMissing,
    MountRowRepeated,
    MountRootDeleted,
    MountPointMismatch,
    /// With the type the row has.
    FileSystemUnlisted(String),
}

impl Cause {
    /// The cause's name, as every refusal writes it.
    pub const fn name(&self) -> &'static str {
        match self {
            Self::MountTableUnreadable => "mount-table-unreadable",
            Self::MountTableUnparsable => "mount-table-unparsable",
            Self::PathUnopenable => "path-unopenable",
            Self::MountIdUnreported => "mount-id-unreported",
            Self::MountRowMissing => "mount-row-missing",
            Self::MountRowRepeated => "mount-row-repeated",
            Self::MountRootDeleted => "mount-root-deleted",
            Self::MountPointMismatch => "mount-point-mismatch",
            Self::FileSystemUnlisted(_) => "file-system-unlisted",
        }
    }
}

impl fmt::Display for Cause {
    /// The name, and the type after `file-system-unlisted`.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FileSystemUnlisted(file_system) => {
                write!(formatter, "{} {file_system}", self.name())
            }
            other => formatter.write_str(other.name()),
        }
    }
}

/// A place that could not be established: the path as it was asked, and the cause.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unestablished {
    pub path: String,
    pub cause: Cause,
}

impl fmt::Display for Unestablished {
    /// The helper's diagnostic line after its own name.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "cannot establish the place of {}: {}",
            self.path, self.cause
        )
    }
}

/// How one place stands to another.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Relation {
    /// Neither lies in the other.
    Apart,
    /// The first is the other or lies in it.
    Within,
    /// The other lies in the first.
    Holds,
}

/// Where a path is: the device of its mount, and its names from the root of that file system.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Place {
    device: String,
    names: Vec<String>,
}

impl Place {
    /// How this place stands to `other`.
    pub fn relation(&self, other: &Self) -> Relation {
        if self.device != other.device {
            Relation::Apart
        } else if self.names.starts_with(&other.names) {
            Relation::Within
        } else if other.names.starts_with(&self.names) {
            Relation::Holds
        } else {
            Relation::Apart
        }
    }

    /// Whether this place is `other` or lies beneath it.
    pub fn lies_in(&self, other: &Self) -> bool {
        self.relation(other) == Relation::Within
    }
}

/// What the kernel reported for an opened path: the inputs of [`place`] beside the table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Opened {
    /// The path as it was asked, which a refusal names.
    pub path: String,
    /// The mount ID `statx` reported for the descriptor, where it reported one.
    pub mount_id: Option<u64>,
    /// The descriptor's path (`/proc/self/fd/<n>`), where it could be read.
    pub descriptor_path: Option<String>,
    /// The names of `path` below the descriptor: empty for a path that exists, else what
    /// follows its nearest existing ancestor, which is what was opened.
    pub remaining: Vec<String>,
}

/// One row of the table.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Row {
    id: u64,
    device: String,
    root: String,
    mount_point: String,
    file_system: String,
}

/// A mount table: the parsed text of a `mountinfo` file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MountTable {
    rows: Vec<Row>,
}

/// The names of an absolute path, or `None` for any other text.
fn names(path: &str) -> Option<Vec<String>> {
    let rest = path.strip_prefix('/')?;
    Some(
        rest.split('/')
            .filter(|name| !name.is_empty())
            .map(str::to_owned)
            .collect(),
    )
}

/// A field of the table with its octal escapes (`\040` for a space) undone.
fn unescape(field: &str) -> Option<String> {
    let bytes = field.as_bytes();
    let mut plain = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'\\' {
            let digits = field.get(index + 1..index + 4)?;
            if !digits.bytes().all(|digit| (b'0'..=b'7').contains(&digit)) {
                return None;
            }
            plain.push(u8::from_str_radix(digits, 8).ok()?);
            index += 4;
        } else {
            plain.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(plain).ok()
}

impl MountTable {
    /// Parses the text of a `mountinfo` file.
    ///
    /// # Errors
    ///
    /// [`Cause::MountTableUnparsable`] for a row that lacks its ID, device, root or mount point,
    /// has no ` - ` separator or no type after it, or holds an escape that is not three octal
    /// digits.
    pub fn parse(text: &str) -> Result<Self, Cause> {
        let mut rows = Vec::new();
        for line in text.lines().filter(|line| !line.is_empty()) {
            rows.push(Self::row(line).ok_or(Cause::MountTableUnparsable)?);
        }
        Ok(Self { rows })
    }

    /// One row: mount ID, parent ID, major:minor, root, mount point, options, optional fields,
    /// a separator, the file-system type, the source, the super options.
    fn row(line: &str) -> Option<Row> {
        let (mount, file_system) = line.split_once(" - ")?;
        let fields: Vec<&str> = mount.split(' ').collect();
        let [id, _, device, root, mount_point, ..] = fields.as_slice() else {
            return None;
        };
        let file_system = file_system.split(' ').next().unwrap_or_default();
        if device.is_empty() || root.is_empty() || mount_point.is_empty() || file_system.is_empty()
        {
            return None;
        }
        Some(Row {
            id: id.parse().ok()?,
            device: (*device).to_owned(),
            root: unescape(root)?,
            mount_point: unescape(mount_point)?,
            file_system: file_system.to_owned(),
        })
    }

    /// The table of this process's mount namespace.
    ///
    /// # Errors
    ///
    /// [`Cause::MountTableUnreadable`] or [`Cause::MountTableUnparsable`].
    pub fn read() -> Result<Self, Cause> {
        Self::read_from(std::path::Path::new("/proc/self/mountinfo"))
    }

    /// The table in the file at `path`.
    ///
    /// # Errors
    ///
    /// As [`Self::read`].
    pub fn read_from(path: &std::path::Path) -> Result<Self, Cause> {
        // Read as bytes: a table that was read and is not text is unparsable, not unreadable.
        let bytes = std::fs::read(path).map_err(|_| Cause::MountTableUnreadable)?;
        let text = String::from_utf8(bytes).map_err(|_| Cause::MountTableUnparsable)?;
        Self::parse(&text)
    }

    /// The place of a descriptor the kernel reported `mount_id` and `descriptor_path` for.
    ///
    /// # Errors
    ///
    /// The first cause of D26 that holds, from `mount-id-unreported` on.
    pub fn place(
        &self,
        mount_id: Option<u64>,
        descriptor_path: Option<&str>,
    ) -> Result<Place, Cause> {
        let mount_id = mount_id.ok_or(Cause::MountIdUnreported)?;
        // The row is chosen by the ID alone. No mount point is matched to find it.
        let mut rows = self.rows.iter().filter(|row| row.id == mount_id);
        let row = rows.next().ok_or(Cause::MountRowMissing)?;
        if rows.next().is_some() {
            return Err(Cause::MountRowRepeated);
        }
        if row.root.ends_with("//deleted") {
            return Err(Cause::MountRootDeleted);
        }
        let below = descriptor_path
            .and_then(names)
            .zip(names(&row.mount_point))
            .and_then(|(path, mount_point)| {
                path.starts_with(&mount_point)
                    .then(|| path[mount_point.len()..].to_vec())
            })
            .ok_or(Cause::MountPointMismatch)?;
        if !LISTED_FILE_SYSTEMS.contains(&row.file_system.as_str()) {
            return Err(Cause::FileSystemUnlisted(row.file_system.clone()));
        }
        let mut place: Vec<String> = row
            .root
            .split('/')
            .filter(|name| !name.is_empty())
            .map(str::to_owned)
            .collect();
        place.extend(below);
        Ok(Place {
            device: row.device.clone(),
            names: place,
        })
    }

    /// The place of an opened path, with the names below what was opened joined on.
    ///
    /// # Errors
    ///
    /// [`Unestablished`], naming the path as it was asked.
    pub fn place_of(&self, opened: &Opened) -> Result<Place, Unestablished> {
        let mut place = self
            .place(opened.mount_id, opened.descriptor_path.as_deref())
            .map_err(|cause| Unestablished {
                path: opened.path.clone(),
                cause,
            })?;
        place.names.extend(opened.remaining.iter().cloned());
        Ok(place)
    }

    /// Whether `subject` is `other` or lies in it, by place.
    ///
    /// # Errors
    ///
    /// [`Unestablished`] when either place cannot be established. A caller cannot read a failure
    /// as "outside".
    pub fn lies_in(&self, subject: &Opened, other: &Opened) -> Result<bool, Unestablished> {
        Ok(self.place_of(subject)?.lies_in(&self.place_of(other)?))
    }
}

/// **The place function** (D26): the place of a descriptor, from the mount ID the kernel
/// reported for it, its path, and the text of the mount table.
///
/// # Errors
///
/// The first cause that holds, in D26's order, of those that need no system call:
/// `mount-table-unparsable`, `mount-id-unreported`, `mount-row-missing`, `mount-row-repeated`,
/// `mount-root-deleted`, `mount-point-mismatch`, `file-system-unlisted`.
pub fn place(
    mount_id: Option<u64>,
    descriptor_path: Option<&str>,
    table: &str,
) -> Result<Place, Cause> {
    MountTable::parse(table)?.place(mount_id, descriptor_path)
}

/// `struct statx` as far as the mount ID: the mask, and `stx_mnt_id` at offset 144 of 256 bytes.
#[cfg(target_os = "linux")]
#[repr(C)]
struct Statx {
    mask: u32,
    _before: [u8; 140],
    mount_id: u64,
    _after: [u8; 104],
}

/// The mount ID of a `statx` result: reported only where the call succeeded and its mask says
/// the kernel filled the field in. A kernel before Linux 5.8 returns success with the bit clear
/// and whatever the buffer held, which is no mount ID.
#[cfg(target_os = "linux")]
fn reported_mount_id(succeeded: bool, mask: u32, mount_id: u64) -> Option<u64> {
    (succeeded && mask & STATX_MNT_ID != 0).then_some(mount_id)
}

/// The names of `path` below its nearest existing ancestor, from those collected walking up:
/// each is a name. `None` where one is `.` or `..`, which only the kernel resolves and which
/// does not exist here to be resolved; an empty name, between two separators, is no name.
#[cfg(any(target_os = "linux", test))]
fn remaining_names(mut collected: Vec<String>) -> Option<Vec<String>> {
    if collected
        .iter()
        .any(|name| matches!(name.as_str(), "." | ".."))
    {
        return None;
    }
    collected.retain(|name| !name.is_empty());
    collected.reverse();
    Some(collected)
}

#[cfg(target_os = "linux")]
impl Opened {
    /// What the kernel reports for the open `descriptor`, which `path` names.
    pub fn of_descriptor(descriptor: std::os::fd::RawFd, path: &str) -> Self {
        let mut status = Statx {
            mask: 0,
            _before: [0; 140],
            mount_id: 0,
            _after: [0; 104],
        };
        // SAFETY: the empty path with `AT_EMPTY_PATH` names the descriptor itself, and `status`
        // is a live buffer of the 256 bytes `statx` writes.
        let reported = unsafe {
            libc::syscall(
                libc::SYS_statx,
                descriptor,
                c"".as_ptr(),
                libc::AT_EMPTY_PATH,
                STATX_MNT_ID,
                &raw mut status,
            )
        };
        Self {
            path: path.to_owned(),
            mount_id: reported_mount_id(reported == 0, status.mask, status.mount_id),
            descriptor_path: std::fs::read_link(format!("/proc/self/fd/{descriptor}"))
                .ok()
                .and_then(|link| link.to_str().map(str::to_owned)),
            remaining: Vec::new(),
        }
    }

    /// Opens `path`, or its nearest ancestor that exists, and reads what the kernel reports for
    /// it. Symbolic links are followed: a caller that must not follow one opens the path itself
    /// and uses [`Self::of_descriptor`].
    ///
    /// # Errors
    ///
    /// [`Cause::PathUnopenable`] when the path is not absolute, cannot be opened for a reason
    /// other than not existing, has no ancestor that exists, or has `.` or `..` among the names
    /// that do not exist yet: the text of such a path is not where it will be.
    pub fn of_path(path: &str) -> Result<Self, Unestablished> {
        use std::os::fd::{AsRawFd as _, FromRawFd as _, OwnedFd};
        let unopenable = || Unestablished {
            path: path.to_owned(),
            cause: Cause::PathUnopenable,
        };
        if !path.starts_with('/') {
            return Err(unopenable());
        }
        let mut existing = path.trim_end_matches('/');
        let mut remaining = Vec::new();
        loop {
            let name = if existing.is_empty() { "/" } else { existing };
            let name = std::ffi::CString::new(name).map_err(|_| unopenable())?;
            // SAFETY: `name` is a NUL-terminated string that outlives the call.
            let descriptor = unsafe { libc::open(name.as_ptr(), libc::O_PATH | libc::O_CLOEXEC) };
            if descriptor >= 0 {
                // SAFETY: `open` returned a descriptor nothing else owns.
                let descriptor = unsafe { OwnedFd::from_raw_fd(descriptor) };
                let mut opened = Self::of_descriptor(descriptor.as_raw_fd(), path);
                opened.remaining = remaining_names(remaining).ok_or_else(unopenable)?;
                return Ok(opened);
            }
            let absent = matches!(
                std::io::Error::last_os_error().raw_os_error(),
                Some(libc::ENOENT | libc::ENOTDIR)
            );
            let Some((parent, last)) = existing.rsplit_once('/') else {
                return Err(unopenable());
            };
            if !absent || existing.is_empty() {
                return Err(unopenable());
            }
            remaining.push(last.to_owned());
            existing = parent;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// D26, `mount-id-unreported`: "`statx` fails, or its result does not have `STATX_MNT_ID`
    /// set". A result without the bit carries no mount ID, whatever the field holds.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_statx_result_without_the_mount_id_bit_reports_none() {
        assert_eq!(reported_mount_id(true, STATX_MNT_ID, 31), Some(31));
        assert_eq!(reported_mount_id(true, STATX_MNT_ID | 0x7ff, 0), Some(0));
        assert_eq!(reported_mount_id(true, 0, 31), None, "the bit is clear");
        assert_eq!(
            reported_mount_id(true, 0x7ff, 31),
            None,
            "the basic fields alone, as a kernel before 5.8 reports them"
        );
        assert_eq!(
            reported_mount_id(false, STATX_MNT_ID, 31),
            None,
            "statx failed"
        );
        // And so no place: the cause is the one D26 names, not a row that happens to have ID 0.
        assert_eq!(
            place(reported_mount_id(true, 0x7ff, 500), Some("/"), VIEW),
            Err(Cause::MountIdUnreported)
        );
    }

    /// Only a name is joined below the nearest existing ancestor. `new/..` beneath a directory
    /// is that directory once `new` exists, and is not the text `new/..` before.
    #[test]
    fn a_dot_or_dot_dot_that_does_not_exist_yet_is_no_name() {
        let names = |collected: &[&str]| {
            remaining_names(collected.iter().map(|name| (*name).to_owned()).collect())
        };
        // Collected walking up, so last name first.
        assert_eq!(
            names(&["b", "a"]),
            Some(vec!["a".to_owned(), "b".to_owned()])
        );
        assert_eq!(names(&[]), Some(Vec::new()));
        assert_eq!(names(&["b", "", "a"]), names(&["b", "a"]), "a//b is a/b");
        assert_eq!(names(&["..", "new"]), None);
        assert_eq!(names(&["b", "..", "new"]), None);
        assert_eq!(names(&[".", "new"]), None);
        assert_eq!(names(&["..."]), Some(vec!["...".to_owned()]), "a name");
    }

    /// The same through a real directory: a root spelled `<existing>/new/..` before `new`
    /// exists is `path-unopenable`, and the directory itself once it does.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_path_through_a_missing_name_and_dot_dot_is_unopenable_until_it_exists() {
        let directory = tempfile::tempdir().expect("scratch");
        let root = directory.path().canonicalize().expect("canonical scratch");
        let root = root.to_str().expect("UTF-8");
        for rest in ["new/..", "new/.", "new/./deeper", "a/../b"] {
            let spelled = format!("{root}/{rest}");
            assert_eq!(
                Opened::of_path(&spelled).map(|opened| opened.remaining),
                Err(Unestablished {
                    path: spelled.clone(),
                    cause: Cause::PathUnopenable,
                }),
                "{spelled}"
            );
        }
        // A missing name is joined as a name, as before.
        let missing = Opened::of_path(&format!("{root}/new/deeper")).expect("an ancestor exists");
        assert_eq!(missing.remaining, ["new", "deeper"]);
        assert_eq!(missing.descriptor_path.as_deref(), Some(root));
        // Once `new` exists the kernel resolves the whole path, and nothing remains.
        std::fs::create_dir(format!("{root}/new")).expect("new");
        let resolved = Opened::of_path(&format!("{root}/new/..")).expect("it exists");
        assert!(resolved.remaining.is_empty());
        assert_eq!(resolved.descriptor_path.as_deref(), Some(root));
    }

    /// D26: `mount-table-unreadable` is a table that "cannot be opened or read". One that was
    /// read and holds a mount point that is not UTF-8 is `mount-table-unparsable`.
    #[test]
    fn a_table_with_a_mount_point_that_is_not_utf8_is_unparsable() {
        let directory = tempfile::tempdir().expect("scratch");
        let table = directory.path().join("mountinfo");
        let mut bytes = b"31 1 259:2 / / rw - ext4 /dev/a rw\n40 31 8:1 / /mnt/caf".to_vec();
        bytes.push(0xe9);
        bytes.extend_from_slice(b" rw - ext4 /dev/b rw\n");
        std::fs::write(&table, &bytes).expect("the table");
        assert_eq!(
            MountTable::read_from(&table).map(drop),
            Err(Cause::MountTableUnparsable)
        );
        // The control: the same rows as text parse, and a file that is not there is unreadable.
        bytes.retain(|byte| *byte != 0xe9);
        std::fs::write(&table, &bytes).expect("the table");
        MountTable::read_from(&table).expect("the same table as text");
        assert_eq!(
            MountTable::read_from(&directory.path().join("absent")).map(drop),
            Err(Cause::MountTableUnreadable)
        );
    }

    /// The view of design 35's probe host: `/bin` is a mount of `/usr/bin`, the workspace is a
    /// directory of the same device, `/tmp` is a file system of its own.
    const VIEW: &str = "\
500 400 0:60 / / rw,nosuid,nodev,relatime - tmpfs tmpfs rw,mode=755
501 500 259:2 /usr /usr ro,nosuid,nodev,relatime - ext4 /dev/nvme0n1p2 rw
502 500 259:2 /usr/bin /bin ro,nosuid,nodev,relatime - ext4 /dev/nvme0n1p2 rw
503 500 259:2 /work/u/workspaces/ws_1 /workspace rw,nosuid,nodev,relatime - ext4 /dev/nvme0n1p2 rw
504 500 0:61 / /tmp rw,nosuid,nodev,relatime - tmpfs tmpfs rw
505 500 259:2 /srv/tools /opt/t ro,nosuid,nodev,relatime - ext4 /dev/nvme0n1p2 rw
506 500 259:2 /srv/tools/bin /opt/x ro,nosuid,nodev,relatime - ext4 /dev/nvme0n1p2 rw
507 500 259:2 /srv/my\\040tools /opt/with\\040space ro,relatime shared:7 master:3 - ext4 /dev/nvme0n1p2 rw
508 500 0:24 / /proc rw,nosuid,nodev,noexec,relatime - proc proc rw
";

    fn at(id: u64, path: &str) -> Place {
        place(Some(id), Some(path), VIEW).unwrap_or_else(|cause| panic!("{path}: {cause}"))
    }

    #[test]
    fn one_directory_at_two_mount_points_is_one_place() {
        // B4: neither text is beneath the other, and they are one directory.
        assert_eq!(at(502, "/bin"), at(501, "/usr/bin"));
        assert_eq!(
            at(502, "/bin").relation(&at(501, "/usr/bin")),
            Relation::Within
        );
        assert!(at(502, "/bin/id").lies_in(&at(501, "/usr/bin")));
        assert!(at(501, "/usr/bin/id").lies_in(&at(502, "/bin")));
        assert!(!at(501, "/usr/lib/x").lies_in(&at(502, "/bin")));
    }

    #[test]
    fn a_part_of_a_directory_at_a_second_mount_point_lies_in_it() {
        // K2: the second path is outside the directory as text, and inside it.
        assert!(at(502, "/bin/id").lies_in(&at(501, "/usr")));
        assert!(at(506, "/opt/x/tool").lies_in(&at(505, "/opt/t")));
        assert_eq!(
            at(505, "/opt/t").relation(&at(506, "/opt/x/tool")),
            Relation::Holds
        );
        assert_eq!(
            at(506, "/opt/x").relation(&at(505, "/opt/t/bin")),
            Relation::Within
        );
        assert_eq!(
            at(505, "/opt/t/lib").relation(&at(506, "/opt/x")),
            Relation::Apart
        );
        // Two devices are apart whatever their names are.
        assert_eq!(at(504, "/tmp").relation(&at(500, "/")), Relation::Apart);
    }

    #[test]
    fn a_path_that_does_not_exist_has_the_place_of_its_ancestor_and_its_names() {
        let table = MountTable::parse(VIEW).expect("the view parses");
        let absent = |id: u64, opened: &str, path: &str, remaining: &[&str]| Opened {
            path: path.to_owned(),
            mount_id: Some(id),
            descriptor_path: Some(opened.to_owned()),
            remaining: remaining.iter().map(|name| (*name).to_owned()).collect(),
        };
        let workspace = absent(503, "/workspace", "/workspace", &[]);
        assert_eq!(
            table.lies_in(
                &absent(
                    503,
                    "/workspace",
                    "/workspace/not/there/yet",
                    &["not", "there", "yet"]
                ),
                &workspace
            ),
            Ok(true)
        );
        assert_eq!(
            table.place_of(&absent(506, "/opt/x", "/opt/x/absent", &["absent"])),
            Ok(at(505, "/opt/t/bin/absent"))
        );
        assert_eq!(
            table.lies_in(
                &absent(503, "/workspace", "/workspace/absent", &["absent"]),
                &absent(501, "/usr", "/usr", &[])
            ),
            Ok(false)
        );
    }

    #[test]
    fn an_escaped_space_in_a_mount_point_and_a_root_is_read() {
        let tool = at(507, "/opt/with space/tool");
        assert!(tool.lies_in(&at(507, "/opt/with space")));
        assert!(!tool.lies_in(&at(505, "/opt/t")));
        assert_eq!(tool.names, ["srv", "my tools", "tool"]);
        // The optional fields before the separator do not move the file-system type.
        let table = MountTable::parse(VIEW).expect("the view parses");
        assert_eq!(table.rows[7].file_system, "ext4");
    }

    #[test]
    fn mount_table_unparsable() {
        for text in [
            // No ` - ` separator.
            "500 400 259:2 / / rw ext4 /dev/a rw\n",
            // Three fields.
            "500 400 259:2\n",
            // An escape that is not three octal digits.
            "500 400 259:2 / /a\\04 rw - ext4 /dev/a rw\n",
            "500 400 259:2 / /a\\0489 rw - ext4 /dev/a rw\n",
            // No type after the separator.
            "500 400 259:2 / / rw - \n",
            // No mount ID.
            "x 400 259:2 / / rw - ext4 /dev/a rw\n",
            // A row that parses beside one that does not.
            "500 400 259:2 / / rw - ext4 /dev/a rw\n501 500 259:2 /usr\n",
        ] {
            assert_eq!(
                place(Some(500), Some("/"), text),
                Err(Cause::MountTableUnparsable),
                "{text:?}"
            );
        }
    }

    #[test]
    fn mount_id_unreported() {
        assert_eq!(
            place(None, Some("/usr/bin"), VIEW),
            Err(Cause::MountIdUnreported)
        );
    }

    #[test]
    fn mount_row_missing() {
        assert_eq!(
            place(Some(999), Some("/usr/bin"), VIEW),
            Err(Cause::MountRowMissing)
        );
    }

    #[test]
    fn mount_row_repeated() {
        let table = "\
500 400 259:2 / / rw - ext4 /dev/a rw
501 500 259:2 /usr /usr ro - ext4 /dev/a rw
501 500 259:2 /usr/bin /bin ro - ext4 /dev/a rw
";
        assert_eq!(
            place(Some(501), Some("/usr/bin"), table),
            Err(Cause::MountRowRepeated)
        );
    }

    #[test]
    fn mount_root_deleted() {
        // S3: a directory bound at a second path and then removed.
        let table = "\
500 400 259:2 / / rw - ext4 /dev/a rw
520 500 259:2 /tree//deleted /at rw - ext4 /dev/a rw
";
        assert_eq!(
            place(Some(520), Some("/at"), table),
            Err(Cause::MountRootDeleted)
        );
        assert_eq!(
            place(Some(520), Some("/at/x"), table),
            Err(Cause::MountRootDeleted)
        );
    }

    #[test]
    fn mount_point_mismatch() {
        let table = "\
500 400 259:2 / / rw - ext4 /dev/a rw
530 500 259:2 /srv/a /opt/a ro - ext4 /dev/a rw
";
        assert_eq!(
            place(Some(530), Some("/opt/b/tool"), table),
            Err(Cause::MountPointMismatch)
        );
        // `/opt/ab` is not beneath `/opt/a`, though its text begins with it.
        assert_eq!(
            place(Some(530), Some("/opt/ab"), table),
            Err(Cause::MountPointMismatch)
        );
        // A descriptor path that could not be read, and one that is no path.
        assert_eq!(
            place(Some(530), None, table),
            Err(Cause::MountPointMismatch)
        );
        assert_eq!(
            place(Some(530), Some("pipe:[4711]"), table),
            Err(Cause::MountPointMismatch)
        );
    }

    #[test]
    fn file_system_unlisted() {
        for file_system in [
            "btrfs",
            "xfs",
            "nfs4",
            "fuse.sshfs",
            "proc",
            "devtmpfs",
            "ext3",
            "EXT4",
        ] {
            let table = format!("540 500 0:70 / /mnt rw - {file_system} source rw\n");
            let refused = place(Some(540), Some("/mnt/tool"), &table);
            assert_eq!(
                refused,
                Err(Cause::FileSystemUnlisted(file_system.to_owned()))
            );
            // The error carries the type.
            assert_eq!(
                refused.expect_err("unlisted").to_string(),
                format!("file-system-unlisted {file_system}")
            );
        }
        for file_system in ["ext4", "overlay", "tmpfs"] {
            let table = format!("540 500 0:70 /tree /mnt rw - {file_system} source rw\n");
            assert_eq!(
                place(Some(540), Some("/mnt/tool"), &table),
                Ok(Place {
                    device: "0:70".to_owned(),
                    names: vec!["tree".to_owned(), "tool".to_owned()],
                })
            );
        }
        assert_eq!(LISTED_FILE_SYSTEMS, ["ext4", "overlay", "tmpfs"]);
    }

    /// The causes are checked in D26's order: the first that holds is the one named.
    #[test]
    fn the_first_cause_that_holds_is_named() {
        // A deleted root on an unlisted type whose mount point does not match either.
        let table = "550 500 0:70 /tree//deleted /at rw - xfs source rw\n";
        assert_eq!(
            place(Some(550), Some("/elsewhere"), table),
            Err(Cause::MountRootDeleted)
        );
        let table = "550 500 0:70 /tree /at rw - xfs source rw\n";
        assert_eq!(
            place(Some(550), Some("/elsewhere"), table),
            Err(Cause::MountPointMismatch)
        );
        assert_eq!(
            place(None, Some("/elsewhere"), table),
            Err(Cause::MountIdUnreported)
        );
        assert_eq!(
            place(None, None, "550 500\n"),
            Err(Cause::MountTableUnparsable)
        );
    }

    /// S1: two mounts stacked on one mount point are two rows with two IDs, and the place is in
    /// the root of the row whose ID the kernel reported.
    #[test]
    fn stacked_mounts_are_told_apart_by_the_reported_id() {
        let table = "\
500 400 259:2 / / rw - ext4 /dev/a rw
560 500 259:2 /first /opt ro - ext4 /dev/a rw
561 560 259:2 /second /opt ro - ext4 /dev/a rw
";
        let lower = place(Some(560), Some("/opt/tool"), table).expect("the lower mount");
        let upper = place(Some(561), Some("/opt/tool"), table).expect("the upper mount");
        assert_eq!(lower.names, ["first", "tool"]);
        assert_eq!(upper.names, ["second", "tool"]);
        assert_eq!(lower.relation(&upper), Relation::Apart);
    }

    /// S2: a mount at `hold/sub` that a later mount over `hold` hides keeps its row, and that
    /// row has the longest mount point matching `hold/sub/z`. The kernel names the row of
    /// `hold`, and the hidden row is never chosen.
    #[test]
    fn a_hidden_mount_is_never_chosen() {
        let table = "\
500 400 259:2 / / rw - ext4 /dev/a rw
570 500 259:2 /other /hold/sub ro - ext4 /dev/a rw
571 500 259:2 /second /hold ro - ext4 /dev/a rw
";
        let file =
            place(Some(571), Some("/hold/sub/z"), table).expect("a file of the second mount");
        assert_eq!(file.names, ["second", "sub", "z"]);
        let second = place(Some(571), Some("/hold"), table).expect("the second mount");
        assert!(file.lies_in(&second));
        let hidden = place(Some(570), Some("/hold/sub"), table).expect("the hidden mount's row");
        assert!(!file.lies_in(&hidden));
    }

    /// No fallback: for each cause the comparison returns the cause and never a boolean, so a
    /// caller cannot read a failure as "outside".
    #[test]
    fn lies_in_returns_the_cause_and_never_a_boolean() {
        let table = MountTable::parse(
            "\
500 400 259:2 / / rw - ext4 /dev/a rw
501 500 259:2 /usr /usr ro - ext4 /dev/a rw
501 500 259:2 /usr /usr ro - ext4 /dev/a rw
520 500 259:2 /tree//deleted /at rw - ext4 /dev/a rw
540 500 0:70 / /mnt rw - xfs source rw
",
        )
        .expect("parses");
        let opened = |path: &str, mount_id: Option<u64>, descriptor_path: Option<&str>| Opened {
            path: path.to_owned(),
            mount_id,
            descriptor_path: descriptor_path.map(str::to_owned),
            remaining: Vec::new(),
        };
        let root = opened("/", Some(500), Some("/"));
        for (subject, cause) in [
            (opened("/a", None, Some("/a")), Cause::MountIdUnreported),
            (opened("/a", Some(999), Some("/a")), Cause::MountRowMissing),
            (
                opened("/usr/a", Some(501), Some("/usr/a")),
                Cause::MountRowRepeated,
            ),
            (
                opened("/at/a", Some(520), Some("/at/a")),
                Cause::MountRootDeleted,
            ),
            (opened("/a", Some(500), None), Cause::MountPointMismatch),
            (opened("/b", Some(520), Some("/b")), Cause::MountRootDeleted),
            (
                opened("/b", Some(540), Some("/b")),
                Cause::MountPointMismatch,
            ),
            (
                opened("/mnt/a", Some(540), Some("/mnt/a")),
                Cause::FileSystemUnlisted("xfs".to_owned()),
            ),
        ] {
            let refused = Unestablished {
                path: subject.path.clone(),
                cause,
            };
            // As the subject and as the other path: neither side answers for the other.
            assert_eq!(table.lies_in(&subject, &root), Err(refused.clone()));
            assert_eq!(table.lies_in(&root, &subject), Err(refused));
        }
        assert_eq!(
            Unestablished {
                path: "/proc/version".to_owned(),
                cause: Cause::FileSystemUnlisted("proc".to_owned()),
            }
            .to_string(),
            "cannot establish the place of /proc/version: file-system-unlisted proc"
        );
    }

    /// The two causes outside the place function: the reader given a table that does not exist,
    /// and a subject beneath a directory this process cannot search.
    #[cfg(target_os = "linux")]
    #[test]
    fn mount_table_unreadable_and_path_unopenable() {
        use std::os::unix::fs::PermissionsExt as _;
        let directory = tempfile::tempdir().expect("a directory");
        assert_eq!(
            MountTable::read_from(&directory.path().join("no-such-table")),
            Err(Cause::MountTableUnreadable)
        );
        let closed = directory.path().join("closed");
        std::fs::create_dir(&closed).expect("a directory");
        std::fs::write(closed.join("file"), b"").expect("a file");
        std::fs::set_permissions(&closed, std::fs::Permissions::from_mode(0o000)).expect("000");
        let subject = closed.join("file");
        let subject = subject.to_str().expect("UTF-8");
        let searched = std::fs::metadata(subject).is_ok();
        let opened = Opened::of_path(subject);
        std::fs::set_permissions(&closed, std::fs::Permissions::from_mode(0o700)).expect("700");
        if searched {
            println!("place path-unopenable: ABSENT (this uid searches a directory of mode 000)");
        } else {
            assert_eq!(
                opened,
                Err(Unestablished {
                    path: subject.to_owned(),
                    cause: Cause::PathUnopenable,
                })
            );
        }
        assert_eq!(
            Opened::of_path("relative/path").map_err(|why| why.cause),
            Err(Cause::PathUnopenable)
        );
    }

    /// The reader on this host: an opened path has the mount ID of one row, and a path that
    /// does not exist has its ancestor's place and its own names.
    #[cfg(target_os = "linux")]
    #[test]
    fn an_opened_path_is_placed_by_the_mount_id_the_kernel_reports() {
        let directory = tempfile::tempdir().expect("a directory");
        let path = directory.path().to_str().expect("UTF-8");
        let table = MountTable::read().expect("this process's mount table");
        let here = Opened::of_path(path).expect("opened");
        assert!(here.mount_id.is_some(), "statx reported no mount ID");
        assert!(here.remaining.is_empty());
        let absent = Opened::of_path(&format!("{path}/not/there")).expect("an ancestor exists");
        assert_eq!(absent.remaining, ["not", "there"]);
        assert_eq!(absent.mount_id, here.mount_id);
        match (table.place_of(&here), table.place_of(&absent)) {
            (Ok(here), Ok(absent)) => {
                assert!(absent.lies_in(&here));
                assert!(!here.lies_in(&absent));
                println!("place of an opened path: executed");
            }
            (Err(why), _) | (_, Err(why)) => {
                // A temporary directory on a file system outside the list: the cause is named.
                assert_eq!(why.cause.name(), "file-system-unlisted", "{why}");
                println!("place of an opened path: ABSENT ({why})");
            }
        }
    }
}
