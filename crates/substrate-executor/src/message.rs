//! The `fs` role's request and response (design 34 § 7.2).
//!
//! Each direction is one message: a `u32be` header length, a closed JSON header, a `u64be` content
//! length and the content. The request header names the operation and carries the wire crate's
//! own query and input types; only `write` carries content. The response header is
//! `{"ok": <wire observation>}`, with `"omitted_entries"` beside it when a listing omitted
//! entries the wire cannot name (ADR 0043), or `{"error": {...}}`, the guarded code's
//! `DriverError`. A response carries no content.
//!
//! **The header bound.** Design 34 § 7.2 states 16 MiB, because 64 KiB, its first figure, is a
//! bound no header that carries the wire crate's own types can keep: `FileReplaceInput` holds the whole new file inline as base64, a
//! `FileEditInput` holds two texts, and a read answer holds its slice inline. The bound is
//! therefore [`MAX_HEADER_BYTES`], which holds a [`MAX_CONTENT_BYTES`] file inline even when
//! every byte is JSON-escaped twice, plus the request's other members.

use std::io::{self, Read, Write};

use serde::{Deserialize, Serialize};
use substrate_guarded::{DriverError, DriverErrorClass};
use substrate_wire::{
    FileEditInput, FilePatchInput, FileReadQuery, FileReplaceInput, WorkspaceTreeQuery,
};

/// The largest header either direction carries: 16 MiB.
pub const MAX_HEADER_BYTES: u32 = 16 * 1024 * 1024;

/// The largest content a request carries: one complete file at the wire's own maximum.
pub const MAX_CONTENT_BYTES: u64 = substrate_wire::MAX_FILE_BYTES;

/// The failure code for any answer that is not this protocol (design 34 § 8).
pub const PROTOCOL_FAILURE: &str = "kubernetes.executor-protocol";

/// Every code a response may carry: each code the guarded file access and the environment rule
/// can return, and this protocol's own failure. A response naming anything else is
/// [`PROTOCOL_FAILURE`]; `known_codes_cover_every_guarded_code` holds this list to the guarded
/// sources.
pub const KNOWN_CODES: &[&str] = &[
    PROTOCOL_FAILURE,
    "exec.environment-refused",
    "request.schema-invalid",
    "resource.not-found",
    "workspace.already-exists",
    "workspace.binary-text-edit",
    "workspace.create-failed",
    "workspace.cursor-invalid",
    "workspace.delete-failed",
    "workspace.destroy-failed",
    "workspace.destroy-no-progress",
    "workspace.destroy-not-empty",
    "workspace.digest-invalid",
    "workspace.directory-scan-limit",
    "workspace.edit-ambiguous",
    "workspace.edit-empty-match",
    "workspace.edit-not-found",
    "workspace.file-limit",
    "workspace.io-failed",
    "workspace.list-failed",
    "workspace.list-limit",
    "workspace.mkdir-failed",
    "workspace.observe-failed",
    "workspace.openat2-unavailable",
    "workspace.patch-count",
    "workspace.patch-overlap",
    "workspace.patch-range",
    "workspace.path-escape",
    "workspace.read-failed",
    "workspace.read-limit",
    "workspace.single-root",
    "workspace.stale-content",
    "workspace.storage-exhausted",
    "workspace.storage-quota-exhausted",
    "workspace.sync-failed",
    "workspace.write-failed",
    "workspace.write-limit",
];

/// The `&'static str` a code names in [`KNOWN_CODES`], if it is one.
pub fn known_code(code: &str) -> Option<&'static str> {
    KNOWN_CODES.iter().copied().find(|known| *known == code)
}

/// Why a message was not read.
#[derive(Debug)]
pub enum MessageError {
    /// The stream ended early or failed.
    Io(io::Error),
    /// The header length is over [`MAX_HEADER_BYTES`].
    HeaderTooLarge(u32),
    /// The content length is over [`MAX_CONTENT_BYTES`].
    ContentTooLarge(u64),
    /// Bytes followed the content.
    Trailing,
    /// The header is not the closed object of its direction.
    Json(serde_json::Error),
    /// An operation other than `write` carried content.
    UnexpectedContent,
}

impl std::fmt::Display for MessageError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "message stream: {error}"),
            Self::HeaderTooLarge(length) => write!(formatter, "header of {length} bytes"),
            Self::ContentTooLarge(length) => write!(formatter, "content of {length} bytes"),
            Self::Trailing => formatter.write_str("bytes after the message"),
            Self::Json(error) => write!(formatter, "header: {error}"),
            Self::UnexpectedContent => formatter.write_str("content on an operation without any"),
        }
    }
}

impl std::error::Error for MessageError {}

/// One framed message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub header: Vec<u8>,
    pub content: Vec<u8>,
}

/// Writes one message.
///
/// # Errors
///
/// The stream's own error, or `InvalidInput` for a header or content over its bound.
pub fn write_message(output: &mut impl Write, header: &[u8], content: &[u8]) -> io::Result<()> {
    let header_length = u32::try_from(header.len())
        .ok()
        .filter(|length| *length <= MAX_HEADER_BYTES)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "header over its bound"))?;
    let content_length = u64::try_from(content.len())
        .ok()
        .filter(|length| *length <= MAX_CONTENT_BYTES)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "content over its bound"))?;
    output.write_all(&header_length.to_be_bytes())?;
    output.write_all(header)?;
    output.write_all(&content_length.to_be_bytes())?;
    output.write_all(content)?;
    output.flush()
}

/// Reads exactly one message and the end of the stream after it.
///
/// # Errors
///
/// [`MessageError`] for a short stream, a length over its bound, or trailing bytes.
pub fn read_message(input: &mut impl Read) -> Result<Message, MessageError> {
    let mut length = [0_u8; 4];
    input.read_exact(&mut length).map_err(MessageError::Io)?;
    let header_length = u32::from_be_bytes(length);
    if header_length > MAX_HEADER_BYTES {
        return Err(MessageError::HeaderTooLarge(header_length));
    }
    let header = read_exactly(input, u64::from(header_length))?;
    let mut length = [0_u8; 8];
    input.read_exact(&mut length).map_err(MessageError::Io)?;
    let content_length = u64::from_be_bytes(length);
    if content_length > MAX_CONTENT_BYTES {
        return Err(MessageError::ContentTooLarge(content_length));
    }
    let content = read_exactly(input, content_length)?;
    let mut trailing = [0_u8; 1];
    loop {
        match input.read(&mut trailing) {
            Ok(0) => break,
            Ok(_) => return Err(MessageError::Trailing),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(MessageError::Io(error)),
        }
    }
    Ok(Message { header, content })
}

/// Reads exactly `length` bytes, allocating only as they arrive, so a claimed length is never
/// trusted for an allocation before its bytes exist.
fn read_exactly(input: &mut impl Read, length: u64) -> Result<Vec<u8>, MessageError> {
    let mut bytes = Vec::new();
    input
        .take(length)
        .read_to_end(&mut bytes)
        .map_err(MessageError::Io)?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) != length {
        return Err(MessageError::Io(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "message ended before its stated length",
        )));
    }
    Ok(bytes)
}

/// One file operation on the keeper's workspace. `workspace` is the workspace id the
/// observations name; the path is relative to the mounted volume.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Request {
    Read {
        workspace: String,
        path: String,
        query: FileReadQuery,
    },
    /// The content is the complete new file.
    Write {
        workspace: String,
        path: String,
    },
    Delete {
        workspace: String,
        path: String,
    },
    ReadV2 {
        workspace: String,
        path: String,
        query: FileReadQuery,
    },
    TreeV2 {
        workspace: String,
        query: WorkspaceTreeQuery,
    },
    ReplaceV2 {
        workspace: String,
        path: String,
        input: FileReplaceInput,
    },
    EditV2 {
        workspace: String,
        path: String,
        input: FileEditInput,
    },
    PatchV2 {
        workspace: String,
        path: String,
        input: FilePatchInput,
    },
    /// The size and digest of one regular file, or its absence: how an unanswered call is
    /// resolved once (design 34 § 13 D10).
    Stat {
        workspace: String,
        path: String,
    },
    /// Whether the guarded I/O is served in this keeper.
    Check {},
}

impl Request {
    /// Writes this request and its content as one message.
    ///
    /// # Errors
    ///
    /// The stream's own error.
    pub fn write_to(&self, output: &mut impl Write, content: &[u8]) -> io::Result<()> {
        let header = serde_json::to_vec(self).map_err(io::Error::other)?;
        write_message(output, &header, content)
    }

    const fn carries_content(&self) -> bool {
        matches!(self, Self::Write { .. })
    }
}

/// Reads one request message.
///
/// # Errors
///
/// [`MessageError`] for a framing fault, a header that is not a closed request, or content on an
/// operation that carries none.
pub fn read_request(input: &mut impl Read) -> Result<(Request, Vec<u8>), MessageError> {
    let message = read_message(input)?;
    let request: Request = serde_json::from_slice(&message.header).map_err(MessageError::Json)?;
    if !request.carries_content() && !message.content.is_empty() {
        return Err(MessageError::UnexpectedContent);
    }
    Ok((request, message.content))
}

/// The class of a refused or failed operation, as [`DriverErrorClass`] spells it on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorClass {
    Refused,
    Conflict,
    Unserved,
    Exhausted,
    Failed,
    NotFound,
}

impl From<DriverErrorClass> for ErrorClass {
    fn from(class: DriverErrorClass) -> Self {
        match class {
            DriverErrorClass::Refused => Self::Refused,
            DriverErrorClass::Conflict => Self::Conflict,
            DriverErrorClass::Unserved => Self::Unserved,
            DriverErrorClass::Exhausted => Self::Exhausted,
            DriverErrorClass::Failed => Self::Failed,
            DriverErrorClass::NotFound => Self::NotFound,
        }
    }
}

impl From<ErrorClass> for DriverErrorClass {
    fn from(class: ErrorClass) -> Self {
        match class {
            ErrorClass::Refused => Self::Refused,
            ErrorClass::Conflict => Self::Conflict,
            ErrorClass::Unserved => Self::Unserved,
            ErrorClass::Exhausted => Self::Exhausted,
            ErrorClass::Failed => Self::Failed,
            ErrorClass::NotFound => Self::NotFound,
        }
    }
}

/// The `DriverError` the guarded code returned, as it crosses the Pod boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ErrorBody {
    pub class: ErrorClass,
    pub code: String,
    pub message: String,
    pub address: Option<String>,
    pub retriable: bool,
}

impl From<&DriverError> for ErrorBody {
    fn from(error: &DriverError) -> Self {
        Self {
            class: error.class.into(),
            code: error.code.to_owned(),
            message: error.message.clone(),
            address: error.address.clone(),
            retriable: error.retriable,
        }
    }
}

impl ErrorBody {
    /// This protocol's own failure, for a request it could not read.
    pub fn protocol(message: impl Into<String>) -> Self {
        Self::from(&DriverError::failed(PROTOCOL_FAILURE, message))
    }

    /// The `DriverError` this body states, its code mapped back through [`KNOWN_CODES`]. A code
    /// outside the table is `Failed` [`PROTOCOL_FAILURE`], whatever class it claimed.
    pub fn into_driver_error(self) -> DriverError {
        match known_code(&self.code) {
            Some(code) => DriverError {
                class: self.class.into(),
                code,
                message: self.message,
                address: self.address,
                retriable: self.retriable,
            },
            None => DriverError::failed(
                PROTOCOL_FAILURE,
                "The executor answered a code outside the protocol's closed set.",
            ),
        }
    }
}

/// One response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Response {
    /// The wire observation, and how many entries a listing omitted.
    Ok {
        value: serde_json::Value,
        omitted_entries: u64,
    },
    Error(ErrorBody),
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ResponseHeader {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ok: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error: Option<ErrorBody>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    omitted_entries: Option<u64>,
}

impl Response {
    /// The header bytes of this response.
    ///
    /// # Errors
    ///
    /// A value that does not serialize.
    pub fn header(&self) -> Result<Vec<u8>, serde_json::Error> {
        let header = match self {
            Self::Ok {
                value,
                omitted_entries,
            } => ResponseHeader {
                ok: Some(value.clone()),
                error: None,
                omitted_entries: (*omitted_entries > 0).then_some(*omitted_entries),
            },
            Self::Error(error) => ResponseHeader {
                ok: None,
                error: Some(error.clone()),
                omitted_entries: None,
            },
        };
        serde_json::to_vec(&header)
    }

    /// Writes this response as one message without content.
    ///
    /// # Errors
    ///
    /// The stream's own error.
    pub fn write_to(&self, output: &mut impl Write) -> io::Result<()> {
        let header = self.header().map_err(io::Error::other)?;
        write_message(output, &header, &[])
    }

    fn from_header(bytes: &[u8]) -> Result<Self, MessageError> {
        let header: ResponseHeader = serde_json::from_slice(bytes).map_err(MessageError::Json)?;
        match header {
            ResponseHeader {
                ok: Some(value),
                error: None,
                omitted_entries,
            } => Ok(Self::Ok {
                value,
                omitted_entries: omitted_entries.unwrap_or(0),
            }),
            ResponseHeader {
                ok: None,
                error: Some(error),
                omitted_entries: None,
            } => Ok(Self::Error(error)),
            _ => Err(MessageError::Json(serde::de::Error::custom(
                "a response is exactly one of ok or error, and only ok counts omitted entries",
            ))),
        }
    }
}

/// Reads one response message.
///
/// # Errors
///
/// [`MessageError`] for a framing fault, a header that is not a closed response, or content.
pub fn read_response(input: &mut impl Read) -> Result<Response, MessageError> {
    let message = read_message(input)?;
    if !message.content.is_empty() {
        return Err(MessageError::UnexpectedContent);
    }
    Response::from_header(&message.header)
}

#[cfg(test)]
mod tests {
    use super::KNOWN_CODES;

    /// The guarded sources whose `DriverError` codes a response can carry.
    const GUARDED_SOURCES: [(&str, &str); 3] = [
        ("fs.rs", include_str!("../../substrate-guarded/src/fs.rs")),
        ("lib.rs", include_str!("../../substrate-guarded/src/lib.rs")),
        (
            "environment.rs",
            include_str!("../../substrate-guarded/src/environment.rs"),
        ),
    ];

    /// Quoted literals of the code shape that are addresses, not codes.
    const ADDRESSES: [&str; 1] = ["workspace.openat2-beneath"];

    /// The class behind the closed table: a code the guarded crate can return and the table does
    /// not name would reach the driver as a protocol failure. Every quoted `<family>.<name>`
    /// literal in the guarded crate's non-test source is a code or a listed address.
    #[test]
    fn known_codes_cover_every_guarded_code() {
        let mut scanned = 0;
        for (name, source) in GUARDED_SOURCES {
            let production = source.split("\n#[cfg(test)]").next().expect("source");
            for family in ["\"exec.", "\"request.", "\"resource.", "\"workspace."] {
                for (at, _) in production.match_indices(family) {
                    let start = at + 1;
                    let Some(length) = production[start..].find('"') else {
                        continue;
                    };
                    let literal = &production[start..start + length];
                    let rest = &literal[family.len() - 1..];
                    let code_shaped = !rest.is_empty()
                        && rest.bytes().all(|byte| {
                            byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-'
                        });
                    if !code_shaped || ADDRESSES.contains(&literal) {
                        continue;
                    }
                    scanned += 1;
                    assert!(
                        KNOWN_CODES.contains(&literal),
                        "{name}: code {literal} is missing from KNOWN_CODES"
                    );
                }
            }
        }
        assert!(
            scanned > 30,
            "the scan found only {scanned} codes; it read the wrong text"
        );
    }
}
