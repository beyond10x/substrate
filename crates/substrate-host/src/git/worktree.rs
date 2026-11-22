//! The worktree side of the Git changes observation.
//!
//! Every entry is observed and read beneath the pinned workspace directory with the guarded file
//! system: `openat2` beneath that directory, no link followed, no open that waits. Nothing the
//! workspace's repository records chooses the directory that is read or the way an entry is
//! read; a link is read as its own target text, never as what it names.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io::Read as _;
use std::os::fd::OwnedFd;
use std::os::unix::ffi::OsStrExt as _;
use std::os::unix::fs::{MetadataExt as _, OpenOptionsExt as _};

use git2::{ObjectType, Oid};
use gix::bstr::{BStr, ByteSlice as _};
use gix::glob::pattern::Case;
use gix::glob::{Pattern, wildmatch};
use substrate_guarded::fs as guarded;
use substrate_wire::MAX_IO_BYTES;

use super::failed;
use crate::{DriverError, DriverErrorClass};

pub(super) const FILE: u32 = 0o100_644;
pub(super) const EXECUTABLE: u32 = 0o100_755;
pub(super) const LINK: u32 = 0o120_000;

/// Directories nested deeper than this beneath the workspace root are not entered.
const MAX_WALK_DEPTH: usize = 100;

/// One regular file or link beneath the workspace root, observed without following it.
#[derive(Clone, Copy)]
pub(super) struct Observed {
    /// The Git mode of the entry: [`FILE`], [`EXECUTABLE`] or [`LINK`].
    pub(super) mode: u32,
    pub(super) size: u64,
}

/// What hashing an observed entry gave.
pub(super) enum Hashed {
    /// The Git object id of the entry's content.
    Id(Oid),
    /// The entry is gone, or changed while it was read.
    Unstable,
    /// The entry is larger than what is left of the budget; none of it was read.
    OverBudget,
}

/// The path of an open descriptor, for a reader that takes a path.
pub(super) fn descriptor_path(descriptor: &impl std::os::fd::AsRawFd) -> String {
    format!("/proc/self/fd/{}", descriptor.as_raw_fd())
}

/// The entry at `path` beneath `directory`, opened without following a link. `None` is an absent
/// entry and one whose path crosses a link, a file or another mount.
pub(super) fn open(directory: &OwnedFd, path: &str) -> Result<Option<std::fs::File>, DriverError> {
    match guarded::open_component_beneath(directory, path) {
        Ok(entry) => Ok(Some(std::fs::File::from(entry))),
        Err(error) if absent(&error) => Ok(None),
        Err(_) => Err(failed("workspace.git-diff-failed")),
    }
}

/// The directory at `path` beneath `directory`; `None` when it is absent or not a directory.
pub(super) fn open_directory(
    directory: &OwnedFd,
    path: &str,
) -> Result<Option<OwnedFd>, DriverError> {
    let Some(entry) = open(directory, path)? else {
        return Ok(None);
    };
    let metadata = entry
        .metadata()
        .map_err(|_| failed("workspace.git-diff-failed"))?;
    Ok(metadata.is_dir().then(|| OwnedFd::from(entry)))
}

/// The regular file at `path` beneath `directory`, opened for reading, and its size. The entry
/// is held beneath the directory first and opened for reading only once it is known to be a
/// regular file, so the open neither waits nor leaves the directory.
fn regular(directory: &OwnedFd, path: &str) -> Result<Option<(std::fs::File, u64)>, DriverError> {
    let Some(held) = open(directory, path)? else {
        return Ok(None);
    };
    if !held.metadata().is_ok_and(|metadata| metadata.is_file()) {
        return Ok(None);
    }
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(descriptor_path(&held))
        .map_err(|_| failed("workspace.git-diff-failed"))?;
    let size = file
        .metadata()
        .map_err(|_| failed("workspace.git-diff-failed"))?
        .size();
    Ok(Some((file, size)))
}

/// The target text of the link at `path` beneath `directory`; `None` when it is not a link.
fn link_target(directory: &OwnedFd, path: &str) -> Result<Option<Vec<u8>>, DriverError> {
    let Some(link) = open(directory, path)? else {
        return Ok(None);
    };
    Ok(nix::fcntl::readlinkat(&link, "")
        .ok()
        .map(OsString::into_encoded_bytes))
}

/// What the entry at `path` beneath `directory` is; `None` for an absent entry and for one that
/// is neither a regular file nor a link.
pub(super) fn observe(directory: &OwnedFd, path: &str) -> Result<Option<Observed>, DriverError> {
    let Some(entry) = open(directory, path)? else {
        return Ok(None);
    };
    let metadata = entry
        .metadata()
        .map_err(|_| failed("workspace.git-diff-failed"))?;
    let mode = match (metadata.file_type().is_symlink(), metadata.is_file()) {
        (true, _) => LINK,
        (false, true) if metadata.mode() & 0o100 != 0 => EXECUTABLE,
        (false, true) => FILE,
        (false, false) => return Ok(None),
    };
    Ok(Some(Observed {
        mode,
        size: metadata.size(),
    }))
}

/// The bytes of the observed entry at `path`: a regular file's content, or a link's own target
/// text. `None` when the entry is larger than `bound`, gone, or no longer what was observed.
pub(super) fn read(
    directory: &OwnedFd,
    path: &str,
    observed: Observed,
    bound: u64,
) -> Result<Option<Vec<u8>>, DriverError> {
    if observed.size > bound {
        return Ok(None);
    }
    if observed.mode == LINK {
        return Ok(link_target(directory, path)?
            .filter(|target| u64::try_from(target.len()).is_ok_and(|length| length <= bound)));
    }
    match guarded::read_regular_beneath(directory, path, bound) {
        Ok(bytes) => Ok(bytes),
        Err(error) if absent(&error) || error.class == DriverErrorClass::Exhausted => Ok(None),
        Err(_) => Err(failed("workspace.git-diff-failed")),
    }
}

/// The Git object id of the observed entry's content, read as a stream: a regular file's bytes,
/// or a link's own target text. The entry's size is taken from `budget` before any of it is
/// read; an entry larger than what is left is not read.
pub(super) fn blob_id(
    directory: &OwnedFd,
    path: &str,
    observed: Observed,
    budget: &mut u64,
) -> Result<Hashed, DriverError> {
    if observed.mode == LINK {
        let Some(spent) = budget.checked_sub(observed.size) else {
            return Ok(Hashed::OverBudget);
        };
        *budget = spent;
        return Ok(link_target(directory, path)?
            .and_then(|target| Oid::hash_object(ObjectType::Blob, &target).ok())
            .map_or(Hashed::Unstable, Hashed::Id));
    }
    let Some((file, size)) = regular(directory, path)? else {
        return Ok(Hashed::Unstable);
    };
    let Some(spent) = budget.checked_sub(size) else {
        return Ok(Hashed::OverBudget);
    };
    *budget = spent;
    let mut digest = Digest(gix::hash::hasher(gix::hash::Kind::Sha1));
    digest.0.update(format!("blob {size}\0").as_bytes());
    // One byte past the size tells a file that grew while it was read.
    let read = std::io::copy(&mut file.take(size.saturating_add(1)), &mut digest)
        .map_err(|_| failed("workspace.git-diff-failed"))?;
    if read != size {
        return Ok(Hashed::Unstable);
    }
    Ok(digest
        .0
        .try_finalize()
        .ok()
        .and_then(|id| Oid::from_bytes(id.as_slice()).ok())
        .map_or(Hashed::Unstable, Hashed::Id))
}

/// A hash fed as a writer.
struct Digest(gix::hash::Hasher);

impl std::io::Write for Digest {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.update(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// The untracked entries of a worktree. `truncated` says that a directory was not entered,
/// being nested deeper than the walk goes.
pub(super) struct Unclaimed {
    pub(super) found: Vec<(String, Observed)>,
    pub(super) truncated: bool,
}

/// Every regular file and link beneath `root` that `claimed` does not answer for and no ignore
/// rule of the workspace covers, with what it is. `claimed` is asked with a path and whether it
/// is a directory; `tracked_beneath` says whether a tracked path lies beneath a directory, named
/// with its closing `/`. The repository's own directory is not entered, and neither is a
/// directory without tracked paths that holds a repository of its own.
pub(super) fn unclaimed(
    root: &OwnedFd,
    claimed: &dyn Fn(&str, bool) -> bool,
    tracked_beneath: &dyn Fn(&str) -> bool,
) -> Result<Unclaimed, DriverError> {
    let mut walk = Walk {
        claimed,
        tracked_beneath,
        ignores: Vec::new(),
        unclaimed: Unclaimed {
            found: Vec::new(),
            truncated: false,
        },
    };
    if let Some(rules) = Ignores::read(root, ".git/info/exclude", "") {
        walk.ignores.push(rules);
    }
    walk.descend(root, "", 0)?;
    Ok(walk.unclaimed)
}

struct Walk<'a> {
    claimed: &'a dyn Fn(&str, bool) -> bool,
    tracked_beneath: &'a dyn Fn(&str) -> bool,
    /// The ignore files that apply to the directory being listed, outermost first.
    ignores: Vec<Ignores>,
    unclaimed: Unclaimed,
}

impl Walk<'_> {
    /// Lists `directory`, which lies `level` directories beneath the root.
    fn descend(
        &mut self,
        directory: &OwnedFd,
        prefix: &str,
        level: usize,
    ) -> Result<(), DriverError> {
        let rules = Ignores::read(directory, ".gitignore", prefix);
        let scoped = rules.is_some();
        self.ignores.extend(rules);
        for (name, kind) in entries(directory)? {
            if name == ".git" {
                continue;
            }
            let is_directory = kind.is_dir();
            let path = [prefix.as_bytes(), name.as_bytes()].concat();
            if self.ignored(&path, is_directory) {
                continue;
            }
            let path = String::from_utf8(path)
                .map_err(|_| failed("workspace.git-path-unrepresentable"))?;
            if (self.claimed)(&path, is_directory) {
                continue;
            }
            let name = &path[prefix.len()..];
            if is_directory {
                let beneath = format!("{path}/");
                // An untracked directory that holds a `.git` entry is another repository's.
                if level == MAX_WALK_DEPTH {
                    self.unclaimed.truncated = true;
                } else if let Some(child) = open_directory(directory, name)?
                    && ((self.tracked_beneath)(&beneath) || open(&child, ".git")?.is_none())
                {
                    self.descend(&child, &beneath, level + 1)?;
                }
            } else if let Some(observed) = observe(directory, name)? {
                self.unclaimed.found.push((path, observed));
            }
        }
        if scoped {
            self.ignores.pop();
        }
        Ok(())
    }

    /// Whether the innermost ignore file with a rule for `path` ignores it.
    fn ignored(&self, path: &[u8], is_directory: bool) -> bool {
        self.ignores
            .iter()
            .rev()
            .find_map(|rules| rules.decide(path, is_directory))
            .unwrap_or(false)
    }
}

/// The names in `directory` and what kind of entry each is, in byte order.
fn entries(directory: &OwnedFd) -> Result<BTreeMap<OsString, std::fs::FileType>, DriverError> {
    std::fs::read_dir(descriptor_path(directory))
        .and_then(|listing| {
            listing
                .map(|entry| {
                    let entry = entry?;
                    Ok((entry.file_name(), entry.file_type()?))
                })
                .collect()
        })
        .map_err(|_| failed("workspace.git-diff-failed"))
}

/// The rules of one ignore file and the directory they are relative to: `""` or `directory/`.
struct Ignores {
    base: String,
    rules: Vec<Pattern>,
}

impl Ignores {
    /// The rules in the first [`MAX_IO_BYTES`] of the regular file at `path` beneath
    /// `directory`, a leading byte-order mark skipped. A file that is absent or not a regular
    /// file contributes no rule.
    fn read(directory: &OwnedFd, path: &str, base: &str) -> Option<Self> {
        let (file, size) = regular(directory, path).ok()??;
        let mut bytes = Vec::new();
        file.take(MAX_IO_BYTES).read_to_end(&mut bytes).ok()?;
        if size > MAX_IO_BYTES {
            // A rule applies whole or not at all: the line the bound cuts is dropped.
            bytes.truncate(bytes.rfind_byte(b'\n').map_or(0, |end| end + 1));
        }
        let rules = bytes
            .strip_prefix(b"\xef\xbb\xbf")
            .unwrap_or(bytes.as_slice())
            .lines()
            .filter(|line| !line.starts_with(b"#"))
            .filter_map(|line| Pattern::from_bytes(without_trailing_spaces(line)))
            .collect();
        Some(Self {
            base: base.to_owned(),
            rules,
        })
    }

    /// Whether the last rule that matches `path` ignores it; `None` when no rule matches.
    fn decide(&self, path: &[u8], is_directory: bool) -> Option<bool> {
        let relative = path.strip_prefix(self.base.as_bytes())?;
        let basename = relative.rfind_byte(b'/').map(|separator| separator + 1);
        self.rules
            .iter()
            .rev()
            .find(|rule| {
                rule.matches_repo_relative_path(
                    BStr::new(relative),
                    basename,
                    Some(is_directory),
                    Case::Sensitive,
                    wildmatch::Mode::NO_MATCH_SLASH_LITERAL,
                )
            })
            .map(|rule| !rule.is_negative())
    }
}

/// `line` without its trailing spaces, keeping one that a backslash escapes.
fn without_trailing_spaces(line: &[u8]) -> &[u8] {
    let mut end = line.len();
    while end > 0 && line[end - 1] == b' ' && (end < 2 || line[end - 2] != b'\\') {
        end -= 1;
    }
    &line[..end]
}

fn absent(error: &DriverError) -> bool {
    error.class == DriverErrorClass::NotFound || error.code == "workspace.path-escape"
}
