//! The unpacker and the store on archives each test builds (design 38 § 12, unit lane).

use std::collections::BTreeMap;
use std::io::Read;
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};

use sha2::Digest as _;

use super::test_archive::{Header, Tar, digest, gzip};
use super::{
    EntryRefusal, ImageBound, ImageError, ImageLimits, ImageStore, MAX_IMAGE_LINK_TARGET_BYTES,
    MAX_IMAGE_PATH_BYTES, RefusedKind,
};

/// A directory holding the store and, beside it, files no unpack may touch.
struct Fixture {
    outer: tempfile::TempDir,
    store: ImageStore,
}

impl Fixture {
    fn new() -> Self {
        Self::with_limits(ImageLimits::default())
    }

    fn with_limits(limits: ImageLimits) -> Self {
        let outer = tempfile::tempdir().expect("temporary directory");
        std::fs::write(outer.path().join("sentinel"), "outside").expect("sentinel");
        std::fs::create_dir(outer.path().join("beside")).expect("beside");
        std::fs::write(outer.path().join("beside/kept"), "kept").expect("kept");
        let store_path = outer.path().join("store");
        std::fs::create_dir(&store_path).expect("store");
        std::fs::set_permissions(&store_path, std::fs::Permissions::from_mode(0o700))
            .expect("store mode");
        let store = ImageStore::open(&store_path, limits).expect("open store");
        Self { outer, store }
    }

    fn store_path(&self) -> PathBuf {
        self.outer.path().join("store")
    }

    fn tree(&self, digest: &str) -> PathBuf {
        self.store_path()
            .join("trees")
            .join(digest.strip_prefix("sha256:").expect("digest"))
    }

    fn create(&self, archive: &[u8]) -> Result<super::ImageTree, ImageError> {
        self.store.create(&digest(archive), archive)
    }

    /// Every path beneath the outer directory but the store, with what it holds.
    fn outside(&self) -> BTreeMap<PathBuf, String> {
        snapshot(self.outer.path(), &self.store_path())
    }

    fn staging_and_trash_are_empty(&self) {
        for area in ["staging", "trash"] {
            let entries: Vec<_> = std::fs::read_dir(self.store_path().join(area))
                .expect(area)
                .collect();
            assert!(entries.is_empty(), "{area}/ holds {entries:?}");
        }
    }

    fn published(&self) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(self.store_path().join("trees"))
            .expect("trees")
            .map(|entry| {
                entry
                    .expect("entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        names.sort();
        names
    }

    /// The archive is refused with `expected`, nothing is published, no staging is left and
    /// nothing outside the store changed.
    fn refused(&self, archive: &[u8]) -> ImageError {
        let before = self.outside();
        let error = self.create(archive).expect_err("refused");
        assert!(
            self.published().is_empty(),
            "published {:?}",
            self.published()
        );
        self.staging_and_trash_are_empty();
        assert_eq!(self.outside(), before, "a path outside the store changed");
        error
    }
}

fn snapshot(root: &Path, skip: &Path) -> BTreeMap<PathBuf, String> {
    let mut seen = BTreeMap::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(&directory).expect("read directory") {
            let path = entry.expect("entry").path();
            if path == skip {
                continue;
            }
            let metadata = std::fs::symlink_metadata(&path).expect("metadata");
            let what = if metadata.file_type().is_symlink() {
                format!("link {:?}", std::fs::read_link(&path).expect("link"))
            } else if metadata.is_dir() {
                pending.push(path.clone());
                format!("directory {:o}", metadata.mode())
            } else {
                format!(
                    "file {:o} {:?} {}",
                    metadata.mode(),
                    std::fs::read(&path).expect("file"),
                    metadata.mtime_nsec()
                )
            };
            seen.insert(path, what);
        }
    }
    seen
}

fn refusal(error: &ImageError) -> EntryRefusal {
    match error {
        ImageError::EntryRefused { refusal, .. } => *refusal,
        other => panic!("not an entry refusal: {other}"),
    }
}

fn os_release() -> Tar {
    let mut tar = Tar::default();
    tar.directory("./", 0o755)
        .directory("./etc/", 0o755)
        .file("./etc/os-release", 0o644, b"ID=fixture\n")
        .directory("./usr/bin/", 0o755)
        .file("./usr/bin/tool", 0o4755, b"#!/bin/true\n")
        .file("./usr/bin/group", 0o2711, b"x")
        .directory("./tmp/", 0o1777)
        .symlink("./bin", "usr/bin")
        .hard_link("./usr/bin/tool-again", "./usr/bin/tool");
    tar
}

#[test]
fn a_correct_digest_unpacks_and_a_file_reads_back() {
    let fixture = Fixture::new();
    let archive = os_release().gzip();
    let tree = fixture.create(&archive).expect("created");
    assert_eq!(tree.digest, digest(&archive));
    assert_eq!(tree.archive_bytes, archive.len() as u64);
    let summary = tree.unpacked.expect("unpacked");
    // `./` and eight entries; `usr/` is made for `usr/bin/`.
    assert_eq!(summary.entries, 10);
    assert_eq!(summary.unpacked_bytes, 11 + 12 + 1);
    let root = fixture.tree(&tree.digest);
    assert_eq!(
        std::fs::read(root.join("etc/os-release")).expect("os-release"),
        b"ID=fixture\n"
    );
    assert_eq!(
        std::fs::read_link(root.join("bin")).expect("link"),
        Path::new("usr/bin")
    );
    let tool = std::fs::metadata(root.join("usr/bin/tool")).expect("tool");
    let again = std::fs::metadata(root.join("usr/bin/tool-again")).expect("hard link");
    assert_eq!(tool.ino(), again.ino(), "the hard link is the same file");
    assert_eq!(
        tool.uid(),
        nix::unistd::geteuid().as_raw(),
        "owned by the store's uid"
    );
    fixture.staging_and_trash_are_empty();
}

#[test]
fn the_published_tree_is_read_only_without_setuid_or_setgid() {
    let fixture = Fixture::new();
    let archive = os_release().gzip();
    let tree = fixture.create(&archive).expect("created");
    let root = fixture.tree(&tree.digest);
    let mode = |path: &str| {
        std::fs::symlink_metadata(root.join(path))
            .expect(path)
            .mode()
            & 0o7777
    };
    assert_eq!(
        mode("usr/bin/tool"),
        0o555,
        "setuid dropped, write bits cleared"
    );
    assert_eq!(mode("usr/bin/group"), 0o511, "setgid dropped");
    assert_eq!(mode("etc/os-release"), 0o444);
    assert_eq!(mode("tmp"), 0o1555, "the sticky bit is kept");
    assert_eq!(mode("usr"), 0o555, "a directory the unpack made");
    assert_eq!(mode("etc"), 0o555);
    assert_eq!(mode(""), 0o555, "the root");
    assert!(std::fs::write(root.join("etc/new"), "x").is_err() || nix::unistd::geteuid().is_root());
}

#[test]
fn a_wrong_digest_is_refused_with_the_observed_digest_and_leaves_nothing() {
    let fixture = Fixture::new();
    let archive = os_release().gzip();
    let other = digest(b"other bytes");
    let before = fixture.outside();
    let error = fixture
        .store
        .create(&other, archive.as_slice())
        .expect_err("refused");
    match &error {
        ImageError::DigestMismatch { observed } => assert_eq!(observed, &digest(&archive)),
        other => panic!("not a digest mismatch: {other}"),
    }
    assert_eq!(error.code(), "image.digest-mismatch");
    assert!(fixture.published().is_empty());
    fixture.staging_and_trash_are_empty();
    assert_eq!(fixture.outside(), before);
}

#[test]
fn a_malformed_digest_is_refused_before_any_byte_is_staged() {
    let fixture = Fixture::new();
    for digest in [
        "sha256:ABC",
        "sha512:00",
        &format!("sha256:{}", "A".repeat(64)),
    ] {
        let error = fixture.store.create(digest, &b""[..]).expect_err("refused");
        assert_eq!(error.code(), "image.digest-invalid", "{digest}");
    }
    fixture.staging_and_trash_are_empty();
}

#[test]
fn a_size_other_than_the_declared_one_is_refused() {
    let fixture = Fixture::new();
    let archive = os_release().gzip();
    let mut upload = fixture.store.upload().expect("upload");
    upload.append(&archive).expect("append");
    let error = upload
        .complete(&digest(&archive), archive.len() as u64 + 1)
        .expect_err("refused");
    assert_eq!(error.code(), "image.size-mismatch");
    assert!(fixture.published().is_empty());
    fixture.staging_and_trash_are_empty();
}

#[test]
fn an_upload_in_chunks_publishes_the_same_tree() {
    let fixture = Fixture::new();
    let archive = os_release().gzip();
    let mut upload = fixture.store.upload().expect("upload");
    for chunk in archive.chunks(100) {
        upload.append(chunk).expect("append");
    }
    assert_eq!(upload.received_bytes(), archive.len() as u64);
    let tree = upload
        .complete(&digest(&archive), archive.len() as u64)
        .expect("created");
    assert!(fixture.tree(&tree.digest).join("etc/os-release").is_file());
}

#[test]
fn a_second_create_of_a_published_digest_reuses_the_tree() {
    let fixture = Fixture::new();
    let archive = os_release().gzip();
    let first = fixture.create(&archive).expect("first");
    assert!(first.unpacked.is_some());
    let inode = std::fs::metadata(fixture.tree(&first.digest).join("etc/os-release"))
        .expect("os-release")
        .ino();
    let second = fixture.create(&archive).expect("second");
    assert_eq!(second.digest, first.digest);
    assert!(
        second.unpacked.is_none(),
        "the second create unpacked again"
    );
    assert_eq!(
        std::fs::metadata(fixture.tree(&first.digest).join("etc/os-release"))
            .expect("os-release")
            .ino(),
        inode
    );
    assert_eq!(fixture.published().len(), 1);
    fixture.staging_and_trash_are_empty();
}

#[test]
fn a_parent_component_is_refused() {
    let fixture = Fixture::new();
    let mut tar = Tar::default();
    tar.file("ok", 0o644, b"x").file("../x", 0o644, b"escape");
    let error = fixture.refused(&tar.gzip());
    assert_eq!(error.code(), "image.entry-refused");
    assert_eq!(refusal(&error), EntryRefusal::ParentComponent);
    assert!(!fixture.outer.path().join("x").exists());
    assert!(!fixture.store_path().join("x").exists());
}

#[test]
fn an_absolute_path_is_refused() {
    let fixture = Fixture::new();
    let target = fixture.outer.path().join("absolute");
    let mut tar = Tar::default();
    tar.file(target.to_str().expect("utf-8"), 0o644, b"escape");
    let error = fixture.refused(&tar.gzip());
    assert_eq!(refusal(&error), EntryRefusal::AbsolutePath);
    assert!(!target.exists());
}

#[test]
fn a_file_beneath_a_symbolic_link_is_refused_and_the_link_is_not_followed() {
    let fixture = Fixture::new();
    let outside = fixture.outer.path().join("beside");
    let mut tar = Tar::default();
    tar.symlink("escape", outside.to_str().expect("utf-8"))
        .file("escape/kept", 0o644, b"overwritten")
        .file("escape/new", 0o644, b"created");
    let error = fixture.refused(&tar.gzip());
    assert_eq!(refusal(&error), EntryRefusal::BeneathSymlink);
    match &error {
        ImageError::EntryRefused { entry, .. } => assert_eq!(entry, "escape/kept"),
        other => panic!("{other}"),
    }
    assert_eq!(std::fs::read(outside.join("kept")).expect("kept"), b"kept");
    assert!(!outside.join("new").exists());
}

#[test]
fn a_relative_symbolic_link_parent_is_refused_too() {
    let fixture = Fixture::new();
    let mut tar = Tar::default();
    tar.symlink("up", "../../..").directory("up/made", 0o755);
    let error = fixture.refused(&tar.gzip());
    assert_eq!(refusal(&error), EntryRefusal::BeneathSymlink);
}

#[test]
fn a_link_pointing_outside_its_root_is_stored_and_never_followed() {
    let fixture = Fixture::new();
    let sentinel = fixture.outer.path().join("sentinel");
    let mut tar = Tar::default();
    tar.symlink("absolute", sentinel.to_str().expect("utf-8"))
        .symlink("relative", "../../../sentinel")
        .directory("dir", 0o755)
        .symlink("dir/up", "../../../../beside");
    let archive = tar.gzip();
    let before = fixture.outside();
    let tree = fixture.create(&archive).expect("created");
    let root = fixture.tree(&tree.digest);
    assert_eq!(
        std::fs::read_link(root.join("absolute")).expect("link"),
        sentinel
    );
    assert_eq!(
        std::fs::read_link(root.join("relative")).expect("link"),
        Path::new("../../../sentinel")
    );
    assert_eq!(
        std::fs::read_link(root.join("dir/up")).expect("link"),
        Path::new("../../../../beside")
    );
    assert_eq!(
        fixture.outside(),
        before,
        "a path outside the store changed"
    );
    assert_eq!(std::fs::read(&sentinel).expect("sentinel"), b"outside");
}

#[test]
fn a_file_over_a_stored_link_is_a_path_named_twice() {
    let fixture = Fixture::new();
    let sentinel = fixture.outer.path().join("sentinel");
    let mut tar = Tar::default();
    tar.symlink("link", sentinel.to_str().expect("utf-8"))
        .file("link", 0o644, b"overwritten");
    let error = fixture.refused(&tar.gzip());
    assert_eq!(refusal(&error), EntryRefusal::Duplicate);
    assert_eq!(std::fs::read(&sentinel).expect("sentinel"), b"outside");
}

#[test]
fn device_nodes_fifos_sparse_files_and_unknown_kinds_are_refused_by_kind() {
    for (flag, kind) in [
        (b'3', RefusedKind::CharacterDevice),
        (b'4', RefusedKind::BlockDevice),
        (b'6', RefusedKind::Fifo),
        (b'S', RefusedKind::Sparse),
        (b'V', RefusedKind::Other(b'V')),
    ] {
        let fixture = Fixture::new();
        let mut tar = Tar::default();
        tar.file("first", 0o644, b"x").special("dev/node", flag);
        let error = fixture.refused(&tar.gzip());
        assert_eq!(
            refusal(&error),
            EntryRefusal::Kind(kind),
            "{}",
            char::from(flag)
        );
        let text = error.to_string();
        assert!(text.contains("\"dev/node\""), "{text}");
        assert!(
            !text.contains(fixture.outer.path().to_str().expect("utf-8")),
            "{text}"
        );
    }
}

#[test]
fn a_pax_sparse_file_is_refused_as_sparse() {
    let fixture = Fixture::new();
    let mut tar = Tar::default();
    tar.pax(&[("GNU.sparse.major", b"1"), ("GNU.sparse.minor", b"0")])
        .file("sparse", 0o644, b"map");
    let error = fixture.refused(&tar.gzip());
    assert_eq!(refusal(&error), EntryRefusal::Kind(RefusedKind::Sparse));
}

#[test]
fn a_hard_link_to_an_entry_not_in_the_archive_is_refused() {
    let fixture = Fixture::new();
    let sentinel = fixture.outer.path().join("sentinel");
    for target in [
        "absent".to_owned(),
        "later".to_owned(),
        sentinel.to_str().expect("utf-8").to_owned(),
        "../sentinel".to_owned(),
        "dir".to_owned(),
        "link".to_owned(),
        "link/kept".to_owned(),
    ] {
        let mut tar = Tar::default();
        tar.directory("dir", 0o755)
            .symlink(
                "link",
                fixture.outer.path().join("beside").to_str().expect("utf-8"),
            )
            .hard_link("hard", &target)
            .file("later", 0o644, b"x");
        let error = fixture.refused(&tar.gzip());
        assert_eq!(refusal(&error), EntryRefusal::HardLinkTarget, "{target}");
    }
    assert_eq!(std::fs::metadata(&sentinel).expect("sentinel").nlink(), 1);
}

#[test]
fn a_path_named_twice_is_refused() {
    let fixture = Fixture::new();
    let mut tar = Tar::default();
    tar.file("a", 0o644, b"1").file("./a", 0o644, b"2");
    assert_eq!(
        refusal(&fixture.refused(&tar.gzip())),
        EntryRefusal::Duplicate
    );
    let mut tar = Tar::default();
    tar.directory("d", 0o755).directory("d/", 0o700);
    assert_eq!(
        refusal(&fixture.refused(&tar.gzip())),
        EntryRefusal::Duplicate
    );
    let mut tar = Tar::default();
    tar.file("f", 0o644, b"1").file("f/beneath", 0o644, b"2");
    assert_eq!(
        refusal(&fixture.refused(&tar.gzip())),
        EntryRefusal::BeneathFile
    );
}

#[test]
fn a_directory_named_after_its_entries_takes_its_own_mode() {
    let fixture = Fixture::new();
    let mut tar = Tar::default();
    tar.file("late/file", 0o644, b"x").directory("late/", 0o750);
    let tree = fixture.create(&tar.gzip()).expect("created");
    let mode = std::fs::metadata(fixture.tree(&tree.digest).join("late"))
        .expect("late")
        .mode();
    assert_eq!(mode & 0o7777, 0o550);
}

#[test]
fn pax_and_gnu_long_names_and_links_are_honoured() {
    let fixture = Fixture::new();
    let deep = format!("{}/file", ["component"; 20].join("/"));
    let gnu = format!("{}/gnu", ["gnu-directory"; 12].join("/"));
    let target = format!("{}/target", ["t"; 80].join("/"));
    assert!(deep.len() > 100 && gnu.len() > 100 && target.len() > 100);
    let mut tar = Tar::default();
    tar.pax(&[("path", deep.as_bytes())])
        .file("short-name", 0o644, b"pax")
        .gnu_long(b'L', gnu.as_bytes())
        .file("short-gnu", 0o644, b"gnu")
        .pax(&[("linkpath", target.as_bytes())])
        .symlink("pax-link", "ignored")
        .gnu_long(b'K', target.as_bytes())
        .symlink("gnu-link", "ignored")
        .pax(&[
            ("size", b"3"),
            ("SCHILY.xattr.security.capability", b"\x01\x02"),
        ])
        .header(&Header {
            size: 0,
            ..Header::new(b"sized", b'0')
        })
        .data(b"abc");
    let tree = fixture.create(&tar.gzip()).expect("created");
    let root = fixture.tree(&tree.digest);
    assert_eq!(std::fs::read(root.join(&deep)).expect("pax path"), b"pax");
    assert_eq!(std::fs::read(root.join(&gnu)).expect("gnu path"), b"gnu");
    assert_eq!(
        std::fs::read_link(root.join("pax-link")).expect("pax link"),
        Path::new(&target)
    );
    assert_eq!(
        std::fs::read_link(root.join("gnu-link")).expect("gnu link"),
        Path::new(&target)
    );
    assert_eq!(std::fs::read(root.join("sized")).expect("pax size"), b"abc");
    assert!(!root.join("short-name").exists() && !root.join("short-gnu").exists());
}

#[test]
fn a_ustar_prefix_is_joined_to_the_name() {
    let fixture = Fixture::new();
    let mut tar = Tar::default();
    tar.file("plain", 0o644, b"x");
    let mut bytes = tar.finish();
    // Rewrite the first header's prefix field and checksum by hand.
    bytes[345..351].copy_from_slice(b"prefix");
    bytes[148..156].copy_from_slice(b"        ");
    let sum: u64 = bytes[..512].iter().map(|&byte| u64::from(byte)).sum();
    bytes[148..155].copy_from_slice(format!("{sum:06o}\0").as_bytes());
    let archive = gzip(&bytes);
    let tree = fixture.create(&archive).expect("created");
    assert!(fixture.tree(&tree.digest).join("prefix/plain").is_file());
}

#[test]
fn each_bound_is_refused_by_name() {
    let limits = ImageLimits {
        max_archive_bytes: 4_096,
        max_unpacked_bytes: 64 * 1024,
        max_entries: 4,
    };
    let fixture = Fixture::with_limits(limits);

    let mut tar = Tar::default();
    for name in ["a", "b", "c", "d", "e"] {
        tar.file(name, 0o644, b"x");
    }
    let error = fixture.refused(&tar.gzip());
    assert!(
        matches!(
            error,
            ImageError::ArchiveLimit {
                bound: ImageBound::Entries,
                limit: 4
            }
        ),
        "{error}"
    );
    assert_eq!(error.code(), "image.archive-limit");

    // Implied directories count: `x/y/z/file` is four entries, `x/y/z/w/file` five.
    let mut tar = Tar::default();
    tar.file("x/y/z/w/file", 0o644, b"x");
    let error = fixture.refused(&tar.gzip());
    assert!(
        matches!(
            error,
            ImageError::ArchiveLimit {
                bound: ImageBound::Entries,
                ..
            }
        ),
        "{error}"
    );

    // A small archive that inflates past the bound stops at it.
    let mut tar = Tar::default();
    tar.file("zeros", 0o644, &vec![0_u8; 256 * 1024]);
    let archive = tar.gzip();
    assert!(archive.len() < 4_096, "{} bytes", archive.len());
    let error = fixture.refused(&archive);
    assert!(
        matches!(error, ImageError::ArchiveLimit { bound: ImageBound::UnpackedBytes, limit } if limit == 64 * 1024),
        "{error}"
    );

    let noise: Vec<u8> = (0..700_u32)
        .flat_map(|n| sha2::Sha256::digest(n.to_le_bytes()))
        .collect();
    let mut tar = Tar::default();
    tar.file("noise", 0o644, &noise);
    let error = fixture.refused(&tar.gzip());
    assert!(
        matches!(
            error,
            ImageError::ArchiveLimit {
                bound: ImageBound::ArchiveBytes,
                limit: 4_096
            }
        ),
        "{error}"
    );
}

#[test]
fn a_deployment_cannot_raise_a_bound_past_its_constant() {
    let limits = ImageLimits {
        max_archive_bytes: u64::MAX,
        max_unpacked_bytes: u64::MAX,
        max_entries: u64::MAX,
    };
    assert_eq!(limits.effective(), ImageLimits::default());
}

#[test]
fn a_path_or_link_target_over_its_bound_is_refused_by_name() {
    let fixture = Fixture::new();
    let long = vec!["p"; 2048].join("/") + "x";
    assert_eq!(long.len(), MAX_IMAGE_PATH_BYTES + 1);
    let mut tar = Tar::default();
    tar.pax(&[("path", long.as_bytes())])
        .file("short", 0o644, b"x");
    let error = fixture.refused(&tar.gzip());
    assert_eq!(refusal(&error), EntryRefusal::PathTooLong);
    assert!(
        error.to_string().contains("MAX_IMAGE_PATH_BYTES"),
        "{error}"
    );

    let mut tar = Tar::default();
    tar.gnu_long(b'L', long.as_bytes())
        .file("short", 0o644, b"x");
    assert_eq!(
        refusal(&fixture.refused(&tar.gzip())),
        EntryRefusal::PathTooLong
    );

    let target = "t".repeat(MAX_IMAGE_LINK_TARGET_BYTES + 1);
    let mut tar = Tar::default();
    tar.pax(&[("linkpath", target.as_bytes())])
        .symlink("link", "x");
    let error = fixture.refused(&tar.gzip());
    assert_eq!(refusal(&error), EntryRefusal::LinkTargetTooLong);
    assert!(
        error.to_string().contains("MAX_IMAGE_LINK_TARGET_BYTES"),
        "{error}"
    );

    let mut tar = Tar::default();
    tar.gnu_long(b'K', target.as_bytes()).symlink("link", "x");
    assert_eq!(
        refusal(&fixture.refused(&tar.gzip())),
        EntryRefusal::LinkTargetTooLong
    );

    let at_bound = "t".repeat(MAX_IMAGE_LINK_TARGET_BYTES);
    let mut tar = Tar::default();
    tar.pax(&[("linkpath", at_bound.as_bytes())])
        .symlink("link", "x");
    fixture
        .create(&tar.gzip())
        .expect("a target at the bound is stored");
}

#[test]
fn a_malformed_archive_is_invalid_and_leaves_nothing() {
    let fixture = Fixture::new();
    let mut tar = Tar::default();
    tar.file("a", 0o644, b"x");
    let mut corrupt = tar.finish();
    corrupt[0] = b'b';
    let error = fixture.refused(&gzip(&corrupt));
    assert_eq!(error.code(), "image.archive-invalid", "{error}");

    let error = fixture.refused(&gzip(b"not a tar at all, and not 512 bytes"));
    assert_eq!(error.code(), "image.archive-invalid", "{error}");

    let error = fixture.refused(b"not gzip");
    assert_eq!(error.code(), "image.archive-invalid", "{error}");

    let mut trailing = os_release().gzip();
    trailing.extend_from_slice(b"trailing");
    let error = fixture.refused(&trailing);
    assert_eq!(error.code(), "image.archive-invalid", "{error}");

    let mut tar = Tar::default();
    tar.file("a", 0o644, b"x");
    let mut after_end = tar.finish();
    let end = after_end.len() - 1;
    after_end[end] = 1;
    let error = fixture.refused(&gzip(&after_end));
    assert_eq!(error.code(), "image.archive-invalid", "{error}");
}

#[test]
fn an_unpack_that_fails_midway_leaves_nothing_published() {
    let fixture = Fixture::new();
    let mut tar = Tar::default();
    for at in 0..50 {
        tar.file(&format!("dir/file-{at}"), 0o644, &[b'x'; 2_000]);
    }
    let whole = tar.finish();
    // Cut inside the 40th file's data: the digest matches the bytes sent, the unpack has
    // written 39 files when the stream ends.
    let archive = gzip(&whole[..39 * 2_560 + 512 + 100]);
    let error = fixture.refused(&archive);
    assert_eq!(error.code(), "image.archive-invalid", "{error}");
}

#[test]
fn a_read_that_fails_midway_leaves_nothing_staged() {
    struct Failing(usize);
    impl Read for Failing {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            if self.0 == 0 {
                return Err(std::io::Error::other("the connection broke"));
            }
            let step = self.0.min(buffer.len()).min(100);
            buffer[..step].fill(7);
            self.0 -= step;
            Ok(step)
        }
    }
    let fixture = Fixture::new();
    let error = fixture
        .store
        .create(&digest(b"x"), Failing(1_000))
        .expect_err("failed");
    assert_eq!(error.code(), "image.store-failed");
    assert!(fixture.published().is_empty());
    fixture.staging_and_trash_are_empty();
}

#[test]
fn an_interrupted_upload_is_removed_when_the_store_opens_again() {
    let fixture = Fixture::new();
    let archive = os_release().gzip();
    let mut upload = fixture.store.upload().expect("upload");
    upload
        .append(&archive[..archive.len() / 2])
        .expect("append");
    // A process that ends here runs no destructor.
    std::mem::forget(upload);
    // And one that ended inside an unpack left a half-written, partly read-only tree.
    let left = fixture
        .store_path()
        .join("staging")
        .join(ulid::Ulid::generate().to_string());
    std::fs::create_dir_all(left.join("tree/usr/bin")).expect("left tree");
    std::fs::write(left.join("tree/usr/bin/tool"), "x").expect("left file");
    std::fs::set_permissions(
        left.join("tree/usr/bin"),
        std::fs::Permissions::from_mode(0o555),
    )
    .expect("read-only");
    let trashed = fixture
        .store_path()
        .join("trash")
        .join(ulid::Ulid::generate().to_string());
    std::fs::create_dir_all(trashed.join("etc")).expect("left trash");
    assert_eq!(
        std::fs::read_dir(fixture.store_path().join("staging"))
            .expect("staging")
            .count(),
        2
    );
    ImageStore::open(&fixture.store_path(), ImageLimits::default()).expect("reopen");
    fixture.staging_and_trash_are_empty();
    assert!(fixture.published().is_empty());
}

#[test]
fn the_store_refuses_to_open_over_what_it_did_not_make() {
    let fixture = Fixture::new();
    std::fs::write(fixture.store_path().join("staging/unknown"), "x").expect("stray");
    let error = ImageStore::open(&fixture.store_path(), ImageLimits::default())
        .err()
        .expect("refused");
    assert_eq!(error.code(), "image.store-invalid", "{error}");
    assert!(
        fixture.store_path().join("staging/unknown").exists(),
        "a stray entry was removed"
    );

    let open = tempfile::tempdir().expect("temporary directory");
    std::fs::set_permissions(open.path(), std::fs::Permissions::from_mode(0o755)).expect("mode");
    let error = ImageStore::open(open.path(), ImageLimits::default())
        .err()
        .expect("refused");
    assert_eq!(error.code(), "image.store-invalid", "{error}");
}

#[test]
fn delete_removes_the_tree_and_is_refused_while_held() {
    let fixture = Fixture::new();
    let archive = os_release().gzip();
    let tree = fixture.create(&archive).expect("created");
    assert!(fixture.store.contains(&tree.digest).expect("contains"));
    let hold = fixture.store.hold(&tree.digest).expect("hold");
    let second = fixture.store.hold(&tree.digest).expect("second hold");
    assert_eq!(hold.digest(), tree.digest);
    assert!(fixture.store.is_held(&tree.digest).expect("held"));
    let error = fixture.store.delete(&tree.digest).expect_err("in use");
    assert_eq!(error.code(), "image.in-use");
    drop(hold);
    assert_eq!(
        fixture
            .store
            .delete(&tree.digest)
            .expect_err("in use")
            .code(),
        "image.in-use"
    );
    drop(second);
    assert!(!fixture.store.is_held(&tree.digest).expect("held"));
    fixture.store.delete(&tree.digest).expect("deleted");
    assert!(!fixture.tree(&tree.digest).exists());
    assert!(fixture.published().is_empty());
    fixture.staging_and_trash_are_empty();
    assert_eq!(
        fixture
            .store
            .delete(&tree.digest)
            .expect_err("absent")
            .code(),
        "image.not-found"
    );
    assert_eq!(
        fixture
            .store
            .hold(&tree.digest)
            .err()
            .expect("absent")
            .code(),
        "image.not-found"
    );
    // The same bytes publish again after the delete.
    assert!(fixture.create(&archive).expect("again").unpacked.is_some());
}

#[test]
fn delete_follows_no_link_in_the_tree() {
    let fixture = Fixture::new();
    let mut tar = Tar::default();
    tar.symlink(
        "beside",
        fixture.outer.path().join("beside").to_str().expect("utf-8"),
    )
    .symlink(
        "sentinel",
        fixture
            .outer
            .path()
            .join("sentinel")
            .to_str()
            .expect("utf-8"),
    );
    let deep = ["deep"; 800].join("/");
    tar.pax(&[("path", format!("{deep}/file").as_bytes())])
        .file("short", 0o000, b"x");
    let archive = tar.gzip();
    let before = fixture.outside();
    let tree = fixture.create(&archive).expect("created");
    fixture.store.delete(&tree.digest).expect("deleted");
    assert_eq!(
        fixture.outside(),
        before,
        "a path outside the store changed"
    );
    assert!(fixture.published().is_empty());
    fixture.staging_and_trash_are_empty();
}
