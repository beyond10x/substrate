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
    Base64Content, Base64Encoding, DigestedFileSlice, DirectoryEntry, DirectoryEntryKind,
    DirectoryKind, DirectoryPage, ExpectedFileState, FileAbsence, FileEditInput, FileKind,
    FileMode, FileMutationResult, FileObservation, FilePatchInput, FileReadQuery, FileReadResult,
    FileReplaceInput, FileSlice, LinePatchEdit, TextMatchPolicy, UnifiedDiff, WorkspaceTree,
    WorkspaceTreeEntry, WorkspaceTreeQuery, validate_relative_path,
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
const MAX_DIFF_LINES: usize = 200;
const MAX_PATCH_EDITS: usize = 128;
pub(super) const DESTROY_BATCH_ITEMS: usize = 4_096;
type RawDirectoryEntry = (CString, libc::mode_t, Option<u64>);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum WorkspaceDestroyBatch {
    Pending { removed_items: u64 },
    Absent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RemovalBatch {
    complete: bool,
    processed_items: usize,
    removed_items: usize,
}

struct RemovalCounters {
    remaining: usize,
    processed: usize,
    removed: usize,
}

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

    pub(super) fn root_identity(&self) -> Result<(libc::dev_t, libc::ino_t), DriverError> {
        let metadata = fstat(self.root.as_raw_fd())?;
        Ok((metadata.st_dev, metadata.st_ino))
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
        validate_api_path(path)?;
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

    pub fn read_digested(
        &self,
        workspace_id: &str,
        root_name: &str,
        path: &str,
        query: &FileReadQuery,
    ) -> Result<DigestedFileSlice, DriverError> {
        validate_api_path(path)?;
        query.validate_shape().map_err(|_| {
            DriverError::refused(
                "request.schema-invalid",
                "File query does not match file mode.",
                "query",
            )
        })?;
        if query.mode != FileMode::File {
            return Err(DriverError::refused(
                "request.schema-invalid",
                "Digest reads require file mode.",
                "query",
            ));
        }
        let limit = query.limit_bytes.expect("shape validated");
        if limit > self.read_limit_bytes {
            return Err(DriverError::exhausted(
                "workspace.read-limit",
                "Requested read exceeds the probed limit.",
                "limit",
            ));
        }
        let workspace = self.workspace_fd(root_name)?;
        let bytes = self.read_complete_from(workspace.as_raw_fd(), path)?;
        let offset = query.offset.expect("shape validated");
        let start = usize::try_from(offset)
            .unwrap_or(usize::MAX)
            .min(bytes.len());
        let limit = usize::try_from(limit).unwrap_or(usize::MAX);
        let end = start.saturating_add(limit).min(bytes.len());
        let slice = &bytes[start..end];
        let returned_bytes = u64::try_from(slice.len()).expect("usize fits u64");
        let next_offset = offset.saturating_add(returned_bytes);
        Ok(DigestedFileSlice {
            kind: FileKind::File,
            workspace: workspace_id.to_owned(),
            path: path.to_owned(),
            size: u64::try_from(bytes.len()).expect("usize fits u64"),
            sha256: sha256(&bytes),
            offset,
            returned_bytes,
            next_offset,
            eof: end >= bytes.len(),
            content: Base64Content {
                encoding: Base64Encoding::Base64,
                data: base64::engine::general_purpose::STANDARD.encode(slice),
            },
            observed_at: Utc::now(),
        })
    }

    pub fn list_tree(
        &self,
        workspace_id: &str,
        root_name: &str,
        query: &WorkspaceTreeQuery,
    ) -> Result<WorkspaceTree, DriverError> {
        query.validate().map_err(|_| {
            DriverError::refused(
                "request.schema-invalid",
                "Workspace tree query is outside the admitted bound.",
                "query",
            )
        })?;
        if query.limit_items > self.list_limit_items {
            return Err(DriverError::exhausted(
                "workspace.list-limit",
                "Requested tree exceeds the probed item limit.",
                "limit",
            ));
        }
        let workspace = self.workspace_fd(root_name)?;
        let mut items = Vec::with_capacity(
            usize::try_from(query.limit_items).expect("u32 tree limit fits usize"),
        );
        let mut truncated = false;
        walk_tree(
            workspace.as_raw_fd(),
            "",
            query.include_hidden,
            usize::try_from(query.limit_items).expect("u32 tree limit fits usize"),
            &mut items,
            &mut truncated,
        )?;
        Ok(WorkspaceTree {
            workspace: workspace_id.to_owned(),
            items,
            truncated,
            observed_at: Utc::now(),
        })
    }

    pub fn replace_cas(
        &self,
        workspace_id: &str,
        root_name: &str,
        path: &str,
        input: &FileReplaceInput,
    ) -> Result<FileMutationResult, DriverError> {
        input.expected.validate().map_err(|_| invalid_digest())?;
        let content = canonical_content(&input.content)?;
        if input.create_parents {
            self.create_parent_directories(root_name, path)?;
        }
        let before = self.current_file(root_name, path)?;
        verify_expected(&input.expected, before.as_deref())?;
        let before_sha256 = before.as_deref().map(sha256);
        let observation = self.write_atomic(workspace_id, root_name, path, &content)?;
        Ok(mutation_result(
            workspace_id,
            path,
            before.as_deref(),
            &content,
            before_sha256,
            observation,
        ))
    }

    pub fn edit_cas(
        &self,
        workspace_id: &str,
        root_name: &str,
        path: &str,
        input: &FileEditInput,
    ) -> Result<FileMutationResult, DriverError> {
        if !is_sha256(&input.expected_sha256) {
            return Err(invalid_digest());
        }
        if input.old_text.is_empty() {
            return Err(DriverError::refused(
                "workspace.edit-empty-match",
                "Text edit match must not be empty.",
                "old_text",
            ));
        }
        let before = self
            .current_file(root_name, path)?
            .ok_or_else(DriverError::not_found)?;
        verify_sha256(&input.expected_sha256, &before)?;
        let source = std::str::from_utf8(&before).map_err(|_| binary_text_refusal())?;
        let after = apply_text_edit(source, input)?.into_bytes();
        let observation = self.write_atomic(workspace_id, root_name, path, &after)?;
        Ok(mutation_result(
            workspace_id,
            path,
            Some(&before),
            &after,
            Some(input.expected_sha256.clone()),
            observation,
        ))
    }

    pub fn patch_cas(
        &self,
        workspace_id: &str,
        root_name: &str,
        path: &str,
        input: &FilePatchInput,
    ) -> Result<FileMutationResult, DriverError> {
        if !is_sha256(&input.expected_sha256) {
            return Err(invalid_digest());
        }
        if input.edits.is_empty() || input.edits.len() > MAX_PATCH_EDITS {
            return Err(DriverError::refused(
                "workspace.patch-count",
                "Patch must contain between one and 128 edits.",
                "edits",
            ));
        }
        let before = self
            .current_file(root_name, path)?
            .ok_or_else(DriverError::not_found)?;
        verify_sha256(&input.expected_sha256, &before)?;
        let source = std::str::from_utf8(&before).map_err(|_| binary_text_refusal())?;
        let after = apply_line_patch(source, &input.edits)?.into_bytes();
        let observation = self.write_atomic(workspace_id, root_name, path, &after)?;
        Ok(mutation_result(
            workspace_id,
            path,
            Some(&before),
            &after,
            Some(input.expected_sha256.clone()),
            observation,
        ))
    }

    fn current_file(&self, root_name: &str, path: &str) -> Result<Option<Vec<u8>>, DriverError> {
        validate_api_path(path)?;
        let workspace = self.workspace_fd(root_name)?;
        match self.read_complete_from(workspace.as_raw_fd(), path) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(error) if error.class == crate::DriverErrorClass::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }

    fn read_complete_from(&self, workspace: RawFd, path: &str) -> Result<Vec<u8>, DriverError> {
        let fd = openat2(
            workspace,
            path,
            libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            0,
        )?;
        let stat = fstat(fd.as_raw_fd())?;
        if stat.st_mode & libc::S_IFMT != libc::S_IFREG {
            return Err(path_escape());
        }
        let size = u64::try_from(stat.st_size).unwrap_or(u64::MAX);
        if size > self.max_file_bytes {
            return Err(DriverError::exhausted(
                "workspace.file-limit",
                "File exceeds the probed complete-file limit.",
                "file",
            ));
        }
        read_bounded_complete(File::from(fd), size, self.max_file_bytes)
    }

    fn create_parent_directories(&self, root_name: &str, path: &str) -> Result<(), DriverError> {
        validate_api_path(path)?;
        let (parent, _) = split_parent(path)?;
        if parent == "." {
            return Ok(());
        }
        let workspace = self.workspace_fd(root_name)?;
        let mut current = duplicate(workspace.as_raw_fd())?;
        for component in parent.split('/') {
            let name = CString::new(component).map_err(|_| path_escape())?;
            match openat2_cstr(
                current.as_raw_fd(),
                &name,
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
                0,
            ) {
                Ok(next) => current = next,
                Err(error) if error.class == crate::DriverErrorClass::NotFound => {
                    // SAFETY: current is guarded and component came from a validated relative path.
                    if unsafe { libc::mkdirat(current.as_raw_fd(), name.as_ptr(), 0o700) } != 0 {
                        return Err(io_failed(
                            "workspace.mkdir-failed",
                            std::io::Error::last_os_error(),
                        ));
                    }
                    sync_fd(current.as_raw_fd())?;
                    current = openat2_cstr(
                        current.as_raw_fd(),
                        &name,
                        libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
                        0,
                    )?;
                }
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }

    pub fn write_atomic(
        &self,
        workspace_id: &str,
        root_name: &str,
        path: &str,
        content: &[u8],
    ) -> Result<FileObservation, DriverError> {
        validate_api_path(path)?;
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
        let executable = existing_file_is_executable(parent_fd.as_raw_fd(), name)?;
        let temporary_name = format!(".substrate-{}.tmp", Ulid::generate());
        let temporary = CString::new(temporary_name.as_str()).expect("ULID has no NUL");
        // SAFETY: parent fd and temporary name are valid; O_EXCL prevents aliasing an existing path.
        let fd = unsafe {
            libc::openat(
                parent_fd.as_raw_fd(),
                temporary.as_ptr(),
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_CLOEXEC | libc::O_NOFOLLOW,
                if executable { 0o700 } else { 0o600 },
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
        Ok(FileObservation {
            kind: FileKind::File,
            workspace: workspace_id.to_owned(),
            path: path.to_owned(),
            size: u64::try_from(content.len()).expect("usize fits u64"),
            sha256: hex::encode(Sha256::digest(content)),
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
        validate_api_path(path)?;
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

    pub fn destroy_workspace_batch(
        &self,
        root_name: &str,
    ) -> Result<WorkspaceDestroyBatch, DriverError> {
        let workspace = match self.workspace_fd(root_name) {
            Ok(workspace) => workspace,
            Err(error) if error.class == crate::DriverErrorClass::NotFound => {
                return Ok(WorkspaceDestroyBatch::Absent);
            }
            Err(error) => return Err(error),
        };
        let batch = remove_children_batch(workspace.as_raw_fd(), DESTROY_BATCH_ITEMS)?;
        if !batch.complete {
            if batch.processed_items == 0 {
                return Err(DriverError::failed(
                    "workspace.destroy-no-progress",
                    "Workspace cleanup could not make bounded forward progress.",
                ));
            }
            return Ok(WorkspaceDestroyBatch::Pending {
                removed_items: u64::try_from(batch.removed_items).expect("usize fits u64"),
            });
        }
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
            Err(error) if error.code == "resource.not-found" => Ok(WorkspaceDestroyBatch::Absent),
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

fn read_bounded_complete(
    file: File,
    observed_size: u64,
    max_file_bytes: u64,
) -> Result<Vec<u8>, DriverError> {
    let mut bytes = Vec::with_capacity(usize::try_from(observed_size).unwrap_or(0));
    file.take(max_file_bytes.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|error| io_failed("workspace.read-failed", error))?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > max_file_bytes {
        return Err(DriverError::exhausted(
            "workspace.file-limit",
            "File exceeds the probed complete-file limit.",
            "file",
        ));
    }
    Ok(bytes)
}

fn openat2(
    directory: RawFd,
    path: &str,
    flags: i32,
    mode: libc::mode_t,
) -> Result<OwnedFd, DriverError> {
    let path = CString::new(path).map_err(|_| path_escape())?;
    openat2_cstr(directory, &path, flags, mode)
}

fn openat2_cstr(
    directory: RawFd,
    path: &CStr,
    flags: i32,
    mode: libc::mode_t,
) -> Result<OwnedFd, DriverError> {
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
    let entries = raw_directory_entries(fd)?;
    let mut result = Vec::new();
    for (name, file_type, size) in entries {
        let name_text = String::from_utf8(name.to_bytes().to_vec()).map_err(|_| path_escape())?;
        if name_text == ".git" {
            continue;
        }
        let (kind, size) = match file_type {
            libc::S_IFREG => (DirectoryEntryKind::File, size),
            libc::S_IFDIR => (DirectoryEntryKind::Directory, None),
            libc::S_IFLNK => (DirectoryEntryKind::Symlink, None),
            _ => return Err(path_escape()),
        };
        result.push(DirectoryEntry {
            name: name_text,
            kind,
            size,
        });
    }
    Ok(result)
}

fn walk_tree(
    directory: RawFd,
    prefix: &str,
    include_hidden: bool,
    limit: usize,
    result: &mut Vec<WorkspaceTreeEntry>,
    truncated: &mut bool,
) -> Result<(), DriverError> {
    let mut entries = raw_directory_entries(directory)?;
    entries.sort_by(|left, right| left.0.as_bytes().cmp(right.0.as_bytes()));
    for (name, file_type, size) in entries {
        let name_bytes = name.as_bytes();
        if name_bytes == b".git" {
            continue;
        }
        if !include_hidden && name_bytes.first() == Some(&b'.') {
            continue;
        }
        if result.len() == limit {
            *truncated = true;
            return Ok(());
        }
        let name_text = String::from_utf8(name_bytes.to_vec()).map_err(|_| path_escape())?;
        let path = if prefix.is_empty() {
            name_text
        } else {
            format!("{prefix}/{name_text}")
        };
        let (kind, entry_size) = match file_type {
            libc::S_IFREG => (DirectoryEntryKind::File, size),
            libc::S_IFDIR => (DirectoryEntryKind::Directory, None),
            libc::S_IFLNK => (DirectoryEntryKind::Symlink, None),
            _ => return Err(path_escape()),
        };
        result.push(WorkspaceTreeEntry {
            path: path.clone(),
            kind,
            size: entry_size,
        });
        if file_type == libc::S_IFDIR {
            let child = openat2_cstr(
                directory,
                &name,
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
                0,
            )?;
            walk_tree(
                child.as_raw_fd(),
                &path,
                include_hidden,
                limit,
                result,
                truncated,
            )?;
            if *truncated {
                return Ok(());
            }
        }
    }
    Ok(())
}

fn validate_api_path(path: &str) -> Result<(), DriverError> {
    validate_relative_path(path).map_err(|_| path_escape())?;
    if path.split('/').any(|component| component == ".git") {
        return Err(DriverError::not_found());
    }
    Ok(())
}

fn remove_children_batch(root: RawFd, limit: usize) -> Result<RemovalBatch, DriverError> {
    let root_scan_limit = limit.div_ceil(2);
    let (entries, root_has_more) = raw_directory_entries_bounded(root, root_scan_limit)?;
    let mut counters = RemovalCounters {
        remaining: limit.saturating_sub(entries.len()),
        processed: entries.len(),
        removed: 0,
    };
    for (name, file_type, _) in entries {
        if file_type == libc::S_IFDIR {
            if counters.remaining > 0 {
                flatten_directory_batch(root, &name, &mut counters)?;
            }
        } else {
            unlink_entry(root, &name, 0)?;
            counters.removed += 1;
        }
    }
    sync_fd(root)?;
    let complete = !root_has_more
        && counters.processed < limit
        && raw_directory_entries_bounded(root, 1)?.0.is_empty();
    Ok(RemovalBatch {
        complete,
        processed_items: counters.processed,
        removed_items: counters.removed,
    })
}

fn flatten_directory_batch(
    root: RawFd,
    name: &CStr,
    counters: &mut RemovalCounters,
) -> Result<(), DriverError> {
    let child = openat2_cstr(
        root,
        name,
        libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        0,
    )?;
    let (entries, has_more) = raw_directory_entries_bounded(child.as_raw_fd(), counters.remaining)?;
    counters.remaining = counters.remaining.saturating_sub(entries.len());
    counters.processed += entries.len();
    for (entry_name, file_type, _) in entries {
        if file_type == libc::S_IFDIR {
            let flattened = CString::new(format!(".substrate-gc-{}", Ulid::generate()))
                .expect("generated cleanup identity has no NUL");
            // SAFETY: both directory fds are owned and both names are NUL-terminated.
            if unsafe {
                libc::renameat(
                    child.as_raw_fd(),
                    entry_name.as_ptr(),
                    root,
                    flattened.as_ptr(),
                )
            } != 0
            {
                return Err(io_failed(
                    "workspace.destroy-failed",
                    std::io::Error::last_os_error(),
                ));
            }
        } else {
            unlink_entry(child.as_raw_fd(), &entry_name, 0)?;
            counters.removed += 1;
        }
    }
    sync_fd(child.as_raw_fd())?;
    drop(child);
    if !has_more {
        match unlink_entry(root, name, libc::AT_REMOVEDIR) {
            Ok(()) => {
                counters.removed += 1;
            }
            Err(error) if error.code == "workspace.destroy-not-empty" => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn unlink_entry(directory: RawFd, name: &CStr, flags: i32) -> Result<(), DriverError> {
    // SAFETY: name came from this directory's own readdir result or a validated direct child.
    if unsafe { libc::unlinkat(directory, name.as_ptr(), flags) } == 0 {
        return Ok(());
    }
    let error = std::io::Error::last_os_error();
    if flags == libc::AT_REMOVEDIR && error.raw_os_error() == Some(libc::ENOTEMPTY) {
        return Err(DriverError::failed(
            "workspace.destroy-not-empty",
            "Workspace directory still contains entries.",
        ));
    }
    Err(io_failed("workspace.destroy-failed", error))
}

struct DirectoryStream(*mut libc::DIR);

impl Drop for DirectoryStream {
    fn drop(&mut self) {
        // SAFETY: the stream is created by fdopendir and owned by this guard.
        unsafe { libc::closedir(self.0) };
    }
}

fn raw_directory_entries(fd: RawFd) -> Result<Vec<RawDirectoryEntry>, DriverError> {
    raw_directory_entries_bounded(fd, usize::MAX).map(|(entries, _)| entries)
}

fn raw_directory_entries_bounded(
    fd: RawFd,
    limit: usize,
) -> Result<(Vec<RawDirectoryEntry>, bool), DriverError> {
    // A fresh open file description is required: dup(2) would share the directory offset and
    // make a later cleanup batch falsely observe end-of-directory.
    let scan = openat2(
        fd,
        ".",
        libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
        0,
    )?;
    let raw = scan.as_raw_fd();
    std::mem::forget(scan);
    // SAFETY: ownership of the fresh descriptor transfers to DIR*.
    let directory = unsafe { libc::fdopendir(raw) };
    if directory.is_null() {
        // SAFETY: fdopendir did not acquire the duplicated fd on failure.
        unsafe { libc::close(raw) };
        return Err(io_failed(
            "workspace.list-failed",
            std::io::Error::last_os_error(),
        ));
    }
    let directory = DirectoryStream(directory);
    let mut result = Vec::new();
    let mut has_more = false;
    loop {
        // SAFETY: directory remains live for the guard's lifetime.
        let entry = unsafe { libc::readdir(directory.0) };
        if entry.is_null() {
            break;
        }
        // SAFETY: readdir returns a dirent with a NUL-terminated d_name.
        let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) };
        if matches!(name.to_bytes(), b"." | b"..") {
            continue;
        }
        if result.len() == limit {
            has_more = true;
            break;
        }
        let name = CString::new(name.to_bytes()).map_err(|_| path_escape())?;
        let metadata = fstatat(fd, &name)?;
        result.push((
            name,
            metadata.st_mode & libc::S_IFMT,
            Some(u64::try_from(metadata.st_size).unwrap_or(0)),
        ));
        if result.len() > MAX_DIRECTORY_SCAN_ITEMS {
            return Err(DriverError::exhausted(
                "workspace.directory-scan-limit",
                "Directory exceeds the bounded observation limit.",
                "directory",
            ));
        }
    }
    Ok((result, has_more))
}

fn canonical_content(content: &Base64Content) -> Result<Vec<u8>, DriverError> {
    let bytes = content.decode().map_err(|_| {
        DriverError::refused(
            "request.schema-invalid",
            "File content is not canonical base64.",
            "content",
        )
    })?;
    if base64::engine::general_purpose::STANDARD.encode(&bytes) != content.data {
        return Err(DriverError::refused(
            "request.schema-invalid",
            "File content is not canonical base64.",
            "content",
        ));
    }
    Ok(bytes)
}

fn verify_expected(expected: &ExpectedFileState, before: Option<&[u8]>) -> Result<(), DriverError> {
    match (expected, before) {
        (ExpectedFileState::Absent, None) => Ok(()),
        (ExpectedFileState::Sha256 { sha256: expected }, Some(bytes)) => {
            verify_sha256(expected, bytes)
        }
        _ => Err(stale_content()),
    }
}

fn verify_sha256(expected: &str, bytes: &[u8]) -> Result<(), DriverError> {
    if sha256(bytes) == expected {
        Ok(())
    } else {
        Err(stale_content())
    }
}

fn stale_content() -> DriverError {
    DriverError {
        class: crate::DriverErrorClass::Conflict,
        code: "workspace.stale-content",
        message: "Workspace file changed since the admitted read.".to_owned(),
        address: Some("expected".to_owned()),
        retriable: false,
    }
}

fn invalid_digest() -> DriverError {
    DriverError::refused(
        "workspace.digest-invalid",
        "Expected digest must be lowercase SHA-256.",
        "expected_sha256",
    )
}

fn binary_text_refusal() -> DriverError {
    DriverError::refused(
        "workspace.binary-text-edit",
        "Structured edits require a UTF-8 regular file.",
        "file",
    )
}

fn sha256(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn mutation_result(
    workspace_id: &str,
    path: &str,
    before: Option<&[u8]>,
    after: &[u8],
    before_sha256: Option<String>,
    observation: FileObservation,
) -> FileMutationResult {
    FileMutationResult {
        kind: FileKind::File,
        workspace: workspace_id.to_owned(),
        path: path.to_owned(),
        size: observation.size,
        before_sha256,
        after_sha256: observation.sha256,
        atomic_replacement: observation.atomic_replacement,
        diff: bounded_diff(path, before, after),
        observed_at: observation.observed_at,
    }
}

fn bounded_diff(path: &str, before: Option<&[u8]>, after: &[u8]) -> UnifiedDiff {
    let before = before.unwrap_or_default();
    let (Ok(before), Ok(after)) = (std::str::from_utf8(before), std::str::from_utf8(after)) else {
        return UnifiedDiff {
            text: String::new(),
            truncated: false,
            binary: true,
        };
    };
    let mut lines = vec![
        format!("--- a/{path}"),
        format!("+++ b/{path}"),
        "@@".to_owned(),
    ];
    lines.extend(before.split_inclusive('\n').map(|line| format!("-{line}")));
    lines.extend(after.split_inclusive('\n').map(|line| format!("+{line}")));
    let truncated = lines.len() > MAX_DIFF_LINES;
    lines.truncate(MAX_DIFF_LINES);
    UnifiedDiff {
        text: lines.join("\n"),
        truncated,
        binary: false,
    }
}

fn apply_text_edit(source: &str, input: &FileEditInput) -> Result<String, DriverError> {
    let exact_matches = source.match_indices(&input.old_text).count();
    if exact_matches == 1 {
        return Ok(source.replacen(&input.old_text, &input.new_text, 1));
    }
    if exact_matches > 1 || input.match_policy == TextMatchPolicy::Exact {
        return Err(edit_match_refusal(exact_matches));
    }
    let normalized_old = normalize_text_match(&input.old_text);
    let source_lines: Vec<&str> = source.split_inclusive('\n').collect();
    let old_line_count = input.old_text.lines().count().max(1);
    let mut matches = Vec::new();
    for start in 0..source_lines.len() {
        let end = start.saturating_add(old_line_count);
        if end > source_lines.len() {
            break;
        }
        let candidate = source_lines[start..end].concat();
        if normalize_text_match(&candidate) == normalized_old {
            matches.push((start, end));
        }
    }
    if matches.len() != 1 {
        return Err(edit_match_refusal(matches.len()));
    }
    let (start, end) = matches[0];
    let mut result = String::new();
    result.push_str(&source_lines[..start].concat());
    result.push_str(&input.new_text);
    result.push_str(&source_lines[end..].concat());
    Ok(result)
}

fn normalize_text_match(value: &str) -> String {
    value.lines().map(str::trim).collect::<Vec<_>>().join("\n")
}

fn edit_match_refusal(matches: usize) -> DriverError {
    let (code, message) = if matches == 0 {
        (
            "workspace.edit-not-found",
            "Edit text was not found uniquely.",
        )
    } else {
        (
            "workspace.edit-ambiguous",
            "Edit text matched more than one location.",
        )
    };
    DriverError::refused(code, message, "old_text")
}

fn apply_line_patch(source: &str, edits: &[LinePatchEdit]) -> Result<String, DriverError> {
    let newline = if source.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let final_newline = source.ends_with('\n');
    let normalized = source.replace("\r\n", "\n");
    let mut lines: Vec<String> = normalized.lines().map(str::to_owned).collect();
    let line_count = u32::try_from(lines.len()).unwrap_or(u32::MAX);
    let mut ranges = Vec::with_capacity(edits.len());
    for edit in edits {
        let range = match edit {
            LinePatchEdit::InsertBefore { line, .. } | LinePatchEdit::InsertAfter { line, .. } => {
                (*line, *line)
            }
            LinePatchEdit::ReplaceRange {
                start_line,
                end_line,
                ..
            }
            | LinePatchEdit::DeleteRange {
                start_line,
                end_line,
            } => (*start_line, *end_line),
        };
        if range.0 == 0 || range.1 < range.0 || range.1 > line_count {
            return Err(DriverError::refused(
                "workspace.patch-range",
                "Patch line range is outside the original file.",
                "edits",
            ));
        }
        ranges.push(range);
    }
    ranges.sort_unstable();
    if ranges.windows(2).any(|pair| pair[0].1 >= pair[1].0) {
        return Err(DriverError::refused(
            "workspace.patch-overlap",
            "Patch edits overlap in original coordinates.",
            "edits",
        ));
    }
    for edit in edits.iter().rev() {
        match edit {
            LinePatchEdit::InsertBefore { line, text } => {
                lines.splice(index(*line)..index(*line), text_lines(text));
            }
            LinePatchEdit::InsertAfter { line, text } => {
                let at = usize::try_from(*line).expect("u32 fits usize");
                lines.splice(at..at, text_lines(text));
            }
            LinePatchEdit::ReplaceRange {
                start_line,
                end_line,
                text,
            } => {
                lines.splice(
                    index(*start_line)..usize::try_from(*end_line).unwrap(),
                    text_lines(text),
                );
            }
            LinePatchEdit::DeleteRange {
                start_line,
                end_line,
            } => {
                lines.drain(index(*start_line)..usize::try_from(*end_line).unwrap());
            }
        }
    }
    let mut result = lines.join(newline);
    if final_newline {
        result.push_str(newline);
    }
    Ok(result)
}

fn index(line: u32) -> usize {
    usize::try_from(line.saturating_sub(1)).expect("u32 fits usize")
}

fn text_lines(value: &str) -> Vec<String> {
    value
        .replace("\r\n", "\n")
        .lines()
        .map(str::to_owned)
        .collect()
}

fn existing_file_is_executable(parent: RawFd, name: &str) -> Result<bool, DriverError> {
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
            Ok(stat.st_mode & 0o111 != 0)
        }
        Err(error) if error.code == "resource.not-found" => Ok(false),
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

/// Refuse any workspace root name that is not a single, safely spellable path component.
///
/// The `ws_` prefix this used to demand was never the containment. It is the **id scheme**
/// (`docs/design/01-contract.md` §1, "Identity of resources") and a prefix check stops no
/// attacker: containment is `workspace_fd` opening the name with `openat2` relative to the
/// pinned root descriptor under `RESOLVE_BENEATH | RESOLVE_NO_SYMLINKS` and `O_NOFOLLOW`, so
/// resolution cannot climb out of the root and cannot be redirected by a symlink. All this
/// function has to carry is the other half of that argument — that the name really is one
/// component, never `/`, `.` or `..` — because a single component under a pinned descriptor
/// cannot address anything outside the root whatever it is called.
///
/// Dropping the prefix is what lets an operator hand over a directory they already have, say
/// `harness` or `engineering-protocols`, and have it served as a confined workspace under its
/// own name. The workspace id then *is* the directory name, so no path-to-id mapping has to be
/// held anywhere, and there is no second record to drift out of step with the disk.
/// `create_workspace` keeps minting `ws_…` ids; only adoption of an existing directory is new.
///
/// A leading `-` is refused as well: such a name is indistinguishable from an option to any
/// tool that is ever handed it as an argv element.
fn validate_root_name(name: &str) -> Result<(), DriverError> {
    let single_component = !name.is_empty()
        && name != "."
        && name != ".."
        && !name.starts_with('-')
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-');
    if single_component {
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
    if error.raw_os_error() == Some(libc::EDQUOT) {
        return DriverError::exhausted(
            "workspace.storage-quota-exhausted",
            "The declared workspace byte or inode ceiling refused the write.",
            "storage",
        );
    }
    if error.raw_os_error() == Some(libc::ENOSPC) {
        return DriverError::exhausted(
            "workspace.storage-exhausted",
            "The backing workspace filesystem has no free capacity.",
            "storage",
        );
    }
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
    use std::ffi::CString;
    use std::ffi::OsString;
    use std::os::fd::AsRawFd as _;
    use std::os::unix::ffi::OsStringExt as _;
    use std::os::unix::fs::symlink;

    use substrate_wire::{FileMode, FileReadQuery};
    use tempfile::tempdir;

    use super::{
        DESTROY_BATCH_ITEMS, GuardedFilesystem, WorkspaceDestroyBatch, list_directory, openat2,
        read_bounded_complete, remove_children_batch, validate_root_name,
    };

    #[test]
    fn a_file_growing_after_metadata_is_still_read_under_the_complete_file_bound() {
        let directory = tempdir().expect("tempdir");
        let path = directory.path().join("growing");
        std::fs::write(&path, vec![b'x'; 1_025]).expect("grown file");
        let file = std::fs::File::open(path).expect("file");
        let error = read_bounded_complete(file, 1, 1_024)
            .expect_err("bytes beyond stale metadata must be detected");
        assert_eq!(error.code, "workspace.file-limit");
    }

    #[test]
    fn a_directory_the_operator_already_owns_is_served_under_its_own_name() {
        // Prevents the regression where only a `ws_`-prefixed directory could be served, so a
        // harness could never be pointed at a real checkout, only at a renamed scratch copy.
        validate_root_name("harness").expect("a plain project directory name is a workspace root");
        let directory = tempdir().expect("tempdir");
        let filesystem =
            GuardedFilesystem::open(directory.path(), 1024, 1024, 100).expect("guarded filesystem");
        if !filesystem.openat2_available() {
            return;
        }
        std::fs::create_dir(directory.path().join("harness")).expect("operator directory");
        filesystem
            .observe_workspace("harness")
            .expect("an adopted directory is observable as a workspace");
        let observed = filesystem
            .write_atomic("harness", "harness", "notes.txt", b"adopted")
            .expect("atomic write inside the adopted root");
        assert!(observed.atomic_replacement);
        assert_eq!(
            std::fs::read_to_string(directory.path().join("harness/notes.txt")).expect("content"),
            "adopted"
        );
    }

    #[test]
    fn a_hyphenated_project_directory_name_is_a_legal_workspace_root() {
        // Prevents adoption from working for `harness` and silently failing for every repository
        // whose own name carries a hyphen, such as `engineering-protocols`.
        validate_root_name("engineering-protocols").expect("hyphenated root name");
        let directory = tempdir().expect("tempdir");
        let filesystem =
            GuardedFilesystem::open(directory.path(), 1024, 1024, 100).expect("guarded filesystem");
        if !filesystem.openat2_available() {
            return;
        }
        std::fs::create_dir(directory.path().join("engineering-protocols")).expect("directory");
        filesystem
            .workspace_fd("engineering-protocols")
            .expect("hyphenated workspace fd");
    }

    #[test]
    fn a_root_name_that_is_not_a_single_path_component_is_still_refused() {
        // Prevents dropping the `ws_` prefix from also dropping the containment argument: only a
        // single component may reach openat2, so traversal, separators, empty, NUL-bearing and
        // option-shaped names must stay refused as a path escape.
        for name in [
            "",
            ".",
            "..",
            "a/b",
            "../escape",
            "/absolute",
            "ws%20test",
            "ws_test\0",
            "-rf",
            ".hidden",
        ] {
            let error = validate_root_name(name).expect_err(name);
            assert_eq!(error.code, "workspace.path-escape", "{name:?}");
        }
    }

    #[test]
    fn create_workspace_still_mints_and_accepts_the_server_prefixed_identity() {
        // Prevents a widened root-name rule from being mistaken for a change to the id scheme:
        // the server still names what it creates `ws_<ULID>`, and those names must keep working.
        let directory = tempdir().expect("tempdir");
        let filesystem =
            GuardedFilesystem::open(directory.path(), 1024, 1024, 100).expect("guarded filesystem");
        if !filesystem.openat2_available() {
            return;
        }
        let minted = format!("ws_{}", ulid::Ulid::generate());
        assert!(minted.starts_with("ws_"));
        filesystem
            .create_workspace(&minted)
            .expect("server-minted workspace");
        filesystem
            .workspace_fd(&minted)
            .expect("minted workspace fd");
        assert!(directory.path().join(&minted).is_dir());
    }

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

    #[test]
    fn guarded_file_and_tree_apis_never_expose_git_control_data() {
        let directory = tempdir().expect("tempdir");
        let filesystem =
            GuardedFilesystem::open(directory.path(), 1024, 1024, 100).expect("guarded filesystem");
        if !filesystem.openat2_available() {
            return;
        }
        filesystem.create_workspace("ws_git").expect("workspace");
        let root = directory.path().join("ws_git");
        std::fs::create_dir(root.join(".git")).expect("git directory");
        std::fs::write(root.join(".git/config"), b"secret control data").expect("git config");
        std::fs::write(root.join("visible.txt"), b"visible").expect("visible file");

        let direct = filesystem
            .read(
                "ws_git",
                "ws_git",
                ".git/config",
                &FileReadQuery {
                    mode: FileMode::File,
                    offset: Some(0),
                    limit_bytes: Some(64),
                    cursor: None,
                    limit_items: None,
                },
            )
            .expect_err("direct .git read must look absent");
        assert_eq!(direct.code, "resource.not-found");
        assert_eq!(
            filesystem
                .write_atomic("ws_git", "ws_git", ".git/config", b"replacement")
                .expect_err("direct .git write must look absent")
                .code,
            "resource.not-found"
        );
        assert_eq!(
            filesystem
                .delete_file("ws_git", "ws_git", ".git/config")
                .expect_err("direct .git delete must look absent")
                .code,
            "resource.not-found"
        );

        let root_fd = filesystem.workspace_fd("ws_git").expect("workspace fd");
        let root_items = list_directory(root_fd.as_raw_fd()).expect("root listing");
        assert_eq!(root_items.len(), 1);
        assert_eq!(root_items[0].name, "visible.txt");

        let tree = filesystem
            .list_tree(
                "ws_git",
                "ws_git",
                &substrate_wire::WorkspaceTreeQuery {
                    limit_items: 100,
                    include_hidden: true,
                },
            )
            .expect("tree listing");
        assert_eq!(tree.items.len(), 1);
        assert_eq!(tree.items[0].path, "visible.txt");
    }

    #[test]
    fn atomic_replacement_preserves_the_existing_executable_class() {
        use std::os::unix::fs::PermissionsExt as _;

        let directory = tempdir().expect("tempdir");
        let filesystem =
            GuardedFilesystem::open(directory.path(), 1024, 1024, 100).expect("guarded filesystem");
        if !filesystem.openat2_available() {
            return;
        }
        filesystem.create_workspace("ws_test").expect("workspace");
        let executable = directory.path().join("ws_test/tool");
        let ordinary = directory.path().join("ws_test/notes");
        std::fs::write(&executable, b"old tool").expect("seed executable");
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700))
            .expect("mark executable");
        std::fs::write(&ordinary, b"old notes").expect("seed ordinary file");

        filesystem
            .write_atomic("ws_test", "ws_test", "tool", b"new tool")
            .expect("replace executable");
        filesystem
            .write_atomic("ws_test", "ws_test", "notes", b"new notes")
            .expect("replace ordinary file");

        assert_ne!(
            std::fs::metadata(executable)
                .expect("executable metadata")
                .permissions()
                .mode()
                & 0o111,
            0
        );
        assert_eq!(
            std::fs::metadata(ordinary)
                .expect("ordinary metadata")
                .permissions()
                .mode()
                & 0o111,
            0
        );
    }

    #[test]
    fn destroy_removes_fifo_non_utf8_and_deep_entries_without_following() {
        let directory = tempdir().expect("tempdir");
        let filesystem =
            GuardedFilesystem::open(directory.path(), 1024, 1024, 100).expect("guarded filesystem");
        if !filesystem.openat2_available() {
            return;
        }
        filesystem
            .create_workspace("ws_hostile")
            .expect("workspace");
        let root = directory.path().join("ws_hostile");
        let fifo = std::ffi::CString::new(root.join("pipe").as_os_str().as_encoded_bytes())
            .expect("fifo path");
        // SAFETY: path is a valid NUL-terminated test path.
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
        std::fs::write(root.join(OsString::from_vec(vec![0xff])), b"raw").expect("raw filename");
        let mut deep = root;
        for _ in 0..70 {
            deep = deep.join("d");
            std::fs::create_dir(&deep).expect("deep directory");
        }
        destroy_until_absent(&filesystem, "ws_hostile");
        assert!(!directory.path().join("ws_hostile").exists());
    }

    #[test]
    fn destroy_batches_progress_beyond_former_depth_and_item_caps() {
        let directory = tempdir().expect("tempdir");
        let filesystem =
            GuardedFilesystem::open(directory.path(), 1024, 1024, 100).expect("guarded filesystem");
        if !filesystem.openat2_available() {
            return;
        }

        filesystem
            .create_workspace("ws_deep")
            .expect("deep workspace");
        let deep_root = filesystem.workspace_fd("ws_deep").expect("workspace fd");
        let child_name = CString::new("d").expect("child name");
        let mut parent = openat2(
            deep_root.as_raw_fd(),
            ".",
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
            0,
        )
        .expect("root duplicate");
        for _ in 0..1_100 {
            // SAFETY: parent is live and child_name is a fixed NUL-terminated relative name.
            assert_eq!(
                unsafe { libc::mkdirat(parent.as_raw_fd(), child_name.as_ptr(), 0o700) },
                0
            );
            parent = openat2(
                parent.as_raw_fd(),
                "d",
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
                0,
            )
            .expect("open nested directory");
        }
        drop(parent);
        assert_cleanup_batches_progress(deep_root.as_raw_fd());
        drop(deep_root);
        assert_eq!(
            filesystem
                .destroy_workspace_batch("ws_deep")
                .expect("remove empty deep workspace"),
            WorkspaceDestroyBatch::Absent
        );

        filesystem
            .create_workspace("ws_wide")
            .expect("wide workspace");
        let wide_root = filesystem.workspace_fd("ws_wide").expect("workspace fd");
        for index in 0..=u32::try_from(DESTROY_BATCH_ITEMS).expect("batch fits u32") {
            let name = CString::new(format!("f{index:06}")).expect("entry name");
            // SAFETY: workspace descriptor and generated direct-child name are valid.
            assert_eq!(
                unsafe {
                    libc::mknodat(
                        wide_root.as_raw_fd(),
                        name.as_ptr(),
                        libc::S_IFREG | 0o600,
                        0,
                    )
                },
                0
            );
        }
        assert_cleanup_batches_progress(wide_root.as_raw_fd());
        drop(wide_root);
        assert_eq!(
            filesystem
                .destroy_workspace_batch("ws_wide")
                .expect("remove empty wide workspace"),
            WorkspaceDestroyBatch::Absent
        );
        assert!(!directory.path().join("ws_deep").exists());
        assert!(!directory.path().join("ws_wide").exists());
    }

    fn assert_cleanup_batches_progress(root: libc::c_int) {
        let mut batches = 0;
        loop {
            let batch = remove_children_batch(root, DESTROY_BATCH_ITEMS).expect("cleanup batch");
            batches += 1;
            if batch.complete {
                break;
            }
            assert!(
                batch.processed_items > 0,
                "every incomplete batch must advance cleanup"
            );
            assert!(batch.processed_items <= DESTROY_BATCH_ITEMS);
        }
        assert!(batches > 1, "fixture must require repeated bounded batches");
    }

    fn destroy_until_absent(filesystem: &GuardedFilesystem, root_name: &str) {
        for _ in 0..2_048 {
            match filesystem
                .destroy_workspace_batch(root_name)
                .expect("workspace cleanup batch")
            {
                WorkspaceDestroyBatch::Pending { removed_items } => {
                    assert!(removed_items <= u64::try_from(DESTROY_BATCH_ITEMS).unwrap());
                }
                WorkspaceDestroyBatch::Absent => return,
            }
        }
        panic!("workspace cleanup did not complete within the test bound");
    }
}
