//! The owner-private, content-addressed store of unpacked trees (design 38 § 3.4).
//!
//! Layout beneath the directory the operator names: the marker `.substrate-image-store`, written
//! when the store is first opened on an empty directory; `staging/<ulid>/` holds one upload's
//! `archive`; `trees/.unpack-<ulid>/` is the tree of that upload being unpacked, once its digest
//! matched; `trees/<sha256 hex>/` is a published tree; `trash/<ulid>/` is a tree being removed.
//!
//! A tree gets every final mode, its root's included, and then is published by one `renameat2`
//! with `RENAME_NOREPLACE` within `trees/`, so a reader sees a whole, read-only tree or none, and
//! nothing after the rename can fail. The tree is unpacked in `trees/` and not in `staging/`
//! because the kernel moves a directory to another parent only when the directory grants its
//! owner write, which a finished root does not. A second upload of a digest already published
//! drops its own tree.
//!
//! Opening the store refuses a directory that holds anything but the marker and the three areas,
//! and areas holding anything but these names, before it creates or removes anything; then it
//! removes every upload, unpack and removal an earlier process left.

use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::fs::File;
use std::io::{BufRead as _, BufReader, Read, Seek as _, SeekFrom, Write as _};
use std::os::fd::{AsFd as _, BorrowedFd, FromRawFd as _, IntoRawFd as _, OwnedFd};
use std::os::unix::fs::OpenOptionsExt as _;
use std::path::Path;
use std::sync::Arc;

use nix::errno::Errno;
use nix::fcntl::AtFlags;
use nix::sys::stat::{FchmodatFlags, Mode, SFlag};
use nix::unistd::UnlinkatFlags;
use parking_lot::Mutex;
use sha2::{Digest as _, Sha256};
use substrate_guarded::fs::openat2_beneath;
use ulid::Ulid;

use super::unpack::{directory_mode, unpack};
use super::{ImageBound, ImageError, ImageLimits, MAX_IMAGE_ENTRIES, digest_hex};
use crate::rename::rename_noreplace_at;

const DIRECTORY_FLAGS: i32 =
    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC;
const PATH_FLAGS: i32 = libc::O_PATH | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC;
const ARCHIVE: &CStr = c"archive";
/// The file the store writes into the empty directory it is first opened on.
const MARKER: &CStr = c".substrate-image-store";
/// The prefix of a tree being unpacked in `trees/`, before the ULID of its upload.
const UNPACK_PREFIX: &str = ".unpack-";
/// The mode of a published tree's root when the archive names no root entry.
const ROOT_MODE: u32 = 0o555;
/// The most entries one listing of `staging/`, `trash/` or `trees/` takes when the store opens.
const MAX_STORE_LISTING: usize = 4_096;
/// The most objects one removal takes: a tree's entries, its root and an upload's archive.
const MAX_REMOVAL_ITEMS: u64 = MAX_IMAGE_ENTRIES + 16;
/// The bytes one read of an archive copies into an upload.
const COPY_BUFFER_BYTES: usize = 64 * 1024;

/// The store. Clones share one store and one set of holds.
#[derive(Clone)]
pub struct ImageStore {
    inner: Arc<Inner>,
}

struct Inner {
    staging: OwnedFd,
    trees: OwnedFd,
    trash: OwnedFd,
    limits: ImageLimits,
    /// Holds by tree, keyed by the digest's hex.
    holds: Mutex<HashMap<String, u64>>,
}

/// What an unpack created; absent for a tree that was published before.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnpackSummary {
    /// Entries created, directories made for entries beneath them included.
    pub entries: u64,
    /// The bytes of the regular files written.
    pub unpacked_bytes: u64,
    /// The bytes the gzip stream inflated to, which the unpacked-bytes bound counts.
    pub inflated_bytes: u64,
}

/// A published tree.
#[derive(Debug)]
pub struct ImageTree {
    /// `sha256:<hex>` of the archive, as observed.
    pub digest: String,
    /// The archive bytes received.
    pub archive_bytes: u64,
    /// `None` when the tree was already published and this upload reused it.
    pub unpacked: Option<UnpackSummary>,
    /// The tree's root, opened `O_PATH` without following a link.
    pub root: OwnedFd,
}

/// One upload: bytes appended into `staging/<ulid>/archive` while their digest is computed.
/// Dropped without [`ImageUpload::complete`], it removes its staging directory.
pub struct ImageUpload {
    store: ImageStore,
    name: CString,
    /// `.unpack-<ulid>`, the name of the tree being unpacked in `trees/`.
    unpack_name: CString,
    archive: File,
    hasher: Sha256,
    received: u64,
}

/// A run's hold on a tree: while one exists, [`ImageStore::delete`] answers
/// [`ImageError::InUse`]. Dropping it releases the hold.
pub struct ImageHold {
    store: ImageStore,
    hex: String,
    root: OwnedFd,
}

impl ImageHold {
    /// The held tree's root, opened `O_PATH` without following a link, for a bind by descriptor.
    #[must_use]
    pub fn root(&self) -> BorrowedFd<'_> {
        self.root.as_fd()
    }

    /// `sha256:<hex>` of the held tree.
    #[must_use]
    pub fn digest(&self) -> String {
        format!("sha256:{}", self.hex)
    }
}

impl Drop for ImageHold {
    fn drop(&mut self) {
        let mut holds = self.store.inner.holds.lock();
        if let Some(count) = holds.get_mut(&self.hex) {
            *count -= 1;
            if *count == 0 {
                holds.remove(&self.hex);
            }
        }
    }
}

impl ImageStore {
    /// Opens the store at `path`, a directory owned by the effective uid that grants group and
    /// other nothing, creates `staging/`, `trees/` and `trash/` beneath it where absent, and
    /// removes every upload and removal an earlier process left there.
    ///
    /// # Errors
    ///
    /// [`ImageError::StoreInvalid`] for a directory of another owner, one that grants group or
    /// other a permission, or an entry beneath it the store did not make;
    /// [`ImageError::Io`] for a failed open, creation or removal.
    pub fn open(path: &Path, limits: ImageLimits) -> Result<Self, ImageError> {
        let root: OwnedFd = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(path)
            .map_err(|error| ImageError::io("opening the image store", error))?
            .into();
        check_private(root.as_fd(), "the image store")?;
        claim_root(root.as_fd())?;
        let staging = open_area(root.as_fd(), c"staging")?;
        let trees = open_area(root.as_fd(), c"trees")?;
        let trash = open_area(root.as_fd(), c"trash")?;
        // Every entry of the three areas is judged before any is removed.
        let staging_left = left_over(staging.as_fd(), "staging", is_ulid)?;
        let trash_left = left_over(trash.as_fd(), "trash", is_ulid)?;
        let trees_left = left_over(trees.as_fd(), "trees", |name| {
            if digest_hex(&format!("sha256:{name}")).is_ok() {
                Some(false)
            } else {
                name.strip_prefix(UNPACK_PREFIX)
                    .filter(|ulid| is_ulid(ulid) == Some(true))
                    .map(|_| true)
            }
        })?;
        for (area, names) in [
            (staging.as_fd(), staging_left),
            (trash.as_fd(), trash_left),
            (trees.as_fd(), trees_left),
        ] {
            for name in names {
                remove_entry(area, &name)?;
            }
        }
        Ok(Self {
            inner: Arc::new(Inner {
                staging,
                trees,
                trash,
                limits: limits.effective(),
                holds: Mutex::new(HashMap::new()),
            }),
        })
    }

    /// The limits this store applies.
    #[must_use]
    pub fn limits(&self) -> ImageLimits {
        self.inner.limits
    }

    /// Starts an upload in a fresh staging directory.
    ///
    /// # Errors
    ///
    /// [`ImageError::Io`] when the staging directory or its archive file cannot be created.
    pub fn upload(&self) -> Result<ImageUpload, ImageError> {
        let name = fresh_name();
        let staging = self.inner.staging.as_fd();
        nix::sys::stat::mkdirat(staging, name.as_c_str(), Mode::from_bits_truncate(0o700))
            .map_err(|errno| ImageError::io("creating a staging directory", errno.into()))?;
        let created = (|| {
            let directory = openat2_beneath(staging, &name, DIRECTORY_FLAGS, 0)?;
            let archive = openat2_beneath(
                directory.as_fd(),
                ARCHIVE,
                libc::O_RDWR | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                0o600,
            )?;
            Ok::<_, std::io::Error>(File::from(archive))
        })();
        match created {
            Ok(archive) => Ok(ImageUpload {
                store: self.clone(),
                unpack_name: unpack_name(&name),
                name,
                archive,
                hasher: Sha256::new(),
                received: 0,
            }),
            Err(error) => {
                drop(remove_entry(staging, &name));
                Err(ImageError::io("creating a staging archive", error))
            }
        }
    }

    /// Receives `archive` whole and completes it against `expected_digest`: [`Self::upload`],
    /// [`ImageUpload::append`] for every read and [`ImageUpload::complete`] with the size
    /// received.
    ///
    /// # Errors
    ///
    /// As [`ImageUpload::append`] and [`ImageUpload::complete`]; a failed read of `archive` is
    /// [`ImageError::Io`] and leaves nothing.
    pub fn create(
        &self,
        expected_digest: &str,
        mut archive: impl Read,
    ) -> Result<ImageTree, ImageError> {
        digest_hex(expected_digest)?;
        let mut upload = self.upload()?;
        let mut buffer = vec![0_u8; COPY_BUFFER_BYTES];
        loop {
            let read = match archive.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => read,
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(ImageError::io("reading the archive", error)),
            };
            upload.append(&buffer[..read])?;
        }
        let size = upload.received_bytes();
        upload.complete(expected_digest, size)
    }

    /// The published tree of `digest`, held until the hold is dropped.
    ///
    /// # Errors
    ///
    /// [`ImageError::DigestInvalid`] for a malformed digest; [`ImageError::NotFound`] when no
    /// tree of it is published.
    pub fn hold(&self, digest: &str) -> Result<ImageHold, ImageError> {
        let hex = digest_hex(digest)?.to_owned();
        let mut holds = self.inner.holds.lock();
        let root = self.open_tree(&hex)?.ok_or(ImageError::NotFound)?;
        *holds.entry(hex.clone()).or_insert(0) += 1;
        Ok(ImageHold {
            store: self.clone(),
            hex,
            root,
        })
    }

    /// Whether a run holds the tree of `digest`.
    ///
    /// # Errors
    ///
    /// [`ImageError::DigestInvalid`] for a malformed digest.
    pub fn is_held(&self, digest: &str) -> Result<bool, ImageError> {
        let hex = digest_hex(digest)?;
        Ok(self.inner.holds.lock().contains_key(hex))
    }

    /// Whether a tree of `digest` is published.
    ///
    /// # Errors
    ///
    /// [`ImageError::DigestInvalid`] for a malformed digest; [`ImageError::Io`] for a failed
    /// open.
    pub fn contains(&self, digest: &str) -> Result<bool, ImageError> {
        Ok(self.open_tree(digest_hex(digest)?)?.is_some())
    }

    /// Removes the published tree of `digest`: renamed into `trash/`, then removed by a bounded
    /// walk that follows no link, its write bits restored as it goes.
    ///
    /// # Errors
    ///
    /// [`ImageError::InUse`] while a hold exists; [`ImageError::NotFound`] when no tree of it is
    /// published; [`ImageError::Io`] for a failed rename or removal, which a later open of the
    /// store finishes.
    pub fn delete(&self, digest: &str) -> Result<(), ImageError> {
        let hex = digest_hex(digest)?;
        let name = hex_name(hex);
        let trashed = fresh_name();
        {
            let holds = self.inner.holds.lock();
            if holds.contains_key(hex) {
                return Err(ImageError::InUse);
            }
            if self.open_tree(hex)?.is_none() {
                return Err(ImageError::NotFound);
            }
            // Moving a directory to another parent writes its `..` entry.
            nix::sys::stat::fchmodat(
                self.inner.trees.as_fd(),
                name.as_c_str(),
                Mode::from_bits_truncate(0o700),
                FchmodatFlags::FollowSymlink,
            )
            .map_err(|errno| ImageError::io("making a tree writable", errno.into()))?;
            rename_noreplace_at(
                self.inner.trees.as_fd(),
                &name,
                self.inner.trash.as_fd(),
                &trashed,
            )
            .map_err(|errno| ImageError::io("moving a tree to the trash", errno.into()))?;
        }
        sync(self.inner.trees.as_fd())?;
        remove_entry(self.inner.trash.as_fd(), &trashed)
    }

    /// The published tree `hex`, opened `O_PATH`, or `None`.
    fn open_tree(&self, hex: &str) -> Result<Option<OwnedFd>, ImageError> {
        let name = hex_name(hex);
        match openat2_beneath(self.inner.trees.as_fd(), &name, PATH_FLAGS, 0) {
            Ok(root) => Ok(Some(root)),
            Err(error) if error.raw_os_error() == Some(libc::ENOENT) => Ok(None),
            Err(error) => Err(ImageError::io("opening a tree", error)),
        }
    }
}

/// `.unpack-<ulid>` for the upload `staging/<ulid>`.
fn unpack_name(upload: &CStr) -> CString {
    let mut name = UNPACK_PREFIX.as_bytes().to_vec();
    name.extend_from_slice(upload.to_bytes());
    CString::new(name).expect("a ULID holds no NUL")
}

/// A fresh, well-formed name for an entry of `staging/` or `trash/`.
fn fresh_name() -> CString {
    CString::new(Ulid::generate().to_string()).expect("a ULID holds no NUL")
}

/// The name of a tree in `trees/`; the caller checked `hex` with [`digest_hex`].
fn hex_name(hex: &str) -> CString {
    CString::new(hex).expect("hex holds no NUL")
}

/// `Some(true)` for a ULID in its canonical form, which names an entry of `staging/` or `trash/`
/// that an earlier process left; `None` for any other name.
fn is_ulid(name: &str) -> Option<bool> {
    Ulid::from_string(name)
        .is_ok_and(|ulid| ulid.to_string() == name)
        .then_some(true)
}

/// The entries of an area to remove when the store opens. Every entry must be a directory whose
/// name `judge` knows: `Some(true)` is left over and removed, `Some(false)` is kept. Anything else
/// refuses the open before anything is removed.
fn left_over(
    area: BorrowedFd<'_>,
    label: &str,
    judge: impl Fn(&str) -> Option<bool>,
) -> Result<Vec<CString>, ImageError> {
    let mut remove = Vec::new();
    for (name, kind) in list(area, MAX_STORE_LISTING, label)? {
        let verdict = name.to_str().ok().and_then(&judge);
        match verdict {
            Some(true) if kind == SFlag::S_IFDIR => remove.push(name),
            Some(false) if kind == SFlag::S_IFDIR => {}
            _ => {
                return Err(ImageError::StoreInvalid {
                    reason: format!("{label}/ holds an entry the store did not make: {name:?}"),
                });
            }
        }
    }
    Ok(remove)
}

/// Claims the store's root before anything is created or removed in it. An empty directory gets
/// the marker [`MARKER`]; otherwise the root must hold the marker as a regular file and nothing
/// but it and the directories `staging/`, `trees/` and `trash/`. So the store never creates its
/// areas in, nor removes anything from, a directory it did not make.
fn claim_root(root: BorrowedFd<'_>) -> Result<(), ImageError> {
    let entries = list(root, MAX_STORE_LISTING, "the image store")?;
    if entries.is_empty() {
        let marker = openat2_beneath(
            root,
            MARKER,
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            0o600,
        )
        .map_err(|error| ImageError::io("marking the image store", error))?;
        nix::unistd::fsync(&marker)
            .map_err(|errno| ImageError::io("marking the image store", errno.into()))?;
        return sync(root);
    }
    let mut marked = false;
    for (name, kind) in &entries {
        let known = match name.as_bytes() {
            b"staging" | b"trees" | b"trash" => *kind == SFlag::S_IFDIR,
            _ if name.as_c_str() == MARKER => {
                marked = *kind == SFlag::S_IFREG;
                marked
            }
            _ => false,
        };
        if !known {
            return Err(ImageError::StoreInvalid {
                reason: format!("the image store holds an entry the store did not make: {name:?}"),
            });
        }
    }
    if !marked {
        return Err(ImageError::StoreInvalid {
            reason: "the image store's directory is not empty and holds no store marker".to_owned(),
        });
    }
    Ok(())
}

impl ImageUpload {
    /// Appends `bytes` to the archive and answers the bytes received.
    ///
    /// # Errors
    ///
    /// [`ImageError::ArchiveLimit`] naming [`ImageBound::ArchiveBytes`] past the store's bound,
    /// which ends the upload; [`ImageError::Io`] for a failed write.
    pub fn append(&mut self, bytes: &[u8]) -> Result<u64, ImageError> {
        let received = self.received.saturating_add(bytes.len() as u64);
        let limit = self.store.inner.limits.max_archive_bytes;
        if received > limit {
            return Err(ImageError::ArchiveLimit {
                bound: ImageBound::ArchiveBytes,
                limit,
            });
        }
        self.archive
            .write_all(bytes)
            .map_err(|error| ImageError::io("writing the staging archive", error))?;
        self.hasher.update(bytes);
        self.received = received;
        Ok(received)
    }

    /// The archive bytes received.
    #[must_use]
    pub fn received_bytes(&self) -> u64 {
        self.received
    }

    /// Compares the size and then the digest of the bytes received, before anything is parsed;
    /// unpacks the archive into `trees/.unpack-<ulid>`; gives it its final modes; and publishes it
    /// as `trees/<hex>`, read-only. A digest already published is reused and the upload's tree
    /// dropped. The staging directory, and the unpacked tree where it is not published, are
    /// removed on every path.
    ///
    /// # Errors
    ///
    /// [`ImageError::DigestInvalid`], [`ImageError::SizeMismatch`] and
    /// [`ImageError::DigestMismatch`] before anything is parsed; [`ImageError::ArchiveInvalid`],
    /// [`ImageError::EntryRefused`] and [`ImageError::ArchiveLimit`] from the unpack;
    /// [`ImageError::Io`] for a failed write, rename or removal. None of them publishes a tree.
    pub fn complete(
        self,
        expected_digest: &str,
        expected_size: u64,
    ) -> Result<ImageTree, ImageError> {
        let result = self.complete_staged(expected_digest, expected_size);
        drop(self);
        result
    }

    fn complete_staged(
        &self,
        expected_digest: &str,
        expected_size: u64,
    ) -> Result<ImageTree, ImageError> {
        let expected = digest_hex(expected_digest)?;
        if self.received != expected_size {
            return Err(ImageError::SizeMismatch {
                expected: expected_size,
                received: self.received,
            });
        }
        let observed = hex::encode(self.hasher.clone().finalize());
        if observed != expected {
            return Err(ImageError::DigestMismatch {
                observed: format!("sha256:{observed}"),
            });
        }
        let digest = format!("sha256:{observed}");
        let inner = &self.store.inner;
        if let Some(root) = self.store.open_tree(&observed)? {
            return Ok(ImageTree {
                digest,
                archive_bytes: self.received,
                unpacked: None,
                root,
            });
        }
        // The tree is unpacked in `trees/` itself: the kernel moves a directory to another
        // parent only when the directory grants its owner write, and the root of a finished
        // tree does not.
        let trees = inner.trees.as_fd();
        nix::sys::stat::mkdirat(
            trees,
            self.unpack_name.as_c_str(),
            Mode::from_bits_truncate(0o700),
        )
        .map_err(|errno| ImageError::io("creating a staging tree", errno.into()))?;
        let tree = openat2_beneath(trees, &self.unpack_name, DIRECTORY_FLAGS, 0)
            .map_err(|error| ImageError::io("opening a staging tree", error))?;

        let mut archive = self
            .archive
            .try_clone()
            .map_err(|error| ImageError::io("reading the staging archive", error))?;
        archive
            .seek(SeekFrom::Start(0))
            .map_err(|error| ImageError::io("reading the staging archive", error))?;
        let decoder = flate2::bufread::GzDecoder::new(BufReader::new(archive));
        let (unpacked, decoder) = unpack(tree.as_fd(), decoder, inner.limits)?;
        let mut rest = decoder.into_inner();
        if !rest
            .fill_buf()
            .map_err(super::tar::stream_error)?
            .is_empty()
        {
            return Err(ImageError::invalid("bytes follow the gzip stream"));
        }

        // Every mode, the root's included, is final before the tree is published, and nothing
        // after the rename can fail.
        let root_mode = unpacked.root_mode.map_or(ROOT_MODE, directory_mode);
        nix::sys::stat::fchmod(&tree, Mode::from_bits_truncate(root_mode))
            .map_err(|errno| ImageError::io("setting a tree's mode", errno.into()))?;
        nix::unistd::syncfs(&tree)
            .map_err(|errno| ImageError::io("syncing the staging tree", errno.into()))?;
        let root = openat2_beneath(trees, &self.unpack_name, PATH_FLAGS, 0)
            .map_err(|error| ImageError::io("opening a staging tree", error))?;
        let published = hex_name(&observed);
        match rename_noreplace_at(trees, &self.unpack_name, trees, &published) {
            Ok(()) => {}
            // Another upload of the same bytes published first.
            Err(Errno::EEXIST | Errno::ENOTEMPTY) => {
                let root = self
                    .store
                    .open_tree(&observed)?
                    .ok_or(ImageError::NotFound)?;
                return Ok(ImageTree {
                    digest,
                    archive_bytes: self.received,
                    unpacked: None,
                    root,
                });
            }
            Err(errno) => return Err(ImageError::io("publishing a tree", errno.into())),
        }
        // Best effort: the tree is whole and final. A rename the disk loses leaves an unpacked
        // tree under its upload's name, which the next open removes, and nothing published.
        let _ = nix::unistd::fsync(trees);
        Ok(ImageTree {
            digest,
            archive_bytes: self.received,
            unpacked: Some(UnpackSummary {
                entries: unpacked.entries,
                unpacked_bytes: unpacked.content_bytes,
                inflated_bytes: unpacked.inflated_bytes,
            }),
            root,
        })
    }
}

impl Drop for ImageUpload {
    fn drop(&mut self) {
        // Best effort: what is left is removed when the store next opens.
        drop(remove_entry(self.store.inner.staging.as_fd(), &self.name));
        drop(remove_entry(
            self.store.inner.trees.as_fd(),
            &self.unpack_name,
        ));
    }
}

/// Refuses a directory of another owner or one that grants group or other a permission.
fn check_private(directory: BorrowedFd<'_>, label: &str) -> Result<(), ImageError> {
    let stat = nix::sys::stat::fstat(directory)
        .map_err(|errno| ImageError::io("observing the image store", errno.into()))?;
    if stat.st_uid != nix::unistd::geteuid().as_raw() {
        return Err(ImageError::StoreInvalid {
            reason: format!("{label} is not owned by the daemon's uid"),
        });
    }
    if stat.st_mode & 0o077 != 0 {
        return Err(ImageError::StoreInvalid {
            reason: format!("{label} grants group or other a permission"),
        });
    }
    Ok(())
}

/// Opens, creating it where absent, one of the store's three areas.
fn open_area(root: BorrowedFd<'_>, name: &CStr) -> Result<OwnedFd, ImageError> {
    match nix::sys::stat::mkdirat(root, name, Mode::from_bits_truncate(0o700)) {
        Ok(()) | Err(Errno::EEXIST) => {}
        Err(errno) => return Err(ImageError::io("creating a store area", errno.into())),
    }
    let area = openat2_beneath(root, name, DIRECTORY_FLAGS, 0).map_err(|error| {
        if error.raw_os_error() == Some(libc::ELOOP) || error.raw_os_error() == Some(libc::ENOTDIR)
        {
            ImageError::StoreInvalid {
                reason: format!("{name:?} in the image store is not a directory"),
            }
        } else {
            ImageError::io("opening a store area", error)
        }
    })?;
    check_private(area.as_fd(), &format!("{name:?} in the image store"))?;
    Ok(area)
}

fn sync(directory: BorrowedFd<'_>) -> Result<(), ImageError> {
    nix::unistd::fsync(directory).map_err(|errno| ImageError::io("syncing the store", errno.into()))
}

/// The entries of `directory`, at most `limit`, with each one's type read without following a
/// link.
fn list(
    directory: BorrowedFd<'_>,
    limit: usize,
    label: &str,
) -> Result<Vec<(CString, SFlag)>, ImageError> {
    let failed = |error| ImageError::io("listing a directory", error);
    // A fresh open file description, so this listing's offset is its own.
    let scan = openat2_beneath(directory, c".", DIRECTORY_FLAGS, 0).map_err(failed)?;
    let raw = scan.into_raw_fd();
    // SAFETY: ownership of the fresh descriptor transfers to the stream.
    let stream = unsafe { libc::fdopendir(raw) };
    if stream.is_null() {
        let error = std::io::Error::last_os_error();
        // SAFETY: fdopendir did not take the descriptor on failure.
        drop(unsafe { OwnedFd::from_raw_fd(raw) });
        return Err(failed(error));
    }
    let stream = DirectoryStream(stream);
    let mut names = Vec::new();
    loop {
        // SAFETY: the stream is live for the guard's lifetime.
        let entry = unsafe { libc::readdir(stream.0) };
        if entry.is_null() {
            break;
        }
        // SAFETY: readdir returns a dirent whose name is NUL-terminated.
        let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) };
        if matches!(name.to_bytes(), b"." | b"..") {
            continue;
        }
        if names.len() == limit {
            return Err(ImageError::StoreInvalid {
                reason: format!("{label}/ holds more than {limit} entries"),
            });
        }
        let name = name.to_owned();
        let stat =
            nix::sys::stat::fstatat(directory, name.as_c_str(), AtFlags::AT_SYMLINK_NOFOLLOW)
                .map_err(|errno| failed(errno.into()))?;
        names.push((
            name,
            SFlag::from_bits_truncate(stat.st_mode) & SFlag::S_IFMT,
        ));
    }
    Ok(names)
}

struct DirectoryStream(*mut libc::DIR);

impl Drop for DirectoryStream {
    fn drop(&mut self) {
        // SAFETY: the stream was made by fdopendir and is owned by this guard.
        unsafe { libc::closedir(self.0) };
    }
}

/// Removes the entry `name` of `parent` and everything beneath it, following no link.
///
/// The walk keeps every path one component deep: each directory beneath the removal's root is
/// moved up to the root under a fresh name before it is emptied, so no path grows past a
/// component and no descriptor is held per level. A directory is given its owner's write and
/// search bits before it is read, because a published tree is read-only. At most
/// [`MAX_REMOVAL_ITEMS`] objects are taken; past that the removal stops, and a later open of the
/// store finishes it.
fn remove_entry(parent: BorrowedFd<'_>, name: &CStr) -> Result<(), ImageError> {
    let failed = |errno: Errno| ImageError::io("removing a tree", errno.into());
    let stat = match nix::sys::stat::fstatat(parent, name, AtFlags::AT_SYMLINK_NOFOLLOW) {
        Ok(stat) => stat,
        Err(Errno::ENOENT) => return Ok(()),
        Err(errno) => return Err(failed(errno)),
    };
    if SFlag::from_bits_truncate(stat.st_mode) & SFlag::S_IFMT != SFlag::S_IFDIR {
        return nix::unistd::unlinkat(parent, name, UnlinkatFlags::NoRemoveDir).map_err(failed);
    }
    make_writable(parent, name)?;
    let root = openat2_beneath(parent, name, DIRECTORY_FLAGS, 0)
        .map_err(|error| ImageError::io("removing a tree", error))?;
    let mut items = 0_u64;
    let mut hoisted = 0_u64;
    loop {
        let entries = list(root.as_fd(), usize::MAX, "a removal")?;
        if entries.is_empty() {
            break;
        }
        for (child, kind) in entries {
            items += 1;
            if items > MAX_REMOVAL_ITEMS {
                return Err(ImageError::Io {
                    operation: "removing a tree",
                    source: std::io::Error::other("the tree exceeds the removal bound"),
                });
            }
            if kind != SFlag::S_IFDIR {
                nix::unistd::unlinkat(root.as_fd(), child.as_c_str(), UnlinkatFlags::NoRemoveDir)
                    .map_err(failed)?;
                continue;
            }
            make_writable(root.as_fd(), &child)?;
            let directory = openat2_beneath(root.as_fd(), &child, DIRECTORY_FLAGS, 0)
                .map_err(|error| ImageError::io("removing a tree", error))?;
            for (grandchild, grandkind) in list(directory.as_fd(), usize::MAX, "a removal")? {
                if grandkind != SFlag::S_IFDIR {
                    items += 1;
                    nix::unistd::unlinkat(
                        directory.as_fd(),
                        grandchild.as_c_str(),
                        UnlinkatFlags::NoRemoveDir,
                    )
                    .map_err(failed)?;
                    continue;
                }
                make_writable(directory.as_fd(), &grandchild)?;
                loop {
                    hoisted += 1;
                    let fresh = CString::new(format!(".removal-{hoisted}")).expect("no NUL");
                    match rename_noreplace_at(directory.as_fd(), &grandchild, root.as_fd(), &fresh)
                    {
                        Ok(()) => break,
                        Err(Errno::EEXIST | Errno::ENOTEMPTY) => {}
                        Err(errno) => return Err(failed(errno)),
                    }
                }
            }
            nix::unistd::unlinkat(root.as_fd(), child.as_c_str(), UnlinkatFlags::RemoveDir)
                .map_err(failed)?;
        }
    }
    drop(root);
    nix::unistd::unlinkat(parent, name, UnlinkatFlags::RemoveDir).map_err(failed)?;
    sync(parent)
}

/// Gives the directory `name` of `parent` its owner's read, write and search bits. The caller
/// read it as a directory without following a link, in a directory only the store's owner
/// writes.
fn make_writable(parent: BorrowedFd<'_>, name: &CStr) -> Result<(), ImageError> {
    nix::sys::stat::fchmodat(
        parent,
        name,
        Mode::from_bits_truncate(0o700),
        FchmodatFlags::FollowSymlink,
    )
    .map_err(|errno| ImageError::io("making a directory writable", errno.into()))
}
