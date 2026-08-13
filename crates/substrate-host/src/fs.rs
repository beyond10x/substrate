use std::ffi::{CStr, CString, OsStr};
use std::fs::File;
use std::io::{Read as _, Seek as _, SeekFrom, Write as _};
use std::os::fd::{AsRawFd as _, FromRawFd as _, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt as _;
use std::path::Path;

use base64::Engine as _;
use chrono::Utc;
use sha2::{Digest as _, Sha256};
use substrate_wire::{
    Base64Content, Base64Encoding, DirectoryEntry, DirectoryEntryKind, DirectoryKind,
    DirectoryPage, FileAbsence, FileKind, FileMode, FileObservation, FileReadQuery, FileReadResult,
    FileSlice, validate_relative_path,
};
use ulid::Ulid;

use crate::DriverError;

const RESOLVE_NO_XDEV: u64 = 0x01;
const RESOLVE_NO_MAGICLINKS: u64 = 0x02;
const RESOLVE_NO_SYMLINKS: u64 = 0x04;
const RESOLVE_BENEATH: u64 = 0x08;
const GUARDED_RESOLVE: u64 =
    RESOLVE_BENEATH | RESOLVE_NO_MAGICLINKS | RESOLVE_NO_SYMLINKS | RESOLVE_NO_XDEV;
const MAX_DIRECTORY_SCAN_ITEMS: usize = 100_000;
const MAX_DESTROY_ITEMS: usize = 100_000;

#[repr(C)]
struct OpenHow {
    flags: u64,
    mode: u64,
    resolve: u64,
}

pub struct GuardedFilesystem {
    root: OwnedFd,
    max_file_bytes: u64,
    read_limit_bytes: u64,
    list_limit_items: u32,
    openat2: bool,
}

impl GuardedFilesystem {
    pub fn open(
        path: &Path,
        max_file_bytes: u64,
        read_limit_bytes: u64,
        list_limit_items: u32,
    ) -> Result<Self, DriverError> {
        let path = cstring(path.as_os_str())?;
        // SAFETY: path is a valid NUL-terminated string and flags require no variadic mode.
        let fd = unsafe {
            libc::open(
                path.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            )
        };
        let root = owned_fd(fd, "workspace root")?;
        let openat2 = openat2(
            root.as_raw_fd(),
            ".",
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
            0,
        )
        .is_ok();
        Ok(Self {
            root,
            max_file_bytes,
            read_limit_bytes,
            list_limit_items,
            openat2,
        })
    }

    pub const fn openat2_available(&self) -> bool {
        self.openat2
    }

    pub fn create_workspace(&self, name: &str) -> Result<(), DriverError> {
        validate_root_name(name)?;
        let name = CString::new(name).map_err(|_| path_escape())?;
        // SAFETY: root and name are valid; mode is used by mkdirat.
        let result = unsafe { libc::mkdirat(self.root.as_raw_fd(), name.as_ptr(), 0o700) };
        if result != 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::EEXIST) {
                return Err(DriverError {
                    class: crate::DriverErrorClass::Conflict,
                    code: "workspace.already-exists",
                    message: "Workspace identity already exists.".to_owned(),
                    address: Some("workspace".to_owned()),
                    retriable: false,
                });
            }
            return Err(io_failed("workspace.create-failed", error));
        }
        sync_fd(self.root.as_raw_fd())?;
        self.observe_workspace(name.to_str().map_err(|_| path_escape())?)
    }

    pub fn observe_workspace(&self, name: &str) -> Result<(), DriverError> {
        let fd = self.workspace_fd(name)?;
        let metadata = fstat(fd.as_raw_fd())?;
        if metadata.st_uid != effective_uid() || metadata.st_mode & libc::S_IFMT != libc::S_IFDIR {
            return Err(path_escape());
        }
        Ok(())
    }

    pub fn read(
        &self,
        workspace_id: &str,
        root_name: &str,
        path: &str,
        query: &FileReadQuery,
    ) -> Result<FileReadResult, DriverError> {
        validate_relative_path(path).map_err(|_| path_escape())?;
        query.validate_shape().map_err(|_| {
            DriverError::refused(
                "request.schema-invalid",
                "File query does not match its selected mode.",
                "query",
            )
        })?;
        let workspace = self.workspace_fd(root_name)?;
        match query.mode {
            FileMode::File => {
                let limit = query.limit_bytes.expect("shape validated");
                if limit > self.read_limit_bytes {
                    return Err(DriverError::exhausted(
                        "workspace.read-limit",
                        "Requested read exceeds the probed limit.",
                        "limit",
                    ));
                }
                let offset = query.offset.expect("shape validated");
                let fd = openat2(
                    workspace.as_raw_fd(),
                    path,
                    libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
                    0,
                )?;
                let stat = fstat(fd.as_raw_fd())?;
                if stat.st_mode & libc::S_IFMT != libc::S_IFREG {
                    return Err(path_escape());
                }
                let mut file = File::from(fd);
                file.seek(SeekFrom::Start(offset))
                    .map_err(|error| io_failed("workspace.read-failed", error))?;
                let allocation = usize::try_from(limit).map_err(|_| {
                    DriverError::exhausted(
                        "workspace.read-limit",
                        "Requested read exceeds platform limits.",
                        "limit",
                    )
                })?;
                let mut bytes = Vec::with_capacity(allocation);
                file.take(limit)
                    .read_to_end(&mut bytes)
                    .map_err(|error| io_failed("workspace.read-failed", error))?;
                let returned_bytes = u64::try_from(bytes.len()).expect("usize fits u64");
                let next_offset = offset.saturating_add(returned_bytes);
                let size = u64::try_from(stat.st_size).unwrap_or(0);
                Ok(FileReadResult::File(FileSlice {
                    kind: FileKind::File,
                    workspace: workspace_id.to_owned(),
                    path: path.to_owned(),
                    offset,
                    returned_bytes,
                    next_offset,
                    eof: next_offset >= size,
                    content: Base64Content {
                        encoding: Base64Encoding::Base64,
                        data: base64::engine::general_purpose::STANDARD.encode(bytes),
                    },
                    observed_at: Utc::now(),
                }))
            }
            FileMode::Directory => {
                let limit = query.limit_items.expect("shape validated");
                if limit > self.list_limit_items {
                    return Err(DriverError::exhausted(
                        "workspace.list-limit",
                        "Requested page exceeds the probed item limit.",
                        "limit",
                    ));
                }
                let fd = openat2(
                    workspace.as_raw_fd(),
                    path,
                    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
                    0,
                )?;
                let mut items = list_directory(fd.as_raw_fd())?;
                items.sort_by(|left, right| left.name.as_bytes().cmp(right.name.as_bytes()));
                let start = decode_cursor(query.cursor.as_deref())?;
                if start > items.len() {
                    return Err(DriverError::refused(
                        "workspace.cursor-invalid",
                        "Directory cursor is outside the observed page set.",
                        "cursor",
                    ));
                }
                let limit = usize::try_from(limit).expect("u32 fits usize");
                let end = start.saturating_add(limit).min(items.len());
                let next_cursor = (end < items.len()).then(|| encode_cursor(end));
                Ok(FileReadResult::Directory(DirectoryPage {
                    kind: DirectoryKind::Directory,
                    workspace: workspace_id.to_owned(),
                    path: path.to_owned(),
                    items: items[start..end].to_vec(),
                    next_cursor,
                    observed_at: Utc::now(),
                }))
            }
        }
    }

    pub fn write_atomic(
        &self,
        workspace_id: &str,
        root_name: &str,
        path: &str,
        content: &[u8],
    ) -> Result<FileObservation, DriverError> {
        validate_relative_path(path).map_err(|_| path_escape())?;
        if u64::try_from(content.len()).expect("usize fits u64") > self.max_file_bytes {
            return Err(DriverError::exhausted(
                "workspace.write-limit",
                "Requested replacement exceeds the probed limit.",
                "limit",
            ));
        }
        let workspace = self.workspace_fd(root_name)?;
        let (parent, name) = split_parent(path)?;
        let parent_fd = if parent == "." {
            duplicate(workspace.as_raw_fd())?
        } else {
            openat2(
                workspace.as_raw_fd(),
                parent,
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
                0,
            )?
        };
        refuse_unsafe_existing(parent_fd.as_raw_fd(), name)?;
        let temporary_name = format!(".substrate-{}.tmp", Ulid::new());
        let temporary = CString::new(temporary_name.as_str()).expect("ULID has no NUL");
        // SAFETY: parent fd and temporary name are valid; O_EXCL prevents aliasing an existing path.
        let fd = unsafe {
            libc::openat(
                parent_fd.as_raw_fd(),
                temporary.as_ptr(),
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_CLOEXEC | libc::O_NOFOLLOW,
                0o600,
            )
        };
        let temporary_fd = owned_fd(fd, "atomic temporary file")?;
        let mut file = File::from(temporary_fd);
        if let Err(error) = file.write_all(content).and_then(|()| file.sync_all()) {
            unlink_name(parent_fd.as_raw_fd(), &temporary_name);
            return Err(io_failed("workspace.write-failed", error));
        }
        drop(file);
        let destination = CString::new(name).map_err(|_| path_escape())?;
        // SAFETY: both names are relative to the same guarded parent descriptor.
        let renamed = unsafe {
            libc::renameat(
                parent_fd.as_raw_fd(),
                temporary.as_ptr(),
                parent_fd.as_raw_fd(),
                destination.as_ptr(),
            )
        };
        if renamed != 0 {
            let error = std::io::Error::last_os_error();
            unlink_name(parent_fd.as_raw_fd(), &temporary_name);
            return Err(io_failed("workspace.write-failed", error));
        }
        sync_fd(parent_fd.as_raw_fd())?;
        let observed = openat2(
            workspace.as_raw_fd(),
            path,
            libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            0,
        )?;
        let stat = fstat(observed.as_raw_fd())?;
        if stat.st_mode & libc::S_IFMT != libc::S_IFREG {
            return Err(path_escape());
        }
        let mut actual = Vec::new();
        File::from(observed)
            .read_to_end(&mut actual)
            .map_err(|error| io_failed("workspace.observe-failed", error))?;
        Ok(FileObservation {
            kind: FileKind::File,
            workspace: workspace_id.to_owned(),
            path: path.to_owned(),
            size: u64::try_from(actual.len()).expect("usize fits u64"),
            sha256: hex::encode(Sha256::digest(actual)),
            atomic_replacement: true,
            observed_at: Utc::now(),
        })
    }

    pub fn delete_file(
        &self,
        workspace_id: &str,
        root_name: &str,
        path: &str,
    ) -> Result<FileAbsence, DriverError> {
        validate_relative_path(path).map_err(|_| path_escape())?;
        let workspace = self.workspace_fd(root_name)?;
        let (parent, name) = split_parent(path)?;
        let parent_fd = if parent == "." {
            duplicate(workspace.as_raw_fd())?
        } else {
            openat2(
                workspace.as_raw_fd(),
                parent,
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
                0,
            )?
        };
        let target = openat2(
            parent_fd.as_raw_fd(),
            name,
            libc::O_PATH | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            0,
        )?;
        let stat = fstat(target.as_raw_fd())?;
        if stat.st_mode & libc::S_IFMT != libc::S_IFREG {
            return Err(path_escape());
        }
        drop(target);
        let name = CString::new(name).map_err(|_| path_escape())?;
        // SAFETY: target was opened and verified relative to the guarded parent.
        if unsafe { libc::unlinkat(parent_fd.as_raw_fd(), name.as_ptr(), 0) } != 0 {
            return Err(io_failed(
                "workspace.delete-failed",
                std::io::Error::last_os_error(),
            ));
        }
        sync_fd(parent_fd.as_raw_fd())?;
        match openat2(
            parent_fd.as_raw_fd(),
            name.to_str().map_err(|_| path_escape())?,
            libc::O_PATH | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            0,
        ) {
            Err(error) if error.code == "resource.not-found" => {}
            Ok(_) => {
                return Err(io_failed(
                    "workspace.observe-failed",
                    std::io::Error::other("deleted target still exists"),
                ));
            }
            Err(error) => return Err(error),
        }
        Ok(FileAbsence {
            kind: FileKind::File,
            workspace: workspace_id.to_owned(),
            path: path.to_owned(),
            absent: true,
            observed_at: Utc::now(),
        })
    }

    pub fn destroy_workspace(&self, root_name: &str) -> Result<(), DriverError> {
        let workspace = self.workspace_fd(root_name)?;
        let mut remaining = MAX_DESTROY_ITEMS;
        remove_children(workspace.as_raw_fd(), 0, &mut remaining)?;
        drop(workspace);
        let name = CString::new(root_name).map_err(|_| path_escape())?;
        // SAFETY: name is a validated direct child; children were removed descriptor-relatively.
        if unsafe { libc::unlinkat(self.root.as_raw_fd(), name.as_ptr(), libc::AT_REMOVEDIR) } != 0
        {
            return Err(io_failed(
                "workspace.destroy-failed",
                std::io::Error::last_os_error(),
            ));
        }
        sync_fd(self.root.as_raw_fd())?;
        match self.workspace_fd(root_name) {
            Err(error) if error.code == "resource.not-found" => Ok(()),
            Ok(_) => Err(io_failed(
                "workspace.observe-failed",
                std::io::Error::other("workspace still exists"),
            )),
            Err(error) => Err(error),
        }
    }

    fn workspace_fd(&self, name: &str) -> Result<OwnedFd, DriverError> {
        validate_root_name(name)?;
        openat2(
            self.root.as_raw_fd(),
            name,
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            0,
        )
    }
}

fn openat2(
    directory: RawFd,
    path: &str,
    flags: i32,
    mode: libc::mode_t,
) -> Result<OwnedFd, DriverError> {
    let path = CString::new(path).map_err(|_| path_escape())?;
    let how = OpenHow {
        flags: u64::try_from(flags).expect("open flags are non-negative"),
        mode: u64::from(mode),
        resolve: GUARDED_RESOLVE,
    };
    // SAFETY: syscall arguments point to initialized OpenHow and a NUL-terminated path.
    let fd = unsafe {
        libc::syscall(
            libc::SYS_openat2,
            directory,
            path.as_ptr(),
            &how,
            size_of::<OpenHow>(),
        )
    };
    if fd < 0 {
        return Err(classify_open_error(std::io::Error::last_os_error()));
    }
    owned_fd(
        i32::try_from(fd).expect("file descriptor fits i32"),
        "openat2",
    )
}

fn list_directory(fd: RawFd) -> Result<Vec<DirectoryEntry>, DriverError> {
    let duplicate = duplicate(fd)?;
    let raw = duplicate.as_raw_fd();
    std::mem::forget(duplicate);
    // SAFETY: ownership of the duplicated descriptor transfers to DIR*.
    let directory = unsafe { libc::fdopendir(raw) };
    if directory.is_null() {
        // SAFETY: fdopendir did not acquire the duplicated fd on failure.
        unsafe { libc::close(raw) };
        return Err(io_failed(
            "workspace.list-failed",
            std::io::Error::last_os_error(),
        ));
    }
    let mut result = Vec::new();
    loop {
        // SAFETY: directory remains live until closed below.
        let entry = unsafe { libc::readdir(directory) };
        if entry.is_null() {
            break;
        }
        // SAFETY: readdir returns a dirent with a NUL-terminated d_name.
        let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) };
        if name.to_bytes() == b"." || name.to_bytes() == b".." {
            continue;
        }
        let name_text = String::from_utf8(name.to_bytes().to_vec()).map_err(|_| path_escape())?;
        let metadata = fstatat(fd, name)?;
        let file_type = metadata.st_mode & libc::S_IFMT;
        let (kind, size) = match file_type {
            libc::S_IFREG => (
                DirectoryEntryKind::File,
                Some(u64::try_from(metadata.st_size).unwrap_or(0)),
            ),
            libc::S_IFDIR => (DirectoryEntryKind::Directory, None),
            libc::S_IFLNK => (DirectoryEntryKind::Symlink, None),
            _ => {
                // SAFETY: directory is live and closed exactly once on this error path.
                unsafe { libc::closedir(directory) };
                return Err(path_escape());
            }
        };
        result.push(DirectoryEntry {
            name: name_text,
            kind,
            size,
        });
        if result.len() > MAX_DIRECTORY_SCAN_ITEMS {
            // SAFETY: directory is live and closed exactly once on this error path.
            unsafe { libc::closedir(directory) };
            return Err(DriverError::exhausted(
                "workspace.directory-scan-limit",
                "Directory exceeds the bounded observation limit.",
                "directory",
            ));
        }
    }
    // SAFETY: directory was obtained by fdopendir and is closed exactly once.
    if unsafe { libc::closedir(directory) } != 0 {
        return Err(io_failed(
            "workspace.list-failed",
            std::io::Error::last_os_error(),
        ));
    }
    Ok(result)
}

fn remove_children(fd: RawFd, depth: usize, remaining: &mut usize) -> Result<(), DriverError> {
    if depth > substrate_wire::MAX_PATH_DEPTH {
        return Err(path_escape());
    }
    let entries = list_directory(fd)?;
    if entries.len() > *remaining {
        return Err(DriverError::exhausted(
            "workspace.destroy-limit",
            "Workspace exceeds the bounded destroy traversal limit.",
            "workspace",
        ));
    }
    *remaining -= entries.len();
    for entry in entries {
        let name = CString::new(entry.name.as_str()).map_err(|_| path_escape())?;
        match entry.kind {
            DirectoryEntryKind::File | DirectoryEntryKind::Symlink => {
                // Symlinks are removed as objects and never followed.
                // SAFETY: name came from this directory's own readdir result.
                if unsafe { libc::unlinkat(fd, name.as_ptr(), 0) } != 0 {
                    return Err(io_failed(
                        "workspace.destroy-failed",
                        std::io::Error::last_os_error(),
                    ));
                }
            }
            DirectoryEntryKind::Directory => {
                let child = openat2(
                    fd,
                    &entry.name,
                    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
                    0,
                )?;
                remove_children(child.as_raw_fd(), depth + 1, remaining)?;
                drop(child);
                // SAFETY: child was opened beneath fd with NO_XDEV/NO_SYMLINKS.
                if unsafe { libc::unlinkat(fd, name.as_ptr(), libc::AT_REMOVEDIR) } != 0 {
                    return Err(io_failed(
                        "workspace.destroy-failed",
                        std::io::Error::last_os_error(),
                    ));
                }
            }
        }
    }
    sync_fd(fd)
}

fn refuse_unsafe_existing(parent: RawFd, name: &str) -> Result<(), DriverError> {
    match openat2(
        parent,
        name,
        libc::O_PATH | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        0,
    ) {
        Ok(fd) => {
            let stat = fstat(fd.as_raw_fd())?;
            if stat.st_mode & libc::S_IFMT != libc::S_IFREG {
                return Err(path_escape());
            }
            Ok(())
        }
        Err(error) if error.code == "resource.not-found" => Ok(()),
        Err(error) => Err(error),
    }
}

fn split_parent(path: &str) -> Result<(&str, &str), DriverError> {
    match path.rsplit_once('/') {
        Some((parent, name)) if !parent.is_empty() && !name.is_empty() => Ok((parent, name)),
        None if !path.is_empty() => Ok((".", path)),
        _ => Err(path_escape()),
    }
}

fn validate_root_name(name: &str) -> Result<(), DriverError> {
    if name.starts_with("ws_")
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        Ok(())
    } else {
        Err(path_escape())
    }
}

fn classify_open_error(error: std::io::Error) -> DriverError {
    match error.raw_os_error() {
        Some(libc::ENOENT) => DriverError::not_found(),
        Some(libc::EXDEV | libc::ELOOP | libc::ENOTDIR) => path_escape(),
        Some(libc::ENOSYS) => DriverError::unserved(
            "workspace.openat2-unavailable",
            "The kernel does not serve guarded workspace I/O.",
            "workspace.openat2-beneath",
        ),
        _ => io_failed("workspace.io-failed", error),
    }
}

fn path_escape() -> DriverError {
    DriverError::refused(
        "workspace.path-escape",
        "Workspace path is outside the confined root.",
        "path",
    )
}

#[allow(clippy::needless_pass_by_value)] // Call sites hand ownership out of map_err closures.
fn io_failed(code: &'static str, error: std::io::Error) -> DriverError {
    DriverError::failed(code, format!("Guarded workspace operation failed: {error}"))
}

fn cstring(value: &OsStr) -> Result<CString, DriverError> {
    CString::new(value.as_bytes()).map_err(|_| path_escape())
}

fn owned_fd(fd: RawFd, _context: &str) -> Result<OwnedFd, DriverError> {
    if fd < 0 {
        return Err(io_failed(
            "workspace.io-failed",
            std::io::Error::last_os_error(),
        ));
    }
    // SAFETY: a successful open/dup returns a new descriptor owned by the caller.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

fn duplicate(fd: RawFd) -> Result<OwnedFd, DriverError> {
    // SAFETY: fcntl duplicates a live descriptor and returns a new owned descriptor.
    owned_fd(
        unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 3) },
        "duplicate",
    )
}

fn fstat(fd: RawFd) -> Result<libc::stat, DriverError> {
    // SAFETY: zeroed stat is valid output storage for fstat.
    let mut stat = unsafe { std::mem::zeroed::<libc::stat>() };
    // SAFETY: fd is live and stat points to writable storage.
    if unsafe { libc::fstat(fd, &raw mut stat) } != 0 {
        return Err(io_failed(
            "workspace.observe-failed",
            std::io::Error::last_os_error(),
        ));
    }
    Ok(stat)
}

fn fstatat(fd: RawFd, name: &CStr) -> Result<libc::stat, DriverError> {
    // SAFETY: zeroed stat is valid output storage for fstatat.
    let mut stat = unsafe { std::mem::zeroed::<libc::stat>() };
    // SAFETY: name belongs to fd and stat points to writable storage.
    if unsafe { libc::fstatat(fd, name.as_ptr(), &raw mut stat, libc::AT_SYMLINK_NOFOLLOW) } != 0 {
        return Err(io_failed(
            "workspace.observe-failed",
            std::io::Error::last_os_error(),
        ));
    }
    Ok(stat)
}

fn sync_fd(fd: RawFd) -> Result<(), DriverError> {
    // SAFETY: fsync only reads descriptor state.
    if unsafe { libc::fsync(fd) } != 0 {
        return Err(io_failed(
            "workspace.sync-failed",
            std::io::Error::last_os_error(),
        ));
    }
    Ok(())
}

fn unlink_name(parent: RawFd, name: &str) {
    if let Ok(name) = CString::new(name) {
        // SAFETY: best-effort cleanup of a name created beneath parent by this operation.
        unsafe { libc::unlinkat(parent, name.as_ptr(), 0) };
    }
}

fn effective_uid() -> libc::uid_t {
    // SAFETY: geteuid takes no arguments and has no preconditions.
    unsafe { libc::geteuid() }
}

fn encode_cursor(index: usize) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(index.to_be_bytes())
}

fn decode_cursor(cursor: Option<&str>) -> Result<usize, DriverError> {
    let Some(cursor) = cursor else {
        return Ok(0);
    };
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(cursor)
        .map_err(|_| {
            DriverError::refused(
                "workspace.cursor-invalid",
                "Directory cursor is invalid.",
                "cursor",
            )
        })?;
    let encoded: [u8; size_of::<usize>()] = bytes.try_into().map_err(|_| {
        DriverError::refused(
            "workspace.cursor-invalid",
            "Directory cursor is invalid.",
            "cursor",
        )
    })?;
    Ok(usize::from_be_bytes(encoded))
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::symlink;

    use substrate_wire::{FileMode, FileReadQuery};
    use tempfile::tempdir;

    use super::GuardedFilesystem;

    #[test]
    fn guarded_io_refuses_escape_and_observes_atomic_content() {
        let directory = tempdir().expect("tempdir");
        let outside = tempdir().expect("outside");
        std::fs::write(outside.path().join("secret"), "outside").expect("outside sentinel");
        let filesystem =
            GuardedFilesystem::open(directory.path(), 1024, 1024, 100).expect("guarded filesystem");
        if !filesystem.openat2_available() {
            return;
        }
        filesystem.create_workspace("ws_test").expect("workspace");
        std::fs::create_dir(directory.path().join("ws_test/src")).expect("src");
        let observed = filesystem
            .write_atomic("ws_test", "ws_test", "src/main.txt", b"hello")
            .expect("atomic write");
        assert!(observed.atomic_replacement);
        let result = filesystem
            .read(
                "ws_test",
                "ws_test",
                "src/main.txt",
                &FileReadQuery {
                    mode: FileMode::File,
                    offset: Some(0),
                    limit_bytes: Some(16),
                    cursor: None,
                    limit_items: None,
                },
            )
            .expect("read");
        assert!(matches!(result, substrate_wire::FileReadResult::File(_)));
        symlink(outside.path(), directory.path().join("ws_test/link")).expect("symlink");
        assert!(
            filesystem
                .read(
                    "ws_test",
                    "ws_test",
                    "link/secret",
                    &FileReadQuery {
                        mode: FileMode::File,
                        offset: Some(0),
                        limit_bytes: Some(16),
                        cursor: None,
                        limit_items: None,
                    }
                )
                .is_err()
        );
        assert_eq!(
            std::fs::read_to_string(outside.path().join("secret")).expect("outside unchanged"),
            "outside"
        );
    }
}
