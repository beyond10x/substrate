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
//! | `mount-covered` | a mount taken as mounted beneath a directory is not the mount the kernel reports at its own mount point, or that path no longer exists: a later mount hides it, or it lies under a stack (D30) |
//!
//! **"Lies in" follows the mounts** (D30). A mount beneath a directory shows files whose place
//! is on another device, or on the directory's device under other names. By place they are
//! outside the directory; in the view they are in it, and the kernel treats them so. Three
//! terms, each from the table and from places, none from the text of an asked path:
//!
//! - **the attachment point of a mount** is a place: its parent row's device, and the parent
//!   row's root joined with the row's mount point taken relative to the parent row's. The root
//!   of the view has none: it is the one row at `/` whose parent ID has no row, or is its own;
//! - **a mount is mounted beneath a directory** when its attachment point lies in the
//!   directory's place, or its parent mount is mounted beneath the directory;
//! - **the places a directory reaches** are its own place and the root of every mount that is
//!   mounted beneath it ([`MountTree::reaches`], [`Reach`]).
//!
//! So a path lies in a directory, in the view, when its place lies in a place the directory
//! reaches. It fails closed like a place: `mount-row-missing`, `mount-row-repeated` and
//! `mount-point-mismatch` are also raised for a row whose parent cannot be established, asked
//! of the whole table once ([`MountTable::into_tree`]). The other causes depend on which side
//! of a comparison the directory is:
//!
//! - **a location a run can write** ([`MountTree::reaches`]) fails closed for every mount
//!   whose root is compared: `mount-row-repeated` where two rows have its ID,
//!   `mount-root-deleted`, `mount-covered`, and `path-unopenable` or `mount-id-unreported`
//!   where the mount point cannot be asked. A place that is missing from this side would be
//!   a writable place called read-only;
//! - **a directory nothing is written through** ([`MountTree::reaches_as_written`]: the host
//!   source of a read-only mount) never refuses. A mount beneath it is counted as reached by
//!   the root its own row names, whether or not the kernel shows it there: that can only make
//!   the directory overlap more, never less, and a host is not refused for a mount nothing
//!   writable touches.
//!
//! A mount of a file-system type outside the list is not compared as a place, and is refused
//! only where the caller says so ([`Unlisted`]). Its row is kept all the same: two such rows
//! with one device, the root of one in the root of the other, show the same files, so two
//! directories that each reach one of them overlap ([`Reach::overlaps`]). No path is ever
//! placed on such a mount: **a symbolic link read there, like a file named there, has no
//! place and refuses** where it is asked.
//!
//! [`place`] is the one function that establishes a place, from the reported mount ID, the
//! descriptor's path and the table's text. The helper and the driver compile this file, so the
//! two cannot disagree. The helper reads the table of the run's view; the driver reads its own,
//! for host paths.

use std::collections::{HashMap, HashSet};
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
    /// The tenth cause (D30): the kernel reports another mount at the row's mount point.
    MountCovered,
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
            Self::MountCovered => "mount-covered",
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

/// What was asked of the table and could not be established.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Asked {
    /// The place of a path (D25).
    #[default]
    Place,
    /// Where a mount is attached, for the mount at this mount point (D30).
    Attachment,
}

/// A place that could not be established: the path as it was asked, and the cause. For
/// [`Asked::Attachment`] the path is the mount point of the row the cause was found in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unestablished {
    pub path: String,
    pub cause: Cause,
    pub asked: Asked,
}

impl Unestablished {
    /// The place of `path` could not be established for `cause`.
    pub(crate) fn place(path: impl Into<String>, cause: Cause) -> Self {
        Self {
            path: path.into(),
            cause,
            asked: Asked::Place,
        }
    }
}

impl fmt::Display for Unestablished {
    /// The helper's diagnostic line after its own name.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.asked {
            Asked::Place => write!(
                formatter,
                "cannot establish the place of {}: {}",
                self.path, self.cause
            ),
            Asked::Attachment => write!(
                formatter,
                "cannot establish where {} is attached: {}",
                self.path, self.cause
            ),
        }
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

/// Whether `place` lies in one of the places a directory reaches (D30).
fn lies_in_any(place: &Place, reached: &[Place]) -> bool {
    reached.iter().any(|other| place.lies_in(other))
}

/// Whether two sets of places overlap (D30): a place of one lies in a place of the other.
fn overlap(one: &[Place], other: &[Place]) -> bool {
    one.iter().any(|place| {
        other
            .iter()
            .any(|against| place.relation(against) != Relation::Apart)
    })
}

/// **What a directory reaches** (D30): its own place, and the root of every mount that is
/// mounted beneath it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reach {
    /// The directory's own place first, then the roots of the mounts of a listed type.
    places: Vec<Place>,
    /// The roots of the mounts beneath it of a type outside the list, as their rows name
    /// them: device and root. Never a place a path is compared with, only row against row.
    unlisted: Vec<Place>,
}

impl Reach {
    /// The places: the directory's own first.
    pub fn places(&self) -> &[Place] {
        &self.places
    }

    /// Whether `place` lies in the directory, in the view: in one of the places it reaches.
    pub fn holds(&self, place: &Place) -> bool {
        lies_in_any(place, &self.places)
    }

    /// Whether two directories overlap, in the view: a place one reaches lies in a place the
    /// other reaches, or each reaches a mount of a type outside the list and the two rows
    /// name one device with the root of one in the root of the other.
    pub fn overlaps(&self, other: &Self) -> bool {
        overlap(&self.places, &other.places) || overlap(&self.unlisted, &other.unlisted)
    }
}

/// One row of the table.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Row {
    id: u64,
    parent: u64,
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
        let [id, parent, device, root, mount_point, ..] = fields.as_slice() else {
            return None;
        };
        let file_system = file_system.split(' ').next().unwrap_or_default();
        if device.is_empty() || root.is_empty() || mount_point.is_empty() || file_system.is_empty()
        {
            return None;
        }
        Some(Row {
            id: id.parse().ok()?,
            parent: parent.parse().ok()?,
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
        if row.root_deleted() {
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
        let mut place = row.root_place();
        place.names.extend(below);
        Ok(place)
    }

    /// The place of an opened path, with the names below what was opened joined on.
    ///
    /// # Errors
    ///
    /// [`Unestablished`], naming the path as it was asked.
    pub fn place_of(&self, opened: &Opened) -> Result<Place, Unestablished> {
        let mut place = self
            .place(opened.mount_id, opened.descriptor_path.as_deref())
            .map_err(|cause| Unestablished::place(opened.path.clone(), cause))?;
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

impl Row {
    /// The place of the directory or file this mount shows: its device and its root. For a
    /// removed directory, the names it had: only a caller that never refuses counts that.
    fn root_place(&self) -> Place {
        Place {
            device: self.device.clone(),
            names: self
                .root
                .strip_suffix("//deleted")
                .unwrap_or(&self.root)
                .split('/')
                .filter(|name| !name.is_empty())
                .map(str::to_owned)
                .collect(),
        }
    }

    /// Whether the directory this mount shows was removed (S3): its root is no place.
    fn root_deleted(&self) -> bool {
        self.root.ends_with("//deleted")
    }
}

/// How a mount of a file-system type outside [`LISTED_FILE_SYSTEMS`] that is mounted beneath a
/// directory is taken (D30). Its root is no place the rule can trust, so it is never compared.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unlisted {
    /// It enters no set of places and refuses nothing. Beneath a location a run can write
    /// that is safe because whatever is compared with a writable place is placed first, and
    /// a path on such a type has no place. Bubblewrap binds each device node of `/dev` as a
    /// `devtmpfs` mount beneath the writable `/dev`, so a view always has this case.
    Skipped,
    /// It refuses, `file-system-unlisted`: beneath a denied directory, whose files the
    /// profile denies wherever else that file system is mounted.
    Refused,
}

/// What the kernel reports at a mount point: the ID of the mount that shows there, or why
/// none can be had.
type Reported = Box<dyn Fn(&str) -> Result<u64, Cause> + Send + Sync>;

/// A mount table as a tree (D30): every row linked to the row of the mount it is attached in.
pub struct MountTree {
    table: MountTable,
    /// For each row the index of its parent's row; `None` for the root of the view.
    parents: Vec<Option<usize>>,
    /// For each row whether another row has its mount ID.
    repeated: Vec<bool>,
    reported: Reported,
}

impl fmt::Debug for MountTree {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MountTree")
            .field("table", &self.table)
            .field("parents", &self.parents)
            .finish_non_exhaustive()
    }
}

impl MountTable {
    /// Links every row to its parent's, with `reported` as what the kernel reports at a mount
    /// point: the mount ID of the path opened, or the cause for which there is none.
    ///
    /// # Errors
    ///
    /// [`Unestablished`] for the first row whose attachment cannot be established, asked of
    /// the whole table, because such a row may be mounted beneath anything:
    /// `mount-row-missing` where a row names a parent ID no row has, or its own, and is not
    /// the one root of the view, at `/`, or where a row's parents never reach that root;
    /// `mount-row-repeated` where more than one row has the parent ID; and
    /// `mount-point-mismatch` where a row's mount point is not its parent row's or beneath it.
    fn into_tree_reported_by(
        self,
        reported: impl Fn(&str) -> Result<u64, Cause> + Send + Sync + 'static,
    ) -> Result<MountTree, Unestablished> {
        let unattached = |row: &Row, cause: Cause| Unestablished {
            path: row.mount_point.clone(),
            cause,
            asked: Asked::Attachment,
        };
        let mut parents = Vec::with_capacity(self.rows.len());
        let mut root = false;
        for row in &self.rows {
            // A row that names itself has no row above it: the root of a mount namespace
            // is its own parent.
            let mut above = self
                .rows
                .iter()
                .enumerate()
                .filter(|(_, other)| other.id == row.parent && row.parent != row.id);
            match (above.next(), above.next()) {
                (None, _) => {
                    if row.mount_point != "/" || root {
                        return Err(unattached(row, Cause::MountRowMissing));
                    }
                    root = true;
                    parents.push(None);
                }
                (Some(_), Some(_)) => return Err(unattached(row, Cause::MountRowRepeated)),
                (Some((index, parent)), None) => {
                    let beneath = names(&row.mount_point)
                        .zip(names(&parent.mount_point))
                        .is_some_and(|(own, over)| own.starts_with(&over));
                    if !beneath {
                        return Err(unattached(row, Cause::MountPointMismatch));
                    }
                    parents.push(Some(index));
                }
            }
        }
        // Every row leads to the root. Rows that name each other in a circle have none.
        for (start, row) in self.rows.iter().enumerate() {
            let mut at = start;
            let mut steps = 0;
            while let Some(parent) = parents[at] {
                at = parent;
                steps += 1;
                if steps > self.rows.len() {
                    return Err(unattached(row, Cause::MountRowMissing));
                }
            }
        }
        let mut seen: HashMap<u64, usize> = HashMap::new();
        for row in &self.rows {
            *seen.entry(row.id).or_default() += 1;
        }
        let repeated = self.rows.iter().map(|row| seen[&row.id] > 1).collect();
        Ok(MountTree {
            table: self,
            parents,
            repeated,
            reported: Box::new(reported),
        })
    }

    /// The tree of a table given as text, in which no mount is hidden: at each mount point
    /// the kernel is taken to report the top of the stack there, the row no other row at that
    /// mount point names as its parent, and of several such rows the last. The order of the
    /// rows of a stack decides nothing.
    ///
    /// # Errors
    ///
    /// [`Unestablished`] for the first row whose attachment cannot be established, asked of
    /// the whole table, because such a row may be mounted beneath anything:
    /// `mount-row-missing` where a row names a parent ID no row has, or its own, and is not
    /// the one root of the view, at `/`, or where a row's parents never reach that root;
    /// `mount-row-repeated` where more than one row has the parent ID; and
    /// `mount-point-mismatch` where a row's mount point is not its parent row's or beneath it.
    pub fn into_tree_as_written(self) -> Result<MountTree, Unestablished> {
        let at: HashMap<u64, &str> = self
            .rows
            .iter()
            .map(|row| (row.id, row.mount_point.as_str()))
            .collect();
        // The rows another row is stacked on: its parent, at its own mount point.
        let beneath: HashSet<u64> = self
            .rows
            .iter()
            .filter(|row| {
                row.parent != row.id
                    && at
                        .get(&row.parent)
                        .is_some_and(|above| *above == row.mount_point)
            })
            .map(|row| row.parent)
            .collect();
        let shown: HashMap<String, u64> = self
            .rows
            .iter()
            .filter(|row| !beneath.contains(&row.id))
            .map(|row| (row.mount_point.clone(), row.id))
            .collect();
        self.into_tree_reported_by(move |mount_point| {
            shown.get(mount_point).copied().ok_or(Cause::MountCovered)
        })
    }

    /// The tree of this process's mount namespace, with what the kernel reports there at each
    /// mount point: the table must be this namespace's.
    ///
    /// # Errors
    ///
    /// As [`Self::into_tree_as_written`].
    #[cfg(target_os = "linux")]
    pub fn into_tree(self) -> Result<MountTree, Unestablished> {
        self.into_tree_reported_by(reported_at)
    }
}

impl MountTree {
    /// The table the tree was linked from.
    pub const fn table(&self) -> &MountTable {
        &self.table
    }

    /// Where the mount of row `index` is attached in its parent mount `parent`: the parent
    /// row's device, and its root joined with the row's mount point taken relative to the
    /// parent row's. `None` where the parent's root was deleted: no place lies in that.
    fn attachment(&self, index: usize, parent: usize) -> Option<Place> {
        let (row, above) = (&self.table.rows[index], &self.table.rows[parent]);
        if above.root_deleted() {
            return None;
        }
        let own = names(&row.mount_point)?;
        let over = names(&above.mount_point)?;
        let mut place = above.root_place();
        place.names.extend(own.get(over.len()..)?.iter().cloned());
        Some(place)
    }

    /// For each row whether its mount is mounted beneath `directory`: its attachment point
    /// lies in the directory's place, or its parent mount is mounted beneath the directory.
    /// A loop over the rows and no recursion: a namespace holds up to `fs.mount-max` mounts,
    /// 100 000 by default, each of which may be mounted in the one before it.
    fn beneath(&self, directory: &Place) -> Vec<bool> {
        let mut known: Vec<Option<bool>> = vec![None; self.table.rows.len()];
        let mut chain = Vec::new();
        for start in 0..known.len() {
            // Up to the first row whose answer is known, or to the root, which is beneath
            // nothing; then down again, each row after its parent.
            let mut at = start;
            let mut above = loop {
                if let Some(answer) = known[at] {
                    break answer;
                }
                match self.parents[at] {
                    None => {
                        known[at] = Some(false);
                        break false;
                    }
                    Some(parent) => {
                        chain.push((at, parent));
                        at = parent;
                    }
                }
            };
            while let Some((index, parent)) = chain.pop() {
                above = above
                    || self
                        .attachment(index, parent)
                        .is_some_and(|place| place.lies_in(directory));
                known[index] = Some(above);
            }
        }
        known
            .into_iter()
            .map(|answer| answer.unwrap_or(false))
            .collect()
    }

    /// **What `directory` reaches, a location a run can write** (D30): its own place, and the
    /// root of every mount that is mounted beneath it. A path lies in the directory, in the
    /// view, when its place lies in one of them ([`Reach::holds`]). It fails closed: a mount
    /// beneath the directory whose root cannot be taken as a place refuses.
    ///
    /// # Errors
    ///
    /// [`Unestablished`], naming the mount point of such a mount: `file-system-unlisted`
    /// where `unlisted` refuses such a type; and for a mount of a listed type
    /// `mount-row-repeated` where another row has its mount ID, so which of them the kernel
    /// shows cannot be said, `mount-root-deleted` where its root was removed, `mount-covered`
    /// where the kernel reports another mount at that mount point or the path is gone, so the
    /// row's root is not what the path shows, and `path-unopenable` or `mount-id-unreported`
    /// where the mount point cannot be asked.
    pub fn reaches(&self, directory: &Place, unlisted: Unlisted) -> Result<Reach, Unestablished> {
        let beneath = self.beneath(directory);
        let mut reach = Reach {
            places: vec![directory.clone()],
            unlisted: Vec::new(),
        };
        for (index, row) in self.table.rows.iter().enumerate() {
            if !beneath[index] {
                continue;
            }
            let unattached = |cause: Cause| Unestablished {
                path: row.mount_point.clone(),
                cause,
                asked: Asked::Attachment,
            };
            if !LISTED_FILE_SYSTEMS.contains(&row.file_system.as_str()) {
                match unlisted {
                    Unlisted::Skipped => {
                        reach.unlisted.push(row.root_place());
                        continue;
                    }
                    Unlisted::Refused => {
                        return Err(unattached(Cause::FileSystemUnlisted(
                            row.file_system.clone(),
                        )));
                    }
                }
            }
            if self.repeated[index] {
                return Err(unattached(Cause::MountRowRepeated));
            }
            if row.root_deleted() {
                return Err(unattached(Cause::MountRootDeleted));
            }
            match (self.reported)(&row.mount_point) {
                Ok(shows) if shows == row.id => {}
                Ok(_) => return Err(unattached(Cause::MountCovered)),
                Err(cause) => return Err(unattached(cause)),
            }
            reach.places.push(row.root_place());
        }
        Ok(reach)
    }

    /// **What `directory` reaches, by the table alone**: for a directory nothing is written
    /// through, the host source of a read-only mount. Every mount the table puts beneath it
    /// is counted by the root its own row names, and the kernel is not asked: a mount that is
    /// covered, whose mount point cannot be opened, whose root was removed or whose ID two
    /// rows have is counted like any other. So this never refuses, and it reaches at least
    /// what [`Self::reaches`] would: a directory can only overlap more for it, never less.
    pub fn reaches_as_written(&self, directory: &Place) -> Reach {
        let beneath = self.beneath(directory);
        let mut reach = Reach {
            places: vec![directory.clone()],
            unlisted: Vec::new(),
        };
        for (row, _) in self
            .table
            .rows
            .iter()
            .zip(beneath)
            .filter(|(_, beneath)| *beneath)
        {
            if LISTED_FILE_SYSTEMS.contains(&row.file_system.as_str()) {
                reach.places.push(row.root_place());
            } else {
                reach.unlisted.push(row.root_place());
            }
        }
        reach
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

/// The mount ID the kernel reports for the open `descriptor`, where it reports one.
#[cfg(target_os = "linux")]
fn mount_id_of(descriptor: std::os::fd::RawFd) -> Option<u64> {
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
    reported_mount_id(reported == 0, status.mask, status.mount_id)
}

/// The mount ID the kernel reports for `path`, opened without following a last symbolic link:
/// the mount that shows at that path, which is the upper one of a stack and another one than
/// a hidden mount's.
///
/// # Errors
///
/// `mount-covered` where the path does not exist: a mount point of the table that is gone
/// lies beneath a later mount. `path-unopenable` where it cannot be opened for another
/// reason, and `mount-id-unreported` where the kernel reports no ID for it.
#[cfg(target_os = "linux")]
fn reported_at(path: &str) -> Result<u64, Cause> {
    use std::os::fd::{AsRawFd as _, FromRawFd as _, OwnedFd};
    let name = std::ffi::CString::new(path).map_err(|_| Cause::PathUnopenable)?;
    // SAFETY: `name` is a NUL-terminated string that outlives the call.
    let descriptor = unsafe {
        libc::open(
            name.as_ptr(),
            libc::O_PATH | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    };
    if descriptor < 0 {
        let gone = matches!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ENOENT | libc::ENOTDIR)
        );
        return Err(if gone {
            Cause::MountCovered
        } else {
            Cause::PathUnopenable
        });
    }
    // SAFETY: `open` returned a descriptor nothing else owns.
    let descriptor = unsafe { OwnedFd::from_raw_fd(descriptor) };
    mount_id_of(descriptor.as_raw_fd()).ok_or(Cause::MountIdUnreported)
}

#[cfg(target_os = "linux")]
impl Opened {
    /// What the kernel reports for the open `descriptor`, which `path` names.
    pub fn of_descriptor(descriptor: std::os::fd::RawFd, path: &str) -> Self {
        Self {
            path: path.to_owned(),
            mount_id: mount_id_of(descriptor),
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
        let unopenable = || Unestablished::place(path, Cause::PathUnopenable);
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
                    asked: Asked::Place,
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
                asked: Asked::Place,
            };
            // As the subject and as the other path: neither side answers for the other.
            assert_eq!(table.lies_in(&subject, &root), Err(refused.clone()));
            assert_eq!(table.lies_in(&root, &subject), Err(refused));
        }
        assert_eq!(
            Unestablished {
                path: "/proc/version".to_owned(),
                cause: Cause::FileSystemUnlisted("proc".to_owned()),
                asked: Asked::Place,
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
                    asked: Asked::Place,
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

    /// D30's table: `/srv` is a directory of the root file system, `/opt` another, and `/tmp`
    /// a tmpfs beside both. Each case adds the rows it is about.
    const BESIDE: &str = "\
500 400 259:2 / / rw - ext4 /dev/a rw
501 500 0:40 / /tmp rw - tmpfs tmpfs rw
";

    /// `BESIDE` with `rows`, as a tree in which no mount is hidden.
    fn tree(rows: &str) -> MountTree {
        MountTable::parse(&format!("{BESIDE}{rows}"))
            .expect("the table parses")
            .into_tree_as_written()
            .expect("every row is attached")
    }

    /// The place of `path`, which the kernel reports on the mount `id`.
    fn on(tree: &MountTree, id: u64, path: &str) -> Place {
        tree.table()
            .place(Some(id), Some(path))
            .expect("the place is established")
    }

    /// The devices of the places `path`, on the mount `id`, reaches.
    fn devices(tree: &MountTree, id: u64, path: &str) -> Vec<String> {
        tree.reaches(&on(tree, id, path), Unlisted::Skipped)
            .expect("what the directory reaches")
            .places()
            .iter()
            .map(|place| format!("{} /{}", place.device, place.names.join("/")))
            .collect()
    }

    /// The refusal of asking what `path`, on the mount `id`, reaches.
    fn unreached(tree: &MountTree, id: u64, path: &str, unlisted: Unlisted) -> Unestablished {
        tree.reaches(&on(tree, id, path), unlisted)
            .expect_err("a mount beneath the directory has no root to compare")
    }

    /// The whole-table refusal of `rows` beside `BESIDE`.
    fn unattached(table: &str) -> Unestablished {
        MountTable::parse(table)
            .expect("the table parses")
            .into_tree_as_written()
            .expect_err("a row whose attachment cannot be established")
    }

    /// D30, a chain of three: a mount beneath a directory, one mounted in that, and one in
    /// that. The directory reaches all three, and a file on the third lies in it although its
    /// place is on a third device. A directory on the first mount reaches the two below it,
    /// and not the mount it is on.
    #[test]
    fn a_chain_of_three_mounts_beneath_a_directory_is_reached() {
        let tree = tree(
            "\
510 500 0:50 / /srv/one rw - tmpfs tmpfs rw
511 510 0:51 / /srv/one/two rw - tmpfs tmpfs rw
512 511 0:52 / /srv/one/two/three rw - tmpfs tmpfs rw
",
        );
        assert_eq!(
            devices(&tree, 500, "/srv"),
            ["259:2 /srv", "0:50 /", "0:51 /", "0:52 /"]
        );
        let reached = tree
            .reaches(&on(&tree, 500, "/srv"), Unlisted::Skipped)
            .expect("reached")
            .places()
            .to_vec();
        let deep = on(&tree, 512, "/srv/one/two/three/file");
        assert!(!deep.lies_in(&on(&tree, 500, "/srv")), "by place alone");
        assert!(lies_in_any(&deep, &reached));
        assert!(lies_in_any(&on(&tree, 500, "/srv/file"), &reached));
        // Beside the directory: the root file system above it, another directory, a tmpfs.
        for (id, path) in [(500, "/"), (500, "/opt/file"), (501, "/tmp/file")] {
            assert!(!lies_in_any(&on(&tree, id, path), &reached), "{path}");
        }
        assert_eq!(
            devices(&tree, 510, "/srv/one"),
            ["0:50 /", "0:51 /", "0:52 /"]
        );
        assert_eq!(devices(&tree, 512, "/srv/one/two/three"), ["0:52 /"]);
        assert_eq!(devices(&tree, 500, "/opt"), ["259:2 /opt"]);
        // A second mount of the third file system elsewhere shows the same places: a path
        // through it lies in what `/srv` reaches, and one through a part of it as well.
        let again = self::tree(
            "\
510 500 0:50 / /srv/one rw - tmpfs tmpfs rw
513 500 0:50 / /mnt/again rw - tmpfs tmpfs rw
514 500 0:50 /part /mnt/part rw - tmpfs tmpfs rw
",
        );
        let reached = again
            .reaches(&on(&again, 500, "/srv"), Unlisted::Skipped)
            .expect("reached")
            .places()
            .to_vec();
        for (id, path) in [(513, "/mnt/again/file"), (514, "/mnt/part/file")] {
            assert!(lies_in_any(&on(&again, id, path), &reached), "{path}");
        }
        assert!(!lies_in_any(&on(&again, 500, "/mnt/file"), &reached));
        // The root of the view reaches every mount.
        assert_eq!(
            devices(&tree, 500, "/"),
            ["259:2 /", "0:40 /", "0:50 /", "0:51 /", "0:52 /"]
        );
    }

    /// D30, a bind of a subdirectory. A directory bound beneath `/srv` is reached with its
    /// own names, so the file it shows lies in `/srv` under its other name too. And a mount
    /// attached in a second mount of a part of `/srv` is mounted beneath `/srv`: its
    /// attachment point is a place, and the text of its mount point decides nothing.
    #[test]
    fn a_bind_of_a_subdirectory_is_reached_by_place() {
        let tree = tree(
            "\
530 500 259:2 /data/part /srv/bound rw - ext4 /dev/a rw
531 500 259:2 /srv/sub /mnt/sub rw - ext4 /dev/a rw
532 531 0:53 / /mnt/sub/deep rw - tmpfs tmpfs rw
",
        );
        assert_eq!(
            devices(&tree, 500, "/srv"),
            ["259:2 /srv", "259:2 /data/part", "0:53 /"]
        );
        let reached = tree
            .reaches(&on(&tree, 500, "/srv"), Unlisted::Skipped)
            .expect("reached")
            .places()
            .to_vec();
        assert!(lies_in_any(&on(&tree, 500, "/data/part/file"), &reached));
        assert!(lies_in_any(&on(&tree, 530, "/srv/bound/file"), &reached));
        assert!(!lies_in_any(&on(&tree, 500, "/data/other"), &reached));
        assert!(lies_in_any(&on(&tree, 532, "/mnt/sub/deep/file"), &reached));
        // Design 35 § 20's own table: `/t`, and its `bin` bound at `/t/one/deep`. What
        // `/t/one` reaches holds the place of `/t/bin`, and `/t/two` reaches itself only.
        let bound = self::tree("540 500 259:2 /t/bin /t/one/deep rw - ext4 /dev/a rw\n");
        assert_eq!(
            devices(&bound, 500, "/t/one"),
            ["259:2 /t/one", "259:2 /t/bin"]
        );
        assert_eq!(devices(&bound, 500, "/t/two"), ["259:2 /t/two"]);
        let one = bound
            .reaches(&on(&bound, 500, "/t/one"), Unlisted::Skipped)
            .expect("reached")
            .places()
            .to_vec();
        assert!(lies_in_any(&on(&bound, 500, "/t/bin/tool"), &one));
        assert!(overlap(&[on(&bound, 500, "/t/bin/tool")], &one));
        // Two sets overlap when a place of one lies in a place of the other, either way.
        let data = tree
            .reaches(&on(&tree, 500, "/data"), Unlisted::Skipped)
            .expect("reached")
            .places()
            .to_vec();
        assert_eq!(data, [on(&tree, 500, "/data")]);
        assert!(
            overlap(&data, &reached),
            "/data holds what /srv/bound shows"
        );
        assert!(overlap(&reached, &data));
        let other = [on(&tree, 500, "/data/other")];
        assert!(!overlap(&other, &reached));
        assert!(!overlap(&reached, &other));
    }

    /// D30, `mount-covered`: a mount beneath the directory that a later mount hides keeps its
    /// row, and its root is not what its mount point shows. The kernel is asked at the mount
    /// point: another mount there, or no such path any more, refuses. Only where the rule
    /// looks: the same table asked about a directory beside it answers.
    #[test]
    fn a_hidden_mount_beneath_a_directory_refuses_mount_covered() {
        let rows = "\
550 500 0:55 / /srv/hold/sub rw - tmpfs tmpfs rw
551 500 0:56 / /srv/hold rw - tmpfs tmpfs rw
";
        // Written in this order, the last row at each mount point is its own: the text of a
        // table cannot say that `/srv/hold/sub` is gone. The kernel can.
        for (shows, cause) in [
            (Err(Cause::MountCovered), Cause::MountCovered),
            (Ok(551), Cause::MountCovered),
            (Err(Cause::PathUnopenable), Cause::PathUnopenable),
            (Err(Cause::MountIdUnreported), Cause::MountIdUnreported),
        ] {
            let tree = MountTable::parse(&format!("{BESIDE}{rows}"))
                .expect("the table parses")
                .into_tree_reported_by(move |mount_point| match mount_point {
                    "/srv/hold/sub" => shows.clone(),
                    "/srv/hold" => Ok(551),
                    "/tmp" => Ok(501),
                    _ => Ok(500),
                })
                .expect("every row is attached");
            let why = unreached(&tree, 500, "/srv", Unlisted::Skipped);
            assert_eq!(
                why,
                Unestablished {
                    path: "/srv/hold/sub".to_owned(),
                    cause: cause.clone(),
                    asked: Asked::Attachment,
                }
            );
            assert_eq!(
                why.to_string(),
                format!("cannot establish where /srv/hold/sub is attached: {cause}")
            );
            assert_eq!(devices(&tree, 500, "/opt"), ["259:2 /opt"]);
            assert_eq!(devices(&tree, 551, "/srv/hold"), ["0:56 /"]);
        }
        assert_eq!(Cause::MountCovered.name(), "mount-covered");
        // With the kernel's answer equal to the row's ID the mount is taken.
        let shown = MountTable::parse(&format!("{BESIDE}{rows}"))
            .expect("the table parses")
            .into_tree_reported_by(|mount_point| match mount_point {
                "/srv/hold/sub" => Ok(550),
                "/srv/hold" => Ok(551),
                _ => Err(Cause::PathUnopenable),
            })
            .expect("every row is attached");
        assert_eq!(
            devices(&shown, 500, "/srv"),
            ["259:2 /srv", "0:55 /", "0:56 /"]
        );
    }

    /// D30, a stack: two mounts at one mount point, the second attached on the root of the
    /// first. The lower one's root is not what the path shows, so a directory it is beneath
    /// refuses `mount-covered`; the table as a whole does not, and a directory beside the
    /// stack answers.
    #[test]
    fn stacked_mounts_beneath_a_directory_refuse_mount_covered() {
        let tree = tree(
            "\
560 500 0:60 / /srv/stack rw - tmpfs tmpfs rw
561 560 0:61 / /srv/stack rw - tmpfs tmpfs rw
",
        );
        let why = unreached(&tree, 500, "/srv", Unlisted::Skipped);
        assert_eq!(why.path, "/srv/stack");
        assert_eq!(why.cause, Cause::MountCovered);
        assert_eq!(why.asked, Asked::Attachment);
        assert_eq!(devices(&tree, 500, "/opt"), ["259:2 /opt"]);
        // The upper mount is what the path shows, and nothing is mounted beneath it.
        assert_eq!(devices(&tree, 561, "/srv/stack"), ["0:61 /"]);
        // An unlisted stack is not compared, so it refuses nothing: the two rows this host
        // has at `/proc/sys/fs/binfmt_misc`.
        let unlisted = self::tree(
            "\
562 500 0:21 / /proc rw - proc proc rw
563 562 0:36 / /proc/sys/fs/binfmt_misc rw - autofs systemd-1 rw
564 563 0:62 / /proc/sys/fs/binfmt_misc rw - binfmt_misc binfmt_misc rw
",
        );
        assert_eq!(devices(&unlisted, 500, "/"), ["259:2 /", "0:40 /"]);
    }

    /// D30, the whole table: a row whose attachment cannot be established may be mounted
    /// beneath anything, so it refuses whatever is asked. A parent ID no row has, on a row
    /// that is not the one root at `/`; a second row with no parent; rows that name each
    /// other.
    #[test]
    fn a_row_with_a_missing_parent_refuses_the_table() {
        for (rows, mount_point) in [
            (
                "570 999 0:70 / /srv/lost rw - tmpfs tmpfs rw\n",
                "/srv/lost",
            ),
            ("571 998 0:71 / / rw - tmpfs tmpfs rw\n", "/"),
            (
                "572 573 0:72 / /srv/a rw - tmpfs tmpfs rw\n\
                 573 572 0:73 / /srv/a rw - tmpfs tmpfs rw\n",
                "/srv/a",
            ),
        ] {
            let why = unattached(&format!("{BESIDE}{rows}"));
            assert_eq!(
                why,
                Unestablished {
                    path: mount_point.to_owned(),
                    cause: Cause::MountRowMissing,
                    asked: Asked::Attachment,
                },
                "{rows}"
            );
            assert_eq!(
                why.to_string(),
                format!("cannot establish where {mount_point} is attached: mount-row-missing")
            );
        }
        // No root at all: the only row without a parent is not at `/`.
        let why = unattached("510 400 0:50 / /srv rw - tmpfs tmpfs rw\n");
        assert_eq!(
            (why.path.as_str(), why.cause),
            ("/srv", Cause::MountRowMissing)
        );
        // A row that names itself has no parent row either: at `/` it is the root, and
        // anywhere else it is attached nowhere.
        let own = MountTable::parse(
            "\
1 1 0:1 / / rw - tmpfs rootfs rw
2 1 0:50 / /srv rw - tmpfs tmpfs rw
",
        )
        .expect("the table parses")
        .into_tree_as_written()
        .expect("a root that is its own parent");
        assert_eq!(devices(&own, 1, "/"), ["0:1 /", "0:50 /"]);
        let why = unattached(&format!(
            "{BESIDE}510 510 0:50 / /srv rw - tmpfs tmpfs rw\n"
        ));
        assert_eq!(
            (why.path.as_str(), why.cause),
            ("/srv", Cause::MountRowMissing)
        );
        // The one root, alone, is a tree.
        assert_eq!(devices(&tree(""), 500, "/"), ["259:2 /", "0:40 /"]);
    }

    /// D30, the whole table: more than one row has the ID a row names as its parent.
    #[test]
    fn a_row_with_a_repeated_parent_refuses_the_table() {
        let why = unattached(&format!(
            "{BESIDE}\
580 500 0:80 / /srv/twice rw - tmpfs tmpfs rw
580 500 0:81 / /srv/again rw - tmpfs tmpfs rw
581 580 0:82 / /srv/twice/in rw - tmpfs tmpfs rw
"
        ));
        assert_eq!(
            why,
            Unestablished {
                path: "/srv/twice/in".to_owned(),
                cause: Cause::MountRowRepeated,
                asked: Asked::Attachment,
            }
        );
        // Two rows with one ID that no row names as its parent are no attachment's question:
        // a path on that mount refuses for itself (D26).
        let tree = tree(
            "\
580 500 0:80 / /srv/twice rw - tmpfs tmpfs rw
580 500 0:81 / /srv/again rw - tmpfs tmpfs rw
",
        );
        assert_eq!(
            tree.table().place(Some(580), Some("/srv/twice")),
            Err(Cause::MountRowRepeated)
        );
    }

    /// D30: a mount point with an escaped space is taken relative to its parent's after both
    /// are unescaped, so the attachment point holds the names as they are.
    #[test]
    fn an_escaped_space_in_a_mount_point_is_unescaped_before_it_is_taken_relative() {
        let tree = tree(
            "\
520 500 259:2 /data/my\\040tools /srv/with\\040space rw - ext4 /dev/a rw
521 520 0:57 / /srv/with\\040space/in\\040it rw - tmpfs tmpfs rw
",
        );
        assert_eq!(
            devices(&tree, 500, "/srv"),
            ["259:2 /srv", "259:2 /data/my tools", "0:57 /"]
        );
        // The second mount is attached in the first at `/data/my tools/in it`.
        assert_eq!(
            devices(&tree, 500, "/data/my tools/in it"),
            ["259:2 /data/my tools/in it", "0:57 /"]
        );
        assert_eq!(devices(&tree, 500, "/data/my"), ["259:2 /data/my"]);
    }

    /// D30, the whole table: a row's mount point is not its parent row's or beneath it, so
    /// where it is attached in the parent cannot be read from the two.
    #[test]
    fn a_mount_point_outside_its_parent_refuses_the_table() {
        for rows in [
            "590 501 0:90 / /opt/elsewhere rw - tmpfs tmpfs rw\n",
            "590 501 0:90 / /tmpfs rw - tmpfs tmpfs rw\n",
            "590 501 0:90 / relative rw - tmpfs tmpfs rw\n",
        ] {
            let why = unattached(&format!("{BESIDE}{rows}"));
            assert_eq!(why.cause, Cause::MountPointMismatch, "{rows}");
            assert_eq!(why.asked, Asked::Attachment, "{rows}");
            assert!(
                why.to_string()
                    .ends_with("is attached: mount-point-mismatch"),
                "{why}"
            );
        }
    }

    /// D30, `mount-root-deleted`: the directory a mount beneath the asked directory shows was
    /// removed, so its root is no place. Refused where the rule looks, and nowhere else. A
    /// mount attached in it is still found beneath the directory, through its parent.
    #[test]
    fn a_deleted_root_beneath_a_directory_refuses() {
        let tree = tree(
            "\
600 500 259:2 /gone//deleted /srv/removed rw - ext4 /dev/a rw
601 600 0:91 / /srv/removed/in rw - tmpfs tmpfs rw
",
        );
        let why = unreached(&tree, 500, "/srv", Unlisted::Skipped);
        assert_eq!(
            why,
            Unestablished {
                path: "/srv/removed".to_owned(),
                cause: Cause::MountRootDeleted,
                asked: Asked::Attachment,
            }
        );
        assert_eq!(devices(&tree, 500, "/opt"), ["259:2 /opt"]);
        // A deleted root holds no attachment point, so a mount attached in it is beneath
        // nothing by its own attachment: it is found through its parent, or not at all.
        assert_eq!(devices(&tree, 601, "/srv/removed/in"), ["0:91 /"]);
        assert_eq!(devices(&tree, 500, "/gone"), ["259:2 /gone"]);
    }

    /// D30, a file-system type outside the list: such a mount is not compared. Beneath a
    /// location a run can write it is skipped and refuses nothing, as each device node
    /// bubblewrap binds beneath `/dev` is; where the caller refuses it, the cause names the
    /// type. A listed mount attached in a skipped one is still reached.
    #[test]
    fn an_unlisted_mount_beneath_a_directory_is_skipped_or_refused() {
        let tree = tree(
            "\
610 500 0:95 / /srv/dev rw - tmpfs tmpfs rw
611 610 0:5 /null /srv/dev/null rw - devtmpfs devtmpfs rw
612 610 0:96 / /srv/dev/pts rw - devpts devpts rw
613 500 8:17 / /srv/data rw - xfs /dev/sdb1 rw
614 613 0:97 / /srv/data/scratch rw - tmpfs tmpfs rw
",
        );
        assert_eq!(
            devices(&tree, 500, "/srv"),
            ["259:2 /srv", "0:95 /", "0:97 /"]
        );
        assert_eq!(devices(&tree, 610, "/srv/dev"), ["0:95 /"]);
        let why = unreached(&tree, 500, "/srv", Unlisted::Refused);
        assert_eq!(
            why,
            Unestablished {
                path: "/srv/dev/null".to_owned(),
                cause: Cause::FileSystemUnlisted("devtmpfs".to_owned()),
                asked: Asked::Attachment,
            }
        );
        assert_eq!(
            why.to_string(),
            "cannot establish where /srv/dev/null is attached: file-system-unlisted devtmpfs"
        );
        // Beside every unlisted mount the refusing form answers too.
        assert_eq!(
            tree.reaches(&on(&tree, 500, "/opt"), Unlisted::Refused)
                .map(|reach| reach.places().to_vec()),
            Ok(vec![on(&tree, 500, "/opt")])
        );
    }

    /// The reader on this host: the table of this process is a tree, the kernel reports the
    /// root of the view at `/`, and a path that is gone is reported `mount-covered`.
    #[cfg(target_os = "linux")]
    #[test]
    fn the_kernel_reports_the_mount_at_a_mount_point() {
        let root = Opened::of_path("/").expect("the root opens");
        assert_eq!(reported_at("/").ok(), root.mount_id);
        assert!(root.mount_id.is_some(), "statx reported no mount ID");
        let directory = tempfile::tempdir().expect("a directory");
        let gone = directory.path().join("gone/below");
        assert_eq!(
            reported_at(gone.to_str().expect("UTF-8")),
            Err(Cause::MountCovered)
        );
        let file = directory.path().join("file");
        std::fs::write(&file, b"").expect("a file");
        assert_eq!(
            reported_at(file.join("below").to_str().expect("UTF-8")),
            Err(Cause::MountCovered),
            "a name beneath a file"
        );
        assert_eq!(reported_at("with\0nul"), Err(Cause::PathUnopenable));
        let table = MountTable::read().expect("this process's mount table");
        match table.into_tree() {
            Ok(tree) => {
                let here = Opened::of_path(directory.path().to_str().expect("UTF-8"))
                    .expect("the directory opens");
                match tree.table().place_of(&here) {
                    Ok(place) => {
                        // Nothing is mounted beneath a directory this case just made.
                        assert_eq!(
                            tree.reaches(&place, Unlisted::Refused)
                                .map(|reach| reach.places().to_vec()),
                            Ok(vec![place.clone()])
                        );
                        println!("place mount tree of this process: executed");
                    }
                    Err(why) => println!("place mount tree of this process: ABSENT ({why})"),
                }
            }
            // A process whose root is no mount of its own table (a `chroot`) has no tree.
            Err(why) => println!("place mount tree of this process: ABSENT ({why})"),
        }
    }

    /// D30, the side nothing is written through: what a directory reaches is read from the
    /// table alone and never refuses. A stack, a mount a later one hides, a removed root and a
    /// mount ID two rows have are each counted by the root their row names; the same table
    /// refuses the side a run can write, cause by cause.
    #[test]
    fn what_a_source_reaches_is_read_from_the_table_and_never_refuses() {
        let as_written = |tree: &MountTree, path: &str| -> Vec<String> {
            tree.reaches_as_written(&on(tree, 500, path))
                .places()
                .iter()
                .map(|place| format!("{} /{}", place.device, place.names.join("/")))
                .collect()
        };
        for (rows, reached, cause) in [
            (
                "560 500 0:60 / /srv/stack rw - tmpfs tmpfs rw\n\
                 561 560 0:61 / /srv/stack rw - tmpfs tmpfs rw\n",
                vec!["259:2 /srv", "0:60 /", "0:61 /"],
                Cause::MountCovered,
            ),
            (
                "600 500 259:2 /gone//deleted /srv/removed rw - ext4 /dev/a rw\n",
                vec!["259:2 /srv", "259:2 /gone"],
                Cause::MountRootDeleted,
            ),
            (
                "580 500 0:80 / /srv/twice rw - tmpfs tmpfs rw\n\
                 580 500 0:81 / /opt/again rw - tmpfs tmpfs rw\n",
                vec!["259:2 /srv", "0:80 /"],
                Cause::MountRowRepeated,
            ),
        ] {
            let tree = tree(rows);
            assert_eq!(as_written(&tree, "/srv"), reached, "{rows}");
            assert_eq!(as_written(&tree, "/usr"), ["259:2 /usr"], "{rows}");
            assert_eq!(
                unreached(&tree, 500, "/srv", Unlisted::Skipped).cause,
                cause,
                "{rows}"
            );
        }
        // A mount the kernel cannot be asked about, for each answer it can give.
        let rows = "550 500 0:55 / /srv/hold/sub rw - tmpfs tmpfs rw\n";
        for shows in [
            Err(Cause::MountCovered),
            Err(Cause::PathUnopenable),
            Err(Cause::MountIdUnreported),
            Ok(999),
        ] {
            let tree = MountTable::parse(&format!("{BESIDE}{rows}"))
                .expect("the table parses")
                .into_tree_reported_by(move |_| shows.clone())
                .expect("every row is attached");
            assert_eq!(as_written(&tree, "/srv"), ["259:2 /srv", "0:55 /"]);
            assert!(
                tree.reaches(&on(&tree, 500, "/srv"), Unlisted::Skipped)
                    .is_err()
            );
        }
        // On a table with nothing to refuse the two sides reach the same.
        let plain = tree("510 500 0:50 / /srv/one rw - tmpfs tmpfs rw\n");
        assert_eq!(
            Ok(plain.reaches_as_written(&on(&plain, 500, "/srv"))),
            plain.reaches(&on(&plain, 500, "/srv"), Unlisted::Skipped)
        );
    }

    /// D30, a mount ID two rows have, beneath a location a run can write: which of the two
    /// the kernel shows at the mount point cannot be said, as for a path (D26). Neither row
    /// is a parent, so the check of the whole table does not see them; asked about a
    /// directory beside both, the table answers.
    #[test]
    fn a_repeated_mount_id_beneath_a_directory_refuses_mount_row_repeated() {
        let tree = tree(
            "\
580 500 0:80 / /srv/twice rw - tmpfs tmpfs rw
580 500 0:81 / /opt/again rw - tmpfs tmpfs rw
",
        );
        for (directory, mount_point) in [("/srv", "/srv/twice"), ("/opt", "/opt/again")] {
            assert_eq!(
                unreached(&tree, 500, directory, Unlisted::Skipped),
                Unestablished {
                    path: mount_point.to_owned(),
                    cause: Cause::MountRowRepeated,
                    asked: Asked::Attachment,
                }
            );
        }
        assert_eq!(devices(&tree, 500, "/usr"), ["259:2 /usr"]);
        // Of a type outside the list the rows are not compared, and refuse nothing.
        let unlisted = self::tree(
            "\
580 500 0:80 / /srv/twice rw - devpts devpts rw
580 500 0:81 / /opt/again rw - devpts devpts rw
",
        );
        assert_eq!(devices(&unlisted, 500, "/srv"), ["259:2 /srv"]);
    }

    /// A table given as text: the mount the kernel is taken to show at a mount point is the
    /// top of the stack there by the rows' parent links, in whatever order the rows are
    /// written.
    #[test]
    fn the_top_of_a_stack_is_read_from_the_parent_links() {
        for rows in [
            "560 500 0:60 / /srv/stack rw - tmpfs tmpfs rw\n\
             561 560 0:61 / /srv/stack rw - tmpfs tmpfs rw\n\
             562 561 0:62 / /srv/stack rw - tmpfs tmpfs rw\n",
            "562 561 0:62 / /srv/stack rw - tmpfs tmpfs rw\n\
             561 560 0:61 / /srv/stack rw - tmpfs tmpfs rw\n\
             560 500 0:60 / /srv/stack rw - tmpfs tmpfs rw\n",
        ] {
            let tree = tree(rows);
            // The top is what the path shows, and nothing is mounted beneath it.
            assert_eq!(devices(&tree, 562, "/srv/stack"), ["0:62 /"], "{rows}");
            // The middle one reaches the top, which is shown; `/srv` reaches the covered ones.
            assert_eq!(
                devices(&tree, 561, "/srv/stack"),
                ["0:61 /", "0:62 /"],
                "{rows}"
            );
            let why = unreached(&tree, 500, "/srv", Unlisted::Skipped);
            assert_eq!(
                (why.path.as_str(), why.cause),
                ("/srv/stack", Cause::MountCovered),
                "{rows}"
            );
        }
    }

    /// D30 and a type outside the list: the root of such a mount is no place and holds no
    /// path, but its row is kept, and two directories that each reach a row of one device,
    /// the root of one in the root of the other, overlap. Row against row only: a place on a
    /// listed type with the same names is apart from it.
    #[test]
    fn two_rows_of_an_unlisted_type_with_one_device_overlap() {
        let tree = tree(
            "\
620 500 0:70 / /srv/media rw - ramfs ramfs rw
621 500 0:70 / /opt/media rw - ramfs ramfs rw
622 500 0:70 /sub /mnt/part rw - ramfs ramfs rw
623 500 0:71 / /var/other rw - ramfs ramfs rw
624 500 0:70 /apart /media/apart rw - ramfs ramfs rw
",
        );
        let reach = |path: &str, as_written: bool| {
            let directory = on(&tree, 500, path);
            if as_written {
                tree.reaches_as_written(&directory)
            } else {
                tree.reaches(&directory, Unlisted::Skipped)
                    .expect("reached")
            }
        };
        for as_written in [false, true] {
            let srv = reach("/srv", as_written);
            assert_eq!(srv.places(), [on(&tree, 500, "/srv")], "no place is added");
            for (path, overlaps) in [
                ("/opt", true),
                ("/mnt", true),
                ("/var", false),
                ("/media", true),
                ("/usr", false),
            ] {
                for other in [reach(path, false), reach(path, true)] {
                    assert_eq!(srv.overlaps(&other), overlaps, "/srv and {path}");
                    assert_eq!(other.overlaps(&srv), overlaps, "{path} and /srv");
                }
            }
            // Two parts of the file system that are apart do not overlap.
            assert!(!reach("/mnt", as_written).overlaps(&reach("/media", as_written)));
            // No path lies in it for the row: a place is never compared with a row.
            let named_alike = Place {
                device: "0:70".to_owned(),
                names: vec!["file".to_owned()],
            };
            assert!(!srv.holds(&named_alike));
        }
    }

    /// The kernel is asked at the path itself, never at what a symbolic link there names: a
    /// link to a directory of another mount reports the link's own mount.
    #[cfg(target_os = "linux")]
    #[test]
    fn the_kernel_is_asked_at_a_mount_point_and_not_through_a_link_there() {
        let directory = tempfile::tempdir().expect("a directory");
        let here = Opened::of_path(directory.path().to_str().expect("UTF-8"))
            .expect("the directory opens")
            .mount_id
            .expect("statx reported no mount ID");
        let Some(elsewhere) = ["/proc", "/sys", "/dev", "/run", "/"]
            .into_iter()
            .find(|path| reported_at(path).is_ok_and(|id| id != here))
        else {
            println!("place question at a link: ABSENT (this host has one mount)");
            return;
        };
        let link = directory.path().join("link");
        std::os::unix::fs::symlink(elsewhere, &link).expect("a link");
        assert_eq!(
            reported_at(link.to_str().expect("UTF-8")),
            Ok(here),
            "a link to {elsewhere} reported the mount it names"
        );
        println!("place question at a link: executed");
    }
}
