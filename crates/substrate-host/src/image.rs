//! Root file systems from a digest-named archive (design 38 § 3).
//!
//! An image is a gzip-compressed tar named by the sha256 of its bytes. [`ImageStore`] receives
//! the bytes into an owner-private staging directory while it computes their digest, compares
//! the digest **before anything is parsed**, unpacks the tar into a second staging directory
//! under the rules of design 38 § 3.3, and publishes the finished tree by one `renameat2` with
//! `RENAME_NOREPLACE` as `trees/<sha256 hex>`, read-only.
//!
//! Every entry is created relative to a directory descriptor with `openat2` beneath it, no
//! symbolic link and no magic link followed, through `substrate-guarded`'s layer
//! ([`substrate_guarded::fs::openat2_beneath`]); a symbolic link is stored and never followed,
//! and a hard link is made with `linkat` relative to descriptors, only to a regular file the same
//! archive already unpacked. The tar reader is this module's own ([`tar`]), so no crate's path
//! handling touches the file system; the inflate is `flate2` on its pure-Rust backend.
//!
//! No daemon route, wire member or fact uses this module yet: the wire of slice 1 is its own
//! change (design 38 § 3.6).

mod store;
mod tar;
mod unpack;

#[cfg(test)]
mod test_archive;
#[cfg(test)]
mod tests;

use std::fmt;

pub use store::{ImageHold, ImageStore, ImageTree, ImageUpload, UnpackSummary};

/// The most archive bytes one image may have: design 01 § 10's ceiling (design 38 § 3.3).
pub const MAX_IMAGE_ARCHIVE_BYTES: u64 = 104_857_600;
/// The most bytes one archive may inflate to, counted while inflating (design 38 § 3.3).
pub const MAX_IMAGE_UNPACKED_BYTES: u64 = 1_073_741_824;
/// The most entries one archive may create, a directory the unpack makes for an entry beneath
/// it counted as one (design 38 § 3.3).
pub const MAX_IMAGE_ENTRIES: u64 = 65_536;
/// The longest path of an entry, in bytes (design 38 § 3.3).
pub const MAX_IMAGE_PATH_BYTES: usize = 4_095;
/// The longest component of an entry's path, in bytes (design 38 § 3.3).
pub const MAX_IMAGE_PATH_COMPONENT_BYTES: usize = 255;
/// The longest target text of a symbolic link, in bytes (design 38 § 3.3).
pub const MAX_IMAGE_LINK_TARGET_BYTES: usize = 4_095;
/// The longest pax extended header the reader takes, in bytes. It holds `path`, `linkpath` and
/// `size` records, each bounded above, and the records the unpack ignores.
pub const MAX_IMAGE_PAX_HEADER_BYTES: u64 = 65_536;

/// The three bounds a deployment may lower (design 38 § 3.3). A value above its constant counts
/// as the constant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImageLimits {
    pub max_archive_bytes: u64,
    pub max_unpacked_bytes: u64,
    pub max_entries: u64,
}

impl Default for ImageLimits {
    fn default() -> Self {
        Self {
            max_archive_bytes: MAX_IMAGE_ARCHIVE_BYTES,
            max_unpacked_bytes: MAX_IMAGE_UNPACKED_BYTES,
            max_entries: MAX_IMAGE_ENTRIES,
        }
    }
}

impl ImageLimits {
    /// The limits the store applies: each value at most its constant.
    #[must_use]
    pub fn effective(self) -> Self {
        Self {
            max_archive_bytes: self.max_archive_bytes.min(MAX_IMAGE_ARCHIVE_BYTES),
            max_unpacked_bytes: self.max_unpacked_bytes.min(MAX_IMAGE_UNPACKED_BYTES),
            max_entries: self.max_entries.min(MAX_IMAGE_ENTRIES),
        }
    }
}

/// The bound an `image.archive-limit` refusal names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageBound {
    ArchiveBytes,
    UnpackedBytes,
    Entries,
}

impl fmt::Display for ImageBound {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::ArchiveBytes => "max_archive_bytes",
            Self::UnpackedBytes => "max_unpacked_bytes",
            Self::Entries => "max_entries",
        })
    }
}

/// The kind of an archive entry the unpack refuses to create.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefusedKind {
    CharacterDevice,
    BlockDevice,
    Fifo,
    Sparse,
    /// Any other tar type flag, by its byte.
    Other(u8),
}

impl fmt::Display for RefusedKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CharacterDevice => formatter.write_str("character device"),
            Self::BlockDevice => formatter.write_str("block device"),
            Self::Fifo => formatter.write_str("FIFO"),
            Self::Sparse => formatter.write_str("sparse file"),
            Self::Other(flag) => write!(formatter, "type flag {:?}", char::from(*flag)),
        }
    }
}

/// Why one entry is `image.entry-refused`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryRefusal {
    Kind(RefusedKind),
    EmptyPath,
    AbsolutePath,
    NulByte,
    ParentComponent,
    /// A `.` component anywhere but a leading `./`.
    CurrentComponent,
    EmptyComponent,
    ComponentTooLong,
    PathTooLong,
    LinkTargetTooLong,
    LinkTargetEmpty,
    /// The path names an entry the archive already created.
    Duplicate,
    /// A component of the path is a symbolic link the archive created.
    BeneathSymlink,
    /// A component of the path is a file the archive created.
    BeneathFile,
    /// A hard link whose target is not a regular file the archive already unpacked.
    HardLinkTarget,
    /// The root (`.` or `./`) as anything but a directory.
    RootNotDirectory,
}

impl fmt::Display for EntryRefusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Kind(kind) => write!(formatter, "the entry is a {kind}"),
            Self::EmptyPath => formatter.write_str("the path is empty"),
            Self::AbsolutePath => formatter.write_str("the path is absolute"),
            Self::NulByte => formatter.write_str("the path holds a NUL byte"),
            Self::ParentComponent => formatter.write_str("the path holds a `..` component"),
            Self::CurrentComponent => formatter.write_str("the path holds a `.` component"),
            Self::EmptyComponent => formatter.write_str("the path holds an empty component"),
            Self::ComponentTooLong => write!(
                formatter,
                "a component is longer than {MAX_IMAGE_PATH_COMPONENT_BYTES} bytes"
            ),
            Self::PathTooLong => write!(
                formatter,
                "the path is longer than MAX_IMAGE_PATH_BYTES ({MAX_IMAGE_PATH_BYTES})"
            ),
            Self::LinkTargetTooLong => write!(
                formatter,
                "the link target is longer than MAX_IMAGE_LINK_TARGET_BYTES \
                 ({MAX_IMAGE_LINK_TARGET_BYTES})"
            ),
            Self::LinkTargetEmpty => formatter.write_str("the link target is empty"),
            Self::Duplicate => formatter.write_str("the path is named twice"),
            Self::BeneathSymlink => formatter.write_str("the path is beneath a symbolic link"),
            Self::BeneathFile => formatter.write_str("the path is beneath a file"),
            Self::HardLinkTarget => formatter
                .write_str("the hard link names no regular file the archive already unpacked"),
            Self::RootNotDirectory => formatter.write_str("the root entry is not a directory"),
        }
    }
}

/// What a store operation answers when it does not succeed. [`ImageError::code`] is the refusal
/// code design 38 § 3.6 names.
#[derive(Debug, thiserror::Error)]
pub enum ImageError {
    #[error("image.digest-invalid: the digest is not `sha256:` and 64 lower-case hex")]
    DigestInvalid,
    #[error("image.size-mismatch: {received} bytes were received, {expected} were declared")]
    SizeMismatch { expected: u64, received: u64 },
    #[error("image.digest-mismatch: the archive's digest is {observed}")]
    DigestMismatch {
        /// `sha256:<hex>` of the bytes received.
        observed: String,
    },
    #[error("image.archive-invalid: {reason}")]
    ArchiveInvalid { reason: String },
    #[error("image.entry-refused: entry {entry:?}: {refusal}")]
    EntryRefused {
        /// The entry's path as the archive names it, never a host path.
        entry: String,
        refusal: EntryRefusal,
    },
    #[error("image.archive-limit: the archive exceeds {bound} ({limit})")]
    ArchiveLimit { bound: ImageBound, limit: u64 },
    #[error("image.not-found: the store holds no tree of this digest")]
    NotFound,
    #[error("image.in-use: the tree is held by a run")]
    InUse,
    #[error("image.store-invalid: {reason}")]
    StoreInvalid { reason: String },
    #[error("image.store-failed: {operation}: {source}")]
    Io {
        operation: &'static str,
        source: std::io::Error,
    },
}

impl ImageError {
    /// The refusal code of this answer.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::DigestInvalid => "image.digest-invalid",
            Self::SizeMismatch { .. } => "image.size-mismatch",
            Self::DigestMismatch { .. } => "image.digest-mismatch",
            Self::ArchiveInvalid { .. } => "image.archive-invalid",
            Self::EntryRefused { .. } => "image.entry-refused",
            Self::ArchiveLimit { .. } => "image.archive-limit",
            Self::NotFound => "image.not-found",
            Self::InUse => "image.in-use",
            Self::StoreInvalid { .. } => "image.store-invalid",
            Self::Io { .. } => "image.store-failed",
        }
    }

    pub(crate) fn io(operation: &'static str, source: std::io::Error) -> Self {
        Self::Io { operation, source }
    }

    pub(crate) fn invalid(reason: impl Into<String>) -> Self {
        Self::ArchiveInvalid {
            reason: reason.into(),
        }
    }

    pub(crate) fn refused(entry: &[u8], refusal: EntryRefusal) -> Self {
        Self::EntryRefused {
            entry: String::from_utf8_lossy(entry).into_owned(),
            refusal,
        }
    }
}

/// The 64 lower-case hex characters of a `sha256:<hex>` digest.
///
/// # Errors
///
/// [`ImageError::DigestInvalid`] for any other text.
pub fn digest_hex(digest: &str) -> Result<&str, ImageError> {
    let hex = digest
        .strip_prefix("sha256:")
        .ok_or(ImageError::DigestInvalid)?;
    if hex.len() == 64
        && hex
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        Ok(hex)
    } else {
        Err(ImageError::DigestInvalid)
    }
}
