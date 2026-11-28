//! Creates the entries of a tar beneath a staging directory under the rules of design 38 § 3.3.
//!
//! Every name is created relative to a directory descriptor that was itself opened with
//! [`openat2_beneath`] (beneath, no symbolic link, no magic link, no mount), one component at a
//! time, so no path text is resolved by anything that would follow a link. Regular files are
//! created `O_CREAT | O_EXCL | O_NOFOLLOW`; a symbolic link is `symlinkat` and is never
//! followed; a hard link is `linkat` without `AT_SYMLINK_FOLLOW`, to a regular file the archive
//! already unpacked. A name that exists already is a path named twice.

use std::collections::HashMap;
use std::ffi::{CStr, CString, OsStr};
use std::fs::File;
use std::io::{Read, Write as _};
use std::os::fd::{AsFd as _, BorrowedFd, OwnedFd};
use std::os::unix::ffi::OsStrExt as _;

use nix::errno::Errno;
use nix::fcntl::AtFlags;
use nix::sys::stat::{Mode, SFlag};
use substrate_guarded::fs::openat2_beneath;

use super::tar::{Entry, Kind, TarReader};
use super::{
    EntryRefusal, ImageBound, ImageError, ImageLimits, MAX_IMAGE_LINK_TARGET_BYTES,
    MAX_IMAGE_PATH_BYTES, MAX_IMAGE_PATH_COMPONENT_BYTES,
};

const DIRECTORY_FLAGS: i32 =
    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC;
const CREATE_FLAGS: i32 =
    libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC;
/// The mode every directory has while the unpack writes into it.
const WORKING_DIRECTORY_MODE: u32 = 0o700;
/// The mode a directory gets that the archive creates only as a parent of an entry.
const IMPLIED_DIRECTORY_MODE: u32 = 0o755;
const COPY_BUFFER_BYTES: usize = 64 * 1024;

/// What one unpack created.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Unpacked {
    /// Entries created, directories the unpack made for entries beneath them included.
    pub entries: u64,
    /// The bytes of the regular files written.
    pub content_bytes: u64,
    /// The bytes the gzip stream inflated to.
    pub inflated_bytes: u64,
    /// The mode the root entry (`.` or `./`) named, if the archive held one.
    pub root_mode: Option<u32>,
}

/// The finished mode of a regular file: its permission bits and the sticky bit, without
/// `setuid`, `setgid` and every write bit (design 38 § 3.3).
pub(super) fn file_mode(mode: u32) -> u32 {
    mode & 0o1555
}

/// The finished mode of a directory: as a file's, and always readable and searchable by the
/// store's owner, which reads and removes the tree.
pub(super) fn directory_mode(mode: u32) -> u32 {
    (mode & 0o1555) | 0o500
}

#[derive(Debug, Clone, Copy)]
struct Directory {
    named: bool,
    mode: u32,
}

/// A path after the leading `./` and trailing `/` are dropped.
enum Normal {
    Root,
    Path(Vec<u8>),
}

struct Unpacker<'a> {
    root: BorrowedFd<'a>,
    limits: ImageLimits,
    entries: u64,
    content_bytes: u64,
    directories: HashMap<Vec<u8>, Directory>,
    root_mode: Option<u32>,
    root_named: bool,
}

/// Unpacks the tar `archive` beneath `root`, an empty directory the caller made and removes on
/// any error, and gives the stream back after the end of the archive.
pub(super) fn unpack<R: Read>(
    root: BorrowedFd<'_>,
    archive: R,
    limits: ImageLimits,
) -> Result<(Unpacked, R), ImageError> {
    let mut unpacker = Unpacker {
        root,
        limits,
        entries: 0,
        content_bytes: 0,
        directories: HashMap::new(),
        root_mode: None,
        root_named: false,
    };
    let mut reader = TarReader::new(archive, limits.max_unpacked_bytes);
    let mut buffer = vec![0_u8; COPY_BUFFER_BYTES];
    while let Some(entry) = reader.next_entry()? {
        unpacker.create(&entry, &mut reader, &mut buffer)?;
    }
    unpacker.finish_directories()?;
    let summary = Unpacked {
        entries: unpacker.entries,
        content_bytes: unpacker.content_bytes,
        inflated_bytes: reader.inflated(),
        root_mode: unpacker.root_mode,
    };
    Ok((summary, reader.into_inner()))
}

impl Unpacker<'_> {
    fn create<R: Read>(
        &mut self,
        entry: &Entry,
        reader: &mut TarReader<R>,
        buffer: &mut [u8],
    ) -> Result<(), ImageError> {
        self.count_entry()?;
        let refused = |refusal| ImageError::refused(&entry.path, refusal);
        if let Kind::Refused(kind) = entry.kind {
            return Err(refused(EntryRefusal::Kind(kind)));
        }
        let path = match normalize(&entry.path).map_err(refused)? {
            Normal::Root => {
                if entry.kind != Kind::Directory {
                    return Err(refused(EntryRefusal::RootNotDirectory));
                }
                if self.root_named {
                    return Err(refused(EntryRefusal::Duplicate));
                }
                self.root_named = true;
                self.root_mode = Some(entry.mode);
                return Ok(());
            }
            Normal::Path(path) => path,
        };
        let (parent_path, name) = split_last(&path);
        let name = CString::new(name).expect("a normalized path holds no NUL");
        let parent = self.open_parent(&entry.path, parent_path)?;
        match entry.kind {
            Kind::Regular => self.write_file(&parent, &name, entry, reader, buffer),
            Kind::Directory => self.make_directory(&parent, &path, &name, entry),
            Kind::Symlink => make_symlink(&parent, &name, entry),
            Kind::HardLink => self.make_hard_link(&parent, &name, entry),
            Kind::Refused(_) => unreachable!("refused above"),
        }
    }

    fn count_entry(&mut self) -> Result<(), ImageError> {
        self.entries += 1;
        if self.entries > self.limits.max_entries {
            return Err(ImageError::ArchiveLimit {
                bound: ImageBound::Entries,
                limit: self.limits.max_entries,
            });
        }
        Ok(())
    }

    /// Opens the directory `parent_path` beneath the root one component at a time, creating a
    /// component the archive has not created yet.
    fn open_parent(&mut self, entry: &[u8], parent_path: &[u8]) -> Result<OwnedFd, ImageError> {
        let mut current = openat2_beneath(self.root, c".", DIRECTORY_FLAGS, 0)
            .map_err(|error| ImageError::io("opening the staging root", error))?;
        if parent_path.is_empty() {
            return Ok(current);
        }
        let mut end = 0;
        for component in parent_path.split(|&byte| byte == b'/') {
            end += component.len();
            let name = CString::new(component).expect("a normalized path holds no NUL");
            current = match openat2_beneath(current.as_fd(), &name, DIRECTORY_FLAGS, 0) {
                Ok(next) => next,
                Err(error) => match error.raw_os_error() {
                    Some(libc::ENOENT) => {
                        self.count_entry()?;
                        nix::sys::stat::mkdirat(
                            current.as_fd(),
                            name.as_c_str(),
                            Mode::from_bits_truncate(WORKING_DIRECTORY_MODE),
                        )
                        .map_err(|errno| ImageError::io("creating a directory", errno.into()))?;
                        self.directories.insert(
                            parent_path[..end].to_vec(),
                            Directory {
                                named: false,
                                mode: IMPLIED_DIRECTORY_MODE,
                            },
                        );
                        openat2_beneath(current.as_fd(), &name, DIRECTORY_FLAGS, 0)
                            .map_err(|error| ImageError::io("opening a directory", error))?
                    }
                    Some(libc::ELOOP) => {
                        return Err(ImageError::refused(entry, EntryRefusal::BeneathSymlink));
                    }
                    // `O_DIRECTORY | O_NOFOLLOW` answers `ENOTDIR` for a symbolic link too.
                    Some(libc::ENOTDIR) => {
                        let link = nix::sys::stat::fstatat(
                            current.as_fd(),
                            name.as_c_str(),
                            AtFlags::AT_SYMLINK_NOFOLLOW,
                        )
                        .is_ok_and(|stat| {
                            SFlag::from_bits_truncate(stat.st_mode) & SFlag::S_IFMT
                                == SFlag::S_IFLNK
                        });
                        let refusal = if link {
                            EntryRefusal::BeneathSymlink
                        } else {
                            EntryRefusal::BeneathFile
                        };
                        return Err(ImageError::refused(entry, refusal));
                    }
                    _ => return Err(ImageError::io("opening a directory", error)),
                },
            };
            end += 1;
        }
        Ok(current)
    }

    fn write_file<R: Read>(
        &mut self,
        parent: &OwnedFd,
        name: &CStr,
        entry: &Entry,
        reader: &mut TarReader<R>,
        buffer: &mut [u8],
    ) -> Result<(), ImageError> {
        let file = openat2_beneath(parent.as_fd(), name, CREATE_FLAGS, 0o600).map_err(|error| {
            if error.raw_os_error() == Some(libc::EEXIST) {
                ImageError::refused(&entry.path, EntryRefusal::Duplicate)
            } else {
                ImageError::io("creating a file", error)
            }
        })?;
        let mut file = File::from(file);
        loop {
            let read = reader.read_data(buffer)?;
            if read == 0 {
                break;
            }
            file.write_all(&buffer[..read])
                .map_err(|error| ImageError::io("writing a file", error))?;
            self.content_bytes += read as u64;
        }
        nix::sys::stat::fchmod(&file, Mode::from_bits_truncate(file_mode(entry.mode)))
            .map_err(|errno| ImageError::io("setting a file's mode", errno.into()))
    }

    fn make_directory(
        &mut self,
        parent: &OwnedFd,
        path: &[u8],
        name: &CStr,
        entry: &Entry,
    ) -> Result<(), ImageError> {
        if let Some(directory) = self.directories.get_mut(path) {
            if directory.named {
                return Err(ImageError::refused(&entry.path, EntryRefusal::Duplicate));
            }
            directory.named = true;
            directory.mode = entry.mode;
            return Ok(());
        }
        nix::sys::stat::mkdirat(
            parent.as_fd(),
            name,
            Mode::from_bits_truncate(WORKING_DIRECTORY_MODE),
        )
        .map_err(|errno| exists_or_io(errno, entry, "creating a directory"))?;
        self.directories.insert(
            path.to_vec(),
            Directory {
                named: true,
                mode: entry.mode,
            },
        );
        Ok(())
    }

    fn make_hard_link(
        &self,
        parent: &OwnedFd,
        name: &CStr,
        entry: &Entry,
    ) -> Result<(), ImageError> {
        let refused = || ImageError::refused(&entry.path, EntryRefusal::HardLinkTarget);
        let Ok(Normal::Path(target)) = normalize(&entry.link) else {
            return Err(refused());
        };
        let (target_parent, target_name) = split_last(&target);
        let target_parent = if target_parent.is_empty() {
            c".".to_owned()
        } else {
            CString::new(target_parent).expect("a normalized path holds no NUL")
        };
        let directory = openat2_beneath(self.root, &target_parent, DIRECTORY_FLAGS, 0)
            .map_err(|_| refused())?;
        let target_name = OsStr::from_bytes(target_name);
        let stat = nix::sys::stat::fstatat(&directory, target_name, AtFlags::AT_SYMLINK_NOFOLLOW)
            .map_err(|_| refused())?;
        if SFlag::from_bits_truncate(stat.st_mode) & SFlag::S_IFMT != SFlag::S_IFREG {
            return Err(refused());
        }
        nix::unistd::linkat(&directory, target_name, parent, name, AtFlags::empty())
            .map_err(|errno| exists_or_io(errno, entry, "creating a hard link"))
    }

    /// Sets each directory's finished mode, deepest first, so a directory still takes its
    /// entries' changes while they are made. The root's mode is the store's to set, after the
    /// tree is published.
    fn finish_directories(&mut self) -> Result<(), ImageError> {
        let mut directories: Vec<(&Vec<u8>, &Directory)> = self.directories.iter().collect();
        directories.sort_by_key(|(path, _)| std::cmp::Reverse(depth(path)));
        for (path, directory) in directories {
            let path = CString::new(path.as_slice()).expect("a normalized path holds no NUL");
            let opened = openat2_beneath(self.root, &path, DIRECTORY_FLAGS, 0)
                .map_err(|error| ImageError::io("opening a directory", error))?;
            nix::sys::stat::fchmod(
                &opened,
                Mode::from_bits_truncate(directory_mode(directory.mode)),
            )
            .map_err(|errno| ImageError::io("setting a directory's mode", errno.into()))?;
        }
        Ok(())
    }
}

fn make_symlink(parent: &OwnedFd, name: &CStr, entry: &Entry) -> Result<(), ImageError> {
    let refused = |refusal| ImageError::refused(&entry.path, refusal);
    if entry.link.is_empty() {
        return Err(refused(EntryRefusal::LinkTargetEmpty));
    }
    if entry.link.len() > MAX_IMAGE_LINK_TARGET_BYTES {
        return Err(refused(EntryRefusal::LinkTargetTooLong));
    }
    let target = CString::new(entry.link.as_slice()).map_err(|_| refused(EntryRefusal::NulByte))?;
    nix::unistd::symlinkat(target.as_c_str(), parent, name)
        .map_err(|errno| exists_or_io(errno, entry, "creating a symbolic link"))
}

fn exists_or_io(errno: Errno, entry: &Entry, operation: &'static str) -> ImageError {
    if errno == Errno::EEXIST {
        ImageError::refused(&entry.path, EntryRefusal::Duplicate)
    } else {
        ImageError::io(operation, errno.into())
    }
}

fn depth(path: &[u8]) -> usize {
    path.split(|&byte| byte == b'/').count()
}

/// The parent of a normalized path and its last component.
fn split_last(path: &[u8]) -> (&[u8], &[u8]) {
    match path.iter().rposition(|&byte| byte == b'/') {
        Some(at) => (&path[..at], &path[at + 1..]),
        None => (&[], path),
    }
}

/// The path rules of design 38 § 3.3: relative, a leading `./` dropped, no NUL, no `..`, no
/// empty or `.` component, no component over 255 bytes and no path over 4,095 bytes. A trailing
/// `/` is dropped, and `.` alone is the root.
fn normalize(raw: &[u8]) -> Result<Normal, EntryRefusal> {
    if raw.is_empty() {
        return Err(EntryRefusal::EmptyPath);
    }
    if raw.contains(&0) {
        return Err(EntryRefusal::NulByte);
    }
    if raw.len() > MAX_IMAGE_PATH_BYTES {
        return Err(EntryRefusal::PathTooLong);
    }
    if raw[0] == b'/' {
        return Err(EntryRefusal::AbsolutePath);
    }
    let mut path = raw;
    while let Some(rest) = path.strip_prefix(b"./") {
        path = rest;
    }
    while let Some(rest) = path.strip_suffix(b"/") {
        path = rest;
    }
    if path.is_empty() || path == b"." {
        return Ok(Normal::Root);
    }
    for component in path.split(|&byte| byte == b'/') {
        match component {
            b"" => return Err(EntryRefusal::EmptyComponent),
            b"." => return Err(EntryRefusal::CurrentComponent),
            b".." => return Err(EntryRefusal::ParentComponent),
            component if component.len() > MAX_IMAGE_PATH_COMPONENT_BYTES => {
                return Err(EntryRefusal::ComponentTooLong);
            }
            _ => {}
        }
    }
    Ok(Normal::Path(path.to_vec()))
}

#[cfg(test)]
mod tests {
    use super::{EntryRefusal, Normal, directory_mode, file_mode, normalize};

    fn path(raw: &str) -> Result<String, EntryRefusal> {
        normalize(raw.as_bytes()).map(|normal| match normal {
            Normal::Root => ".".to_owned(),
            Normal::Path(path) => String::from_utf8(path).expect("utf-8"),
        })
    }

    #[test]
    fn a_path_is_relative_without_dot_dot_and_loses_a_leading_dot_slash() {
        assert_eq!(path("./etc/os-release"), Ok("etc/os-release".to_owned()));
        assert_eq!(path("././usr/bin/"), Ok("usr/bin".to_owned()));
        assert_eq!(path("./"), Ok(".".to_owned()));
        assert_eq!(path("."), Ok(".".to_owned()));
        assert_eq!(path(""), Err(EntryRefusal::EmptyPath));
        assert_eq!(path("/etc/passwd"), Err(EntryRefusal::AbsolutePath));
        assert_eq!(path("../x"), Err(EntryRefusal::ParentComponent));
        assert_eq!(path("a/../../x"), Err(EntryRefusal::ParentComponent));
        assert_eq!(path("a//b"), Err(EntryRefusal::EmptyComponent));
        assert_eq!(path("a/./b"), Err(EntryRefusal::CurrentComponent));
        assert_eq!(path("a\0b"), Err(EntryRefusal::NulByte));
        assert_eq!(path(&"c".repeat(256)), Err(EntryRefusal::ComponentTooLong));
        assert_eq!(path(&"c".repeat(255)), Ok("c".repeat(255)));
        let long = vec!["d"; 2048].join("/");
        assert_eq!(long.len(), 4095);
        assert!(path(&long).is_ok());
        assert_eq!(path(&format!("{long}x")), Err(EntryRefusal::PathTooLong));
    }

    #[test]
    fn a_mode_loses_setuid_setgid_and_write_bits_and_keeps_the_sticky_bit() {
        assert_eq!(file_mode(0o4755), 0o555);
        assert_eq!(file_mode(0o2711), 0o511);
        assert_eq!(file_mode(0o1777), 0o1555);
        assert_eq!(file_mode(0o644), 0o444);
        assert_eq!(directory_mode(0o1777), 0o1555);
        assert_eq!(directory_mode(0o000), 0o500);
        assert_eq!(directory_mode(0o2755), 0o555);
    }
}
