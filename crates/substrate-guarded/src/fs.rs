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

use crate::{DriverError, Listed};

const RESOLVE_NO_XDEV: u64 = 0x01;
const RESOLVE_NO_MAGICLINKS: u64 = 0x02;
const RESOLVE_NO_SYMLINKS: u64 = 0x04;
const RESOLVE_BENEATH: u64 = 0x08;
const GUARDED_RESOLVE: u64 =
    RESOLVE_BENEATH | RESOLVE_NO_MAGICLINKS | RESOLVE_NO_SYMLINKS | RESOLVE_NO_XDEV;
/// The only flags a workspace file is opened with for reading. A confined process can put a FIFO
/// (or, with privilege, a device) at any workspace path; without `O_NONBLOCK` opening it waits for
/// a writer forever, before the regular-file check that follows every such open can refuse it.
/// `O_NONBLOCK` does not change reads of a regular file.
const FILE_READ_FLAGS: i32 = libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK;
const MAX_DIRECTORY_SCAN_ITEMS: usize = 100_000;
const MAX_DIFF_LINES: usize = 200;
const MAX_PATCH_EDITS: usize = 128;
pub const DESTROY_BATCH_ITEMS: usize = 4_096;
type RawDirectoryEntry = (CString, libc::mode_t, Option<u64>);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceDestroyBatch {
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

    /// A duplicate of the workspaces-root descriptor pinned at open, for the exec path to resolve
    /// workspaces beneath, so exec and the file routes name one root.
    pub fn root_descriptor(&self) -> Result<OwnedFd, DriverError> {
        // SAFETY: duplicates the live root descriptor; the result is owned immediately.
        let fd = unsafe { libc::fcntl(self.root.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 3) };
        owned_fd(fd, "workspace root")
    }

    pub fn root_identity(&self) -> Result<(libc::dev_t, libc::ino_t), DriverError> {
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
    ) -> Result<Listed<FileReadResult>, DriverError> {
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
                let fd = openat2(workspace.as_raw_fd(), path, FILE_READ_FLAGS, 0)?;
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
                Ok(Listed::complete(FileReadResult::File(FileSlice {
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
                })))
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
                let (mut items, omitted_entries) = list_directory(fd.as_raw_fd())?;
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
                // The count is the directory's, not the page's: an omitted entry is on no page, so
                // every page of one directory names the same count (ADR 0043, decision 3).
                Ok(Listed {
                    value: FileReadResult::Directory(DirectoryPage {
                        kind: DirectoryKind::Directory,
                        workspace: workspace_id.to_owned(),
                        path: path.to_owned(),
                        items: items[start..end].to_vec(),
                        next_cursor,
                        observed_at: Utc::now(),
                    }),
                    omitted_entries,
                })
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
    ) -> Result<Listed<WorkspaceTree>, DriverError> {
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
        let mut walk = TreeWalk::default();
        walk_tree(
            workspace.as_raw_fd(),
            "",
            query.include_hidden,
            usize::try_from(query.limit_items).expect("u32 tree limit fits usize"),
            &mut items,
            &mut walk,
        )?;
        // Either cause sets `truncated`, as since 0.10.0, because a client reads it as "incomplete";
        // the header carries the omission count, which tells the two causes apart (ADR 0043).
        Ok(Listed {
            value: WorkspaceTree {
                workspace: workspace_id.to_owned(),
                items,
                truncated: walk.limit_reached || walk.omitted > 0,
                observed_at: Utc::now(),
            },
            omitted_entries: walk.omitted,
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
        let fd = openat2(workspace, path, FILE_READ_FLAGS, 0)?;
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

/// The wire name and kind of one directory entry, or `None` for an entry the contract cannot
/// name: a type other than regular file, directory or symlink (a FIFO, socket or device), or a
/// name that is not UTF-8. Such an entry is omitted from listings; it is not an escape.
fn representable_entry(
    name: &CStr,
    file_type: libc::mode_t,
    size: Option<u64>,
) -> Option<(String, DirectoryEntryKind, Option<u64>)> {
    let (kind, size) = match file_type {
        libc::S_IFREG => (DirectoryEntryKind::File, size),
        libc::S_IFDIR => (DirectoryEntryKind::Directory, None),
        libc::S_IFLNK => (DirectoryEntryKind::Symlink, None),
        _ => return None,
    };
    let name = String::from_utf8(name.to_bytes().to_vec()).ok()?;
    Some((name, kind, size))
}

/// The directory's representable entries and how many it omitted. `.git` is left out by policy
/// and is not counted; an entry the wire cannot name is counted (ADR 0043, decision 2).
fn list_directory(fd: RawFd) -> Result<(Vec<DirectoryEntry>, u64), DriverError> {
    let entries = raw_directory_entries(fd)?;
    let mut result = Vec::new();
    let mut omitted = 0_u64;
    for (name, file_type, size) in entries {
        if name.to_bytes() == b".git" {
            continue;
        }
        let Some((name_text, kind, size)) = representable_entry(&name, file_type, size) else {
            omitted = omitted.saturating_add(1);
            continue;
        };
        result.push(DirectoryEntry {
            name: name_text,
            kind,
            size,
        });
    }
    Ok((result, omitted))
}

/// Why a tree walk answers an incomplete tree. The two causes are kept apart because only
/// reaching the limit stops the walk; an omission is counted, sets `truncated` beside the limit, and
/// its count is answered as a header that tells the two causes apart (ADR 0043).
#[derive(Default)]
struct TreeWalk {
    limit_reached: bool,
    omitted: u64,
}

fn walk_tree(
    directory: RawFd,
    prefix: &str,
    include_hidden: bool,
    limit: usize,
    result: &mut Vec<WorkspaceTreeEntry>,
    walk: &mut TreeWalk,
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
        // Omitted before the limit check: an unrepresentable entry is never counted toward the
        // limit and never descended into; it is counted, and the count is answered as a header.
        let Some((name_text, kind, entry_size)) = representable_entry(&name, file_type, size)
        else {
            walk.omitted = walk.omitted.saturating_add(1);
            continue;
        };
        if result.len() == limit {
            walk.limit_reached = true;
            return Ok(());
        }
        let path = if prefix.is_empty() {
            name_text
        } else {
            format!("{prefix}/{name_text}")
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
                walk,
            )?;
            if walk.limit_reached {
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
        ranges.push((range, edit));
    }
    ranges.sort_unstable_by_key(|(range, _)| *range);
    if ranges.windows(2).any(|pair| pair[0].0.1 >= pair[1].0.0) {
        return Err(DriverError::refused(
            "workspace.patch-overlap",
            "Patch edits overlap in original coordinates.",
            "edits",
        ));
    }
    // Apply bottom-up in original line order, never in request order, so an earlier edit never
    // shifts the lines a later one addresses and every permutation of the list gives the same
    // bytes. No tie-break between kinds is needed: two edits anchored to one original line
    // overlap and were refused above, so start lines are strictly ascending here.
    for (_, edit) in ranges.into_iter().rev() {
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

/// Pins the directory an exec mounts at `/workspace`: the entry `workspace` names, opened with
/// `openat2` beneath `root`, the workspaces-root descriptor the driver pinned when it opened, and
/// without following any link, so bubblewrap binds this descriptor and never re-resolves a path.
/// The file routes resolve every root name beneath that same descriptor (`workspace_fd`), so exec
/// and the file routes name one object even after the root's path is replaced.
///
/// An adopted directory belongs to the operator, who may replace it after adopt. The answers are
/// the ones adoption and the file routes give for the same entry (`classify_open_error`): absent
/// is `resource.not-found`; a symlink, a regular file or another mount in its place is
/// `workspace.path-escape`; a kernel without `openat2` is `workspace.openat2-unavailable`.
pub fn pin_workspace_directory_beneath(
    root: &OwnedFd,
    workspace: &Path,
) -> Result<OwnedFd, DriverError> {
    let Some(name) = workspace.file_name() else {
        return Err(path_escape());
    };
    let name = CString::new(name.as_bytes()).map_err(|_| path_escape())?;
    openat2_cstr(
        root.as_raw_fd(),
        &name,
        libc::O_PATH | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        0,
    )
}

/// Opens one component of a writable subtree beneath the pinned directory `directory`, without
/// following a link, and reports what it is.
pub fn open_component_beneath(
    directory: &OwnedFd,
    component: &str,
) -> Result<OwnedFd, DriverError> {
    openat2(
        directory.as_raw_fd(),
        component,
        libc::O_PATH | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        0,
    )
}

/// Reads the regular file `name` directly beneath the pinned directory `directory`, opened with
/// `openat2` beneath it without following any link, and at most `max_bytes` of it. An absent file
/// is `Ok(None)`; a symlink, another kind of file, or one over the bound is an error.
pub fn read_regular_beneath(
    directory: &OwnedFd,
    name: &str,
    max_bytes: u64,
) -> Result<Option<Vec<u8>>, DriverError> {
    let fd = match openat2(directory.as_raw_fd(), name, FILE_READ_FLAGS, 0) {
        Ok(fd) => fd,
        Err(error) if error.class == crate::DriverErrorClass::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let stat = fstat(fd.as_raw_fd())?;
    if stat.st_mode & libc::S_IFMT != libc::S_IFREG {
        return Err(path_escape());
    }
    let size = u64::try_from(stat.st_size).unwrap_or(u64::MAX);
    if size > max_bytes {
        return Err(DriverError::exhausted(
            "workspace.file-limit",
            "File exceeds the bound it is read within.",
            "file",
        ));
    }
    read_bounded_complete(File::from(fd), size, max_bytes).map(Some)
}

/// [`pin_workspace_directory_beneath`] for a directory the caller has just created and holds by
/// path, as the capability probe does: the parent is opened by path, following links.
pub fn pin_workspace_directory(workspace: &Path) -> Result<OwnedFd, DriverError> {
    let Some(parent) = workspace.parent() else {
        return Err(path_escape());
    };
    let parent = CString::new(parent.as_os_str().as_bytes()).map_err(|_| path_escape())?;
    // SAFETY: the C string is terminated and the successful descriptor is immediately owned.
    let root = unsafe {
        libc::open(
            parent.as_ptr(),
            libc::O_PATH | libc::O_DIRECTORY | libc::O_CLOEXEC,
        )
    };
    if root < 0 {
        return Err(classify_open_error(std::io::Error::last_os_error()));
    }
    let root = owned_fd(root, "open")?;
    pin_workspace_directory_beneath(&root, workspace)
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
/// Dropping the prefix is what lets `workspace.adopt` (ADR 0035) serve a directory the operator
/// already has, say `harness` or `engineering-protocols`, under its own name: the store row keeps
/// a minted `ws_…` id and this name as its `root_name`.
///
/// The set is `substrate_wire::is_root_name`, the one predicate the adopt admission also uses.
/// It adds the `NAME_MAX` length bound, and refuses a leading `-` because such a name is
/// indistinguishable from an option to any tool that is ever handed it as an argv element.
pub fn validate_root_name(name: &str) -> Result<(), DriverError> {
    if substrate_wire::is_root_name(name) {
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

    use substrate_wire::{FileMode, FileReadQuery, LinePatchEdit};
    use tempfile::tempdir;

    use crate::Listed;

    use super::{
        DESTROY_BATCH_ITEMS, GuardedFilesystem, WorkspaceDestroyBatch, apply_line_patch,
        list_directory, openat2, read_bounded_complete, remove_children_batch, validate_root_name,
    };

    const SIX_LINES: &str = "L1\nL2\nL3\nL4\nL5\nL6\n";

    fn insert_before(line: u32, text: &str) -> LinePatchEdit {
        LinePatchEdit::InsertBefore {
            line,
            text: text.to_owned(),
        }
    }

    fn insert_after(line: u32, text: &str) -> LinePatchEdit {
        LinePatchEdit::InsertAfter {
            line,
            text: text.to_owned(),
        }
    }

    fn replace(start_line: u32, end_line: u32, text: &str) -> LinePatchEdit {
        LinePatchEdit::ReplaceRange {
            start_line,
            end_line,
            text: text.to_owned(),
        }
    }

    fn every_permutation(edits: &[LinePatchEdit]) -> Vec<Vec<LinePatchEdit>> {
        if edits.len() <= 1 {
            return vec![edits.to_vec()];
        }
        let mut all = Vec::new();
        for (at, first) in edits.iter().enumerate() {
            let mut rest = edits.to_vec();
            rest.remove(at);
            for mut tail in every_permutation(&rest) {
                tail.insert(0, first.clone());
                all.push(tail);
            }
        }
        all
    }

    #[test]
    fn a_line_patch_gives_the_same_bytes_whatever_order_its_two_edits_arrive_in() {
        // Prevents the regression where edits were applied in reverse *input* order, so a
        // non-ascending list shifted the later edit onto the wrong line.
        let ascending = [insert_before(2, "A\nB"), replace(5, 5, "X")];
        let expected = "L1\nA\nB\nL2\nL3\nL4\nX\nL6\n";
        assert_eq!(
            apply_line_patch(SIX_LINES, &ascending).expect("ascending patch"),
            expected
        );
        let descending = [replace(5, 5, "X"), insert_before(2, "A\nB")];
        assert_eq!(
            apply_line_patch(SIX_LINES, &descending).expect("descending patch"),
            expected
        );
    }

    #[test]
    fn a_line_patch_gives_the_same_bytes_for_every_permutation_of_three_edits() {
        // `insert_after(2)` and `insert_before(3)` land at the same boundary between L2 and L3;
        // the one anchored to the earlier line is applied nearer the top.
        let edits = [
            insert_after(2, "after-2"),
            insert_before(3, "before-3"),
            replace(5, 6, "X"),
        ];
        let expected = "L1\nL2\nafter-2\nbefore-3\nL3\nL4\nX\n";
        for permutation in every_permutation(&edits) {
            assert_eq!(
                apply_line_patch(SIX_LINES, &permutation).expect("permuted patch"),
                expected,
                "{permutation:?}"
            );
        }
    }

    #[test]
    fn two_line_patch_edits_anchored_to_one_line_are_refused_in_either_order() {
        // Edits at the same original line have no ordering to apply in; they are refused as
        // overlapping, whichever kind arrives first.
        for pair in [
            [insert_before(3, "A"), replace(3, 3, "X")],
            [replace(3, 3, "X"), insert_before(3, "A")],
            [insert_before(3, "A"), insert_after(3, "B")],
            [insert_after(3, "B"), insert_before(3, "A")],
        ] {
            let error = apply_line_patch(SIX_LINES, &pair).expect_err("same-line edits");
            assert_eq!(error.code, "workspace.patch-overlap", "{pair:?}");
        }
    }

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
        // One predicate with the wire (ADR 0035): Linux NAME_MAX bounds a root name.
        let longest = "a".repeat(substrate_wire::MAX_ROOT_NAME_BYTES);
        validate_root_name(&longest).expect("NAME_MAX bytes");
        let error = validate_root_name(&format!("{longest}a")).expect_err("over NAME_MAX");
        assert_eq!(error.code, "workspace.path-escape");
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
            .expect("read")
            .value;
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
        let (root_items, root_omitted) = list_directory(root_fd.as_raw_fd()).expect("root listing");
        assert_eq!(
            root_omitted, 0,
            ".git is left out by policy, not counted as omitted"
        );
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
        assert_eq!(tree.omitted_entries, 0, ".git is not an omission");
        let tree = tree.value;
        assert_eq!(tree.items.len(), 1);
        assert_eq!(tree.items[0].path, "visible.txt");
    }

    #[test]
    fn special_entries_are_omitted_from_directory_pages_and_trees_rather_than_refused() {
        // Prevents a FIFO, a socket or a non-UTF-8 name, any of which a confined process can
        // create, from making its directory page and the whole workspace tree answer
        // `workspace.path-escape` for an escape that did not happen.
        let directory = tempdir().expect("tempdir");
        let filesystem =
            GuardedFilesystem::open(directory.path(), 1024, 1024, 100).expect("guarded filesystem");
        if !filesystem.openat2_available() {
            return;
        }
        filesystem.create_workspace("ws_mixed").expect("workspace");
        let mixed = directory.path().join("ws_mixed/mixed");
        std::fs::create_dir_all(mixed.join("sub")).expect("sub directory");
        std::fs::write(mixed.join("sub/inner.txt"), b"inner").expect("inner file");
        std::fs::write(mixed.join("visible.txt"), b"visible").expect("visible file");
        std::fs::write(directory.path().join("ws_mixed/z-after.txt"), b"after")
            .expect("after file");
        let fifo = CString::new(mixed.join("a-fifo").into_os_string().into_vec()).expect("path");
        // SAFETY: `fifo` is a valid NUL-terminated path.
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0, "mkfifo");
        let socket = std::os::unix::net::UnixListener::bind(mixed.join("b-socket")).expect("bind");
        drop(socket);
        let raw_name = mixed.join(std::path::PathBuf::from(OsString::from_vec(
            b"c\xff-dir".to_vec(),
        )));
        std::fs::create_dir(&raw_name).expect("non-UTF-8 directory");
        std::fs::write(raw_name.join("never-listed.txt"), b"x").expect("file beneath it");

        let page = |cursor: Option<String>, limit_items: u32| {
            let listed = filesystem
                .read(
                    "ws_mixed",
                    "ws_mixed",
                    "mixed",
                    &FileReadQuery {
                        mode: FileMode::Directory,
                        offset: None,
                        limit_bytes: None,
                        cursor,
                        limit_items: Some(limit_items),
                    },
                )
                .expect("directory page with special entries");
            let super::FileReadResult::Directory(page) = listed.value else {
                panic!("directory mode answers a directory page");
            };
            (page, listed.omitted_entries)
        };
        let (whole, omitted) = page(None, 100);
        let names: Vec<&str> = whole.items.iter().map(|item| item.name.as_str()).collect();
        assert_eq!(names, ["sub", "visible.txt"]);
        assert_eq!(whole.next_cursor, None);
        // ADR 0043: the FIFO, the socket and the non-UTF-8 directory, each counted once.
        assert_eq!(omitted, 3, "the page names the three entries it omitted");
        // Decision 3: the count is the directory's, so every page carries it.
        let (first, first_omitted) = page(None, 1);
        let (second, second_omitted) = page(first.next_cursor.clone(), 1);
        assert_eq!(first.items.len() + second.items.len(), 2);
        assert_eq!((first_omitted, second_omitted), (3, 3));

        let expected = [
            "mixed",
            "mixed/sub",
            "mixed/sub/inner.txt",
            "mixed/visible.txt",
            "z-after.txt",
        ];
        for limit_items in [100, 5] {
            let listed = filesystem
                .list_tree(
                    "ws_mixed",
                    "ws_mixed",
                    &substrate_wire::WorkspaceTreeQuery {
                        limit_items,
                        include_hidden: false,
                    },
                )
                .expect("tree with special entries");
            let paths: Vec<&str> = listed
                .value
                .items
                .iter()
                .map(|item| item.path.as_str())
                .collect();
            assert_eq!(paths, expected, "limit {limit_items}");
            assert_eq!(listed.omitted_entries, 3, "limit {limit_items}");
            assert!(
                listed.value.truncated,
                "an omission still sets truncated, for compatibility (ADR 0043), limit {limit_items}"
            );
        }
        // The limit reached after every omission was walked: truncated, and still counted.
        let listed = filesystem
            .list_tree(
                "ws_mixed",
                "ws_mixed",
                &substrate_wire::WorkspaceTreeQuery {
                    limit_items: 4,
                    include_hidden: false,
                },
            )
            .expect("tree at its limit");
        assert!(listed.value.truncated, "the limit is truncation");
        assert_eq!(listed.omitted_entries, 3);
    }

    /// Runs `operation` on its own thread and answers whether it returned within a bound. A
    /// blocked open of `fifo` is released afterwards by a non-blocking writer, so a red case
    /// does not leave its thread parked for the rest of the test process.
    fn answers_promptly<R: Send + 'static>(
        fifo: &std::path::Path,
        operation: impl FnOnce() -> R + Send + 'static,
    ) -> Option<R> {
        let (sender, receiver) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let _ = sender.send(operation());
        });
        let answer = receiver
            .recv_timeout(std::time::Duration::from_secs(3))
            .ok();
        if answer.is_none() {
            let path = CString::new(fifo.as_os_str().as_encoded_bytes()).expect("fifo path");
            // SAFETY: `path` is a valid NUL-terminated path; the descriptor is closed at once.
            let writer = unsafe { libc::open(path.as_ptr(), libc::O_WRONLY | libc::O_NONBLOCK) };
            if writer >= 0 {
                // SAFETY: `writer` was just opened by this function.
                unsafe { libc::close(writer) };
            }
            let _ = worker.join();
        }
        answer
    }

    fn workspace_with_fifo(name: &str) -> (tempfile::TempDir, std::path::PathBuf) {
        let directory = tempdir().expect("tempdir");
        let root = directory.path().join(name);
        std::fs::create_dir(&root).expect("workspace root");
        let fifo = root.join("pipe");
        let path = CString::new(fifo.as_os_str().as_encoded_bytes()).expect("fifo path");
        // SAFETY: `path` is a valid NUL-terminated path.
        assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0, "mkfifo");
        (directory, fifo)
    }

    #[test]
    fn a_fifo_omitted_from_listings_still_blocks_a_v1_file_read_of_its_path() {
        // The listing now hides a FIFO a confined process made, but its path is still
        // addressable: a file-mode read opens it without O_NONBLOCK and waits for a writer that
        // never comes, before the regular-file check at fs.rs:174 can refuse it.
        let (directory, fifo) = workspace_with_fifo("ws_fifo_read");
        let filesystem =
            GuardedFilesystem::open(directory.path(), 1024, 1024, 100).expect("guarded filesystem");
        if !filesystem.openat2_available() {
            return;
        }
        let answer = answers_promptly(&fifo, move || {
            filesystem
                .read(
                    "ws_fifo_read",
                    "ws_fifo_read",
                    "pipe",
                    &FileReadQuery {
                        mode: FileMode::File,
                        offset: Some(0),
                        limit_bytes: Some(16),
                        cursor: None,
                        limit_items: None,
                    },
                )
                .map(|_| ())
                .map_err(|error| error.code)
        });
        let answer = answer.expect("a file read of a FIFO answers within 3 s instead of blocking");
        assert!(answer.is_err(), "a FIFO is not a readable file: {answer:?}");
    }

    #[test]
    fn a_fifo_omitted_from_listings_still_blocks_a_v2_digested_read_of_its_path() {
        // The same open in read_complete_from (fs.rs:454) serves v2 digested reads and every CAS
        // mutation's current-content observation (replace_cas, edit_cas, patch).
        let (directory, fifo) = workspace_with_fifo("ws_fifo_digest");
        let filesystem =
            GuardedFilesystem::open(directory.path(), 1024, 1024, 100).expect("guarded filesystem");
        if !filesystem.openat2_available() {
            return;
        }
        let answer = answers_promptly(&fifo, move || {
            filesystem
                .read_digested(
                    "ws_fifo_digest",
                    "ws_fifo_digest",
                    "pipe",
                    &FileReadQuery {
                        mode: FileMode::File,
                        offset: Some(0),
                        limit_bytes: Some(16),
                        cursor: None,
                        limit_items: None,
                    },
                )
                .map(|_| ())
                .map_err(|error| error.code)
        });
        let answer =
            answer.expect("a digested read of a FIFO answers within 3 s instead of blocking");
        assert!(answer.is_err(), "a FIFO is not a readable file: {answer:?}");
    }

    #[test]
    fn no_open_of_a_workspace_path_can_wait_for_a_fifo_writer() {
        // The class behind the FIFO cases above: an open of a workspace path that is not O_PATH,
        // not O_DIRECTORY and not O_CREAT|O_EXCL opens whatever is there, and on a FIFO without
        // O_NONBLOCK it waits for a writer. Every open call in this module's non-test source must
        // name its flags within a few lines, and each such flag set must be one of those safe
        // shapes or FILE_READ_FLAGS, which carries O_NONBLOCK.
        const WINDOW: usize = 6;
        const FORWARDING: &str = "openat2_cstr(directory, &path, flags, mode)";
        let source = include_str!("fs.rs");
        let production = source
            .split("\n#[cfg(test)]\nmod tests {")
            .next()
            .expect("module source");
        assert!(
            production.len() < source.len(),
            "the test module boundary was not found, so the scan read the wrong text"
        );
        let lines: Vec<&str> = production.lines().collect();
        let safe = |line: &str| {
            line.contains("libc::O_PATH")
                || line.contains("libc::O_DIRECTORY")
                || (line.contains("libc::O_CREAT") && line.contains("libc::O_EXCL"))
                || (line.contains("libc::O_NONBLOCK") && line.contains("const FILE_READ_FLAGS"))
        };
        let mut strays = Vec::new();
        let mut flag_lines = 0;
        for (index, line) in lines.iter().enumerate() {
            if line.contains("libc::O_CLOEXEC") {
                flag_lines += 1;
                if !safe(line) {
                    strays.push(format!("fs.rs:{}: {}", index + 1, line.trim()));
                }
            }
            if line.contains("std::fs::File::open")
                || line.contains("File::open(")
                || line.contains("OpenOptions")
            {
                strays.push(format!("fs.rs:{}: {}", index + 1, line.trim()));
            }
            let call = ["openat2(", "openat2_cstr(", "libc::open(", "libc::openat("]
                .iter()
                .any(|name| line.contains(name));
            if call && !line.trim_start().starts_with("fn ") && line.trim() != FORWARDING {
                let window = &lines[index..lines.len().min(index + WINDOW)];
                if !window.iter().any(|candidate| {
                    candidate.contains("libc::O_CLOEXEC") || candidate.contains("FILE_READ_FLAGS")
                }) {
                    strays.push(format!(
                        "fs.rs:{}: {} (flags not named)",
                        index + 1,
                        line.trim()
                    ));
                }
            }
        }
        assert!(
            flag_lines >= 15,
            "found {flag_lines} flag sets; the module has more, so the scan is not reading them"
        );
        assert!(
            strays.is_empty(),
            "opens that can wait for a FIFO writer: {strays:#?}"
        );
        assert!(
            production.matches("FILE_READ_FLAGS, 0)").count() >= 2,
            "the file read and the complete read both open with FILE_READ_FLAGS"
        );
    }

    #[test]
    fn every_compare_and_set_mutation_of_a_fifo_path_answers_instead_of_blocking() {
        // replace_cas, edit_cas and patch_cas observe the current content through the same
        // read_complete_from before they verify anything; each must refuse a FIFO promptly.
        let digest = "0".repeat(64);
        for mutation in ["replace", "edit", "patch"] {
            let name = format!("ws_fifo_{mutation}");
            let (directory, fifo) = workspace_with_fifo(&name);
            let filesystem = GuardedFilesystem::open(directory.path(), 1024, 1024, 100)
                .expect("guarded filesystem");
            if !filesystem.openat2_available() {
                return;
            }
            let digest = digest.clone();
            let answer = answers_promptly(&fifo, move || {
                let result = match mutation {
                    "replace" => filesystem.replace_cas(
                        &name,
                        &name,
                        "pipe",
                        &substrate_wire::FileReplaceInput {
                            content: substrate_wire::Base64Content {
                                encoding: substrate_wire::Base64Encoding::Base64,
                                data: "eA==".to_owned(),
                            },
                            expected: substrate_wire::ExpectedFileState::Absent,
                            create_parents: false,
                        },
                    ),
                    "edit" => filesystem.edit_cas(
                        &name,
                        &name,
                        "pipe",
                        &substrate_wire::FileEditInput {
                            expected_sha256: digest,
                            old_text: "a".to_owned(),
                            new_text: "b".to_owned(),
                            match_policy: substrate_wire::TextMatchPolicy::Exact,
                        },
                    ),
                    _ => filesystem.patch_cas(
                        &name,
                        &name,
                        "pipe",
                        &substrate_wire::FilePatchInput {
                            expected_sha256: digest,
                            edits: vec![insert_before(1, "x")],
                        },
                    ),
                };
                result.map(|_| ()).map_err(|error| error.code)
            });
            let answer = answer
                .unwrap_or_else(|| panic!("{mutation} of a FIFO answers within 3 s, not blocking"));
            assert!(
                answer.is_err(),
                "{mutation} of a FIFO is refused: {answer:?}"
            );
        }
    }

    #[test]
    fn tree_omission_probe_deep_hidden_and_exact_limit() {
        // An omission three levels deep is counted and sets truncated (ADR 0043); a hidden
        // FIFO excluded by include_hidden=false is not an omission, so an exact-limit tree without
        // one is not truncated; and omitted entries never consume limit_items.
        let directory = tempdir().expect("tempdir");
        let filesystem =
            GuardedFilesystem::open(directory.path(), 1024, 1024, 100).expect("guarded filesystem");
        if !filesystem.openat2_available() {
            return;
        }
        let mkfifo = |path: std::path::PathBuf| {
            let path = CString::new(path.into_os_string().into_vec()).expect("path");
            // SAFETY: `path` is a valid NUL-terminated path.
            assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0, "mkfifo");
        };
        let tree = |name: &str, limit_items: u32, include_hidden: bool| {
            filesystem
                .list_tree(
                    name,
                    name,
                    &substrate_wire::WorkspaceTreeQuery {
                        limit_items,
                        include_hidden,
                    },
                )
                .expect("tree")
        };

        filesystem.create_workspace("ws_deep").expect("workspace");
        let deep = directory.path().join("ws_deep");
        std::fs::create_dir_all(deep.join("a/b/c")).expect("deep dirs");
        mkfifo(deep.join("a/b/c/0-fifo"));
        std::fs::write(deep.join("a/b/c/x.txt"), b"x").expect("deep file");
        std::fs::write(deep.join("z.txt"), b"z").expect("last file");
        let Listed {
            value: observed,
            omitted_entries,
        } = tree("ws_deep", 5, false);
        let paths: Vec<&str> = observed.items.iter().map(|i| i.path.as_str()).collect();
        assert_eq!(paths, ["a", "a/b", "a/b/c", "a/b/c/x.txt", "z.txt"]);
        assert_eq!(omitted_entries, 1, "a deep omission is counted");
        assert!(observed.truncated, "a deep omission sets truncated");

        filesystem.create_workspace("ws_hidden").expect("workspace");
        let hidden = directory.path().join("ws_hidden");
        mkfifo(hidden.join(".hidden-fifo"));
        std::fs::write(hidden.join("a.txt"), b"a").expect("file");
        std::fs::write(hidden.join("b.txt"), b"b").expect("file");
        let observed = tree("ws_hidden", 2, false);
        assert_eq!(observed.value.items.len(), 2);
        assert!(!observed.value.truncated, "an exact-limit tree is complete");
        assert_eq!(
            observed.omitted_entries, 0,
            "a hidden entry the caller excluded is no omission"
        );
        let observed = tree("ws_hidden", 2, true);
        assert_eq!(observed.value.items.len(), 2);
        assert!(
            observed.value.truncated,
            "an included hidden FIFO sets truncated"
        );
        assert_eq!(
            observed.omitted_entries, 1,
            "an included hidden FIFO is an omission"
        );
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

#[cfg(test)]
mod line_patch_order_tests;
