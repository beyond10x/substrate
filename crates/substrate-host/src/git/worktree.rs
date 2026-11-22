//! The worktree side of the Git changes observation.
//!
//! Every entry is observed and read beneath the pinned workspace directory with the guarded file
//! system: `openat2` beneath that directory, no link followed, at most the caller's byte bound of
//! any one file. Nothing the workspace's repository records chooses the directory that is read
//! or the way an entry is read; a link is read as its own target text, never as what it names.

use std::ffi::OsString;
use std::os::fd::OwnedFd;
use std::os::unix::ffi::OsStrExt as _;
use std::os::unix::fs::MetadataExt as _;

use gix::bstr::{BStr, ByteSlice as _};
use gix::glob::pattern::Case;
use gix::glob::{Pattern, wildmatch};
use substrate_guarded::fs as guarded;
use substrate_wire::{MAX_IO_BYTES, MAX_PATH_DEPTH};

use super::failed;
use crate::{DriverError, DriverErrorClass};

pub(super) const FILE: u32 = 0o100_644;
pub(super) const EXECUTABLE: u32 = 0o100_755;
pub(super) const LINK: u32 = 0o120_000;

/// One regular file or link beneath the workspace root, observed without following it.
#[derive(Clone, Copy)]
pub(super) struct Observed {
    /// The Git mode of the entry: [`FILE`], [`EXECUTABLE`] or [`LINK`].
    pub(super) mode: u32,
    pub(super) size: u64,
    /// Seconds and nanoseconds of the last modification.
    pub(super) modified: (i64, i64),
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
        modified: (metadata.mtime(), metadata.mtime_nsec()),
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
        let Some(link) = open(directory, path)? else {
            return Ok(None);
        };
        return Ok(nix::fcntl::readlinkat(&link, "")
            .ok()
            .map(OsString::into_encoded_bytes)
            .filter(|target| u64::try_from(target.len()).is_ok_and(|length| length <= bound)));
    }
    match guarded::read_regular_beneath(directory, path, bound) {
        Ok(bytes) => Ok(bytes),
        Err(error) if absent(&error) || error.class == DriverErrorClass::Exhausted => Ok(None),
        Err(_) => Err(failed("workspace.git-diff-failed")),
    }
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
) -> Result<Vec<(String, Observed)>, DriverError> {
    let mut walk = Walk {
        claimed,
        tracked_beneath,
        ignores: Vec::new(),
        found: Vec::new(),
    };
    if let Some(rules) = Ignores::read(root, ".git/info/exclude", "") {
        walk.ignores.push(rules);
    }
    walk.descend(root, "", 1)?;
    Ok(walk.found)
}

struct Walk<'a> {
    claimed: &'a dyn Fn(&str, bool) -> bool,
    tracked_beneath: &'a dyn Fn(&str) -> bool,
    /// The ignore files that apply to the directory being listed, outermost first.
    ignores: Vec<Ignores>,
    found: Vec<(String, Observed)>,
}

impl Walk<'_> {
    fn descend(
        &mut self,
        directory: &OwnedFd,
        prefix: &str,
        depth: usize,
    ) -> Result<(), DriverError> {
        if depth > MAX_PATH_DEPTH {
            return Err(DriverError::refused(
                "workspace.path-escape",
                "Workspace path escapes its root.",
                "path",
            ));
        }
        let rules = Ignores::read(directory, ".gitignore", prefix);
        let scoped = rules.is_some();
        self.ignores.extend(rules);
        for (name, is_directory) in entries(directory)? {
            if name == ".git" {
                continue;
            }
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
                if let Some(child) = open_directory(directory, name)?
                    && ((self.tracked_beneath)(&beneath) || open(&child, ".git")?.is_none())
                {
                    self.descend(&child, &beneath, depth + 1)?;
                }
            } else if let Some(observed) = observe(directory, name)? {
                self.found.push((path, observed));
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

/// The names in `directory` and whether each is a directory, in byte order.
fn entries(directory: &OwnedFd) -> Result<Vec<(OsString, bool)>, DriverError> {
    let mut entries = std::fs::read_dir(descriptor_path(directory))
        .and_then(|listing| {
            listing
                .map(|entry| {
                    let entry = entry?;
                    Ok((entry.file_name(), entry.file_type()?.is_dir()))
                })
                .collect::<std::io::Result<Vec<_>>>()
        })
        .map_err(|_| failed("workspace.git-diff-failed"))?;
    entries.sort();
    Ok(entries)
}

/// The rules of one ignore file and the directory they are relative to: `""` or `directory/`.
struct Ignores {
    base: String,
    rules: Vec<Pattern>,
}

impl Ignores {
    /// The rules of the regular file at `path` beneath `directory`, read within the public I/O
    /// bound. A file that is absent, a link, or larger than the bound contributes no rule.
    fn read(directory: &OwnedFd, path: &str, base: &str) -> Option<Self> {
        let bytes = guarded::read_regular_beneath(directory, path, MAX_IO_BYTES).ok()??;
        let rules = bytes
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
