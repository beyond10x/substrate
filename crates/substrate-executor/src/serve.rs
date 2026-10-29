//! The `fs` role: one file operation in the keeper (design 34 § 7.2).
//!
//! Every operation goes through `substrate-guarded`'s `openat2` I/O beneath the pinned volume
//! root, with no symlink, magic link or mount crossing: the host driver's own code. No shell and no
//! path string that follows a link performs file I/O.

use std::io::{self, Read, Write};
use std::path::Path;

use serde::Serialize;
use substrate_guarded::fs::{GuardedFilesystem, GuardedIoProofs};
use substrate_guarded::{DriverError, DriverErrorClass, Listed};
use substrate_wire::{FileMode, FileReadQuery, MAX_FILE_BYTES, MAX_IO_BYTES, MAX_LIST_ITEMS};

use crate::message::{ErrorBody, MAX_HEADER_BYTES, Request, Response, read_request};

/// Where the keeper mounts the workspace volume, which is the guarded root.
pub const WORKSPACE_ROOT: &str = "/workspace";

/// The root name every operation resolves the single-workspace root with. It names no child: the
/// volume root is the workspace.
const ROOT_NAME: &str = "workspace";

/// Opens `root` as the one workspace it is, at the wire's own file, read and list maxima. The
/// driver holds a request to its own, lower, published bounds before it calls.
///
/// # Errors
///
/// The guarded open's own refusal.
pub fn open_workspace(root: &Path) -> Result<GuardedFilesystem, DriverError> {
    GuardedFilesystem::open_single(root, MAX_FILE_BYTES, MAX_IO_BYTES, MAX_LIST_ITEMS)
}

/// The answer to `check`: each proof of design 34 § 6.2 on its own, made in a scratch directory in
/// the workspace that is gone before the answer is written. The driver states
/// `workspace.openat2-beneath` from the first, `workspace.guarded-io` only from the first two, and
/// `workspace.atomic-replace` from the third.
#[derive(Serialize)]
struct Check {
    openat2_beneath: bool,
    symlink_escape_refused: bool,
    atomic_replace: bool,
}

impl From<GuardedIoProofs> for Check {
    fn from(proofs: GuardedIoProofs) -> Self {
        Self {
            openat2_beneath: proofs.openat2_beneath,
            symlink_escape_refused: proofs.symlink_escape_refused,
            atomic_replace: proofs.atomic_replace,
        }
    }
}

/// The answer to `stat`: a regular file's complete size and digest, or its absence.
#[derive(Serialize)]
#[serde(untagged)]
enum Stat {
    Present {
        path: String,
        size: u64,
        sha256: String,
    },
    Absent {
        path: String,
        absent: bool,
    },
}

/// Reads one request from `input`, serves it on the filesystem `open` returns and writes one
/// response to `output`. A request that does not decode is answered, not dropped.
///
/// # Errors
///
/// Only a failure to write the response.
pub fn serve(
    input: &mut impl Read,
    output: &mut impl Write,
    open: impl FnOnce() -> Result<GuardedFilesystem, DriverError>,
) -> io::Result<()> {
    let response = match read_request(input) {
        Ok((request, content)) => match open() {
            Ok(filesystem) => handle(&filesystem, request, &content),
            Err(error) => Response::Error(ErrorBody::from(&error)),
        },
        Err(error) => Response::Error(ErrorBody::protocol(format!(
            "The executor could not read the request: {error}."
        ))),
    };
    let response = bounded(response);
    response.write_to(output)
}

/// A response whose header is over the bound becomes the list limit: only a listing grows with
/// the request, and fewer items answer it.
fn bounded(response: Response) -> Response {
    let fits = response.header().is_ok_and(|header| {
        u32::try_from(header.len()).is_ok_and(|length| length <= MAX_HEADER_BYTES)
    });
    if fits {
        return response;
    }
    Response::Error(ErrorBody::from(&DriverError::exhausted(
        "workspace.list-limit",
        "The answer exceeds the executor's response bound; request fewer items.",
        "limit",
    )))
}

/// Serves one decoded request.
pub fn handle(filesystem: &GuardedFilesystem, request: Request, content: &[u8]) -> Response {
    let result = match request {
        Request::Read {
            workspace,
            path,
            query,
        } => filesystem
            .read(&workspace, ROOT_NAME, &path, &query)
            .and_then(listed),
        Request::Write { workspace, path } => filesystem
            .write_atomic(&workspace, ROOT_NAME, &path, content)
            .and_then(complete),
        Request::Delete { workspace, path } => filesystem
            .delete_file(&workspace, ROOT_NAME, &path)
            .and_then(complete),
        Request::ReadV2 {
            workspace,
            path,
            query,
        } => filesystem
            .read_digested(&workspace, ROOT_NAME, &path, &query)
            .and_then(complete),
        Request::TreeV2 { workspace, query } => filesystem
            .list_tree(&workspace, ROOT_NAME, &query)
            .and_then(listed),
        Request::ReplaceV2 {
            workspace,
            path,
            input,
        } => filesystem
            .replace_cas(&workspace, ROOT_NAME, &path, &input)
            .and_then(complete),
        Request::EditV2 {
            workspace,
            path,
            input,
        } => filesystem
            .edit_cas(&workspace, ROOT_NAME, &path, &input)
            .and_then(complete),
        Request::PatchV2 {
            workspace,
            path,
            input,
        } => filesystem
            .patch_cas(&workspace, ROOT_NAME, &path, &input)
            .and_then(complete),
        Request::Stat { workspace, path } => stat(filesystem, &workspace, path),
        Request::Check {} => complete(Check::from(filesystem.prove_guarded_io())),
    };
    match result {
        Ok(response) => response,
        Err(error) => Response::Error(ErrorBody::from(&error)),
    }
}

fn stat(
    filesystem: &GuardedFilesystem,
    workspace: &str,
    path: String,
) -> Result<Response, DriverError> {
    // A one-byte digested read observes the whole file: its size and the digest of every byte.
    let query = FileReadQuery {
        mode: FileMode::File,
        offset: Some(0),
        limit_bytes: Some(1),
        cursor: None,
        limit_items: None,
    };
    match filesystem.read_digested(workspace, ROOT_NAME, &path, &query) {
        Ok(slice) => complete(Stat::Present {
            path,
            size: slice.size,
            sha256: slice.sha256,
        }),
        Err(error) if error.class == DriverErrorClass::NotFound => {
            complete(Stat::Absent { path, absent: true })
        }
        Err(error) => Err(error),
    }
}

fn complete(value: impl Serialize) -> Result<Response, DriverError> {
    observation(value, 0)
}

fn listed<T: Serialize>(listed: Listed<T>) -> Result<Response, DriverError> {
    observation(listed.value, listed.omitted_entries)
}

fn observation(value: impl Serialize, omitted_entries: u64) -> Result<Response, DriverError> {
    let value = serde_json::to_value(value).map_err(|_| {
        DriverError::failed(
            crate::message::PROTOCOL_FAILURE,
            "The executor could not serialize its observation.",
        )
    })?;
    Ok(Response::Ok {
        value,
        omitted_entries,
    })
}
