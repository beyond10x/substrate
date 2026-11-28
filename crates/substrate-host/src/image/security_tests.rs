//! Conformance cases for the unpacker and the store against design 38 § 3.3 and § 3.4, on
//! archives each case builds in memory: nothing outside the store root changes, no kind but the
//! four is created, every bound holds, a failure publishes nothing and leaves no staging, the
//! removal follows no link, and no answer names a host path.

use std::collections::BTreeMap;
use std::io::Write as _;
use std::os::unix::ffi::OsStrExt as _;
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};

use super::test_archive::{Header, Tar, digest, gzip};
use super::{EntryRefusal, ImageBound, ImageError, ImageLimits, ImageStore, RefusedKind};

/// A directory holding the store and, beside it, files no unpack and no removal may touch.
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

    fn outside(&self) -> BTreeMap<PathBuf, String> {
        snapshot(self.outer.path(), Some(&self.store_path()))
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

    /// The archive is refused, nothing is published, no staging is left, nothing outside the
    /// store changed and the answer names no host path.
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
        self.names_no_host_path(&error);
        error
    }

    fn names_no_host_path(&self, error: &ImageError) {
        let text = error.to_string();
        let outer = self.outer.path().to_str().expect("utf-8");
        assert!(
            !text.contains(outer),
            "the answer names a host path: {text}"
        );
    }
}

fn snapshot(root: &Path, skip: Option<&Path>) -> BTreeMap<PathBuf, String> {
    let mut seen = BTreeMap::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(&directory).expect("read directory") {
            let path = entry.expect("entry").path();
            if Some(path.as_path()) == skip {
                continue;
            }
            let metadata = std::fs::symlink_metadata(&path).expect("metadata");
            let what = if metadata.file_type().is_symlink() {
                format!("link {:?}", std::fs::read_link(&path).expect("link"))
            } else if metadata.is_dir() {
                pending.push(path.clone());
                format!("directory {:o} {}", metadata.mode(), metadata.ino())
            } else {
                format!(
                    "file {:o} {} {} {}",
                    metadata.mode(),
                    metadata.ino(),
                    metadata.nlink(),
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

fn outer_text(fixture: &Fixture, name: &str) -> String {
    fixture
        .outer
        .path()
        .join(name)
        .to_str()
        .expect("utf-8")
        .to_owned()
}

/// One ustar header block whose numeric fields are given as raw bytes, so a case can state a
/// size or an owner the test writer cannot.
fn raw_header(name: &[u8], flag: u8, mode: u32, size: [u8; 12], owner: u32) -> [u8; 512] {
    let mut block = [0_u8; 512];
    block[..name.len()].copy_from_slice(name);
    block[100..108].copy_from_slice(format!("{mode:07o}\0").as_bytes());
    block[108..116].copy_from_slice(format!("{owner:07o}\0").as_bytes());
    block[116..124].copy_from_slice(format!("{owner:07o}\0").as_bytes());
    block[124..136].copy_from_slice(&size);
    block[136..148].copy_from_slice(b"00000000000\0");
    block[156] = flag;
    block[257..263].copy_from_slice(b"ustar\0");
    block[263..265].copy_from_slice(b"00");
    block[148..156].copy_from_slice(b"        ");
    let sum: u64 = block.iter().map(|&byte| u64::from(byte)).sum();
    block[148..156].copy_from_slice(format!("{sum:06o}\0 ").as_bytes());
    block
}

fn octal_size(size: u64) -> [u8; 12] {
    let mut field = [0_u8; 12];
    field.copy_from_slice(format!("{size:011o}\0").as_bytes());
    field
}

/// GNU's base-256 form of `size`, which states any 64-bit value.
fn base256_size(size: u64) -> [u8; 12] {
    let mut field = [0_u8; 12];
    field[0] = 0x80;
    field[4..].copy_from_slice(&size.to_be_bytes());
    field
}

fn end_of_archive(bytes: &mut Vec<u8>) {
    bytes.extend(std::iter::repeat_n(0, 1024));
}

// Invariant 1: no byte outside the store root, whatever the archive names.

#[test]
fn a_pax_path_or_gnu_long_name_that_escapes_is_refused_over_a_safe_ustar_name() {
    let fixture = Fixture::new();
    // An absolute name that is not a path of the fixture: a refusal answers the archive's own
    // text, which a host path in the archive would make look like a leak.
    let absolute = "/substrate-image-escape".to_owned();
    for (records, expected) in [
        (
            vec![("path", b"../escape".as_slice())],
            EntryRefusal::ParentComponent,
        ),
        (
            vec![("path", absolute.as_bytes())],
            EntryRefusal::AbsolutePath,
        ),
        (
            vec![("path", b"a/../../escape".as_slice())],
            EntryRefusal::ParentComponent,
        ),
        (
            vec![("path", b"ok\0/../../escape".as_slice())],
            EntryRefusal::NulByte,
        ),
    ] {
        let mut tar = Tar::default();
        tar.pax(&records).file("safe", 0o644, b"escape");
        let error = fixture.refused(&tar.gzip());
        assert_eq!(refusal(&error), expected, "{records:?}");
    }
    for (text, expected) in [
        (b"../escape".as_slice(), EntryRefusal::ParentComponent),
        (absolute.as_bytes(), EntryRefusal::AbsolutePath),
        (b"ok\0../escape".as_slice(), EntryRefusal::NulByte),
    ] {
        let mut tar = Tar::default();
        tar.gnu_long(b'L', text).file("safe", 0o644, b"escape");
        assert_eq!(refusal(&fixture.refused(&tar.gzip())), expected);
    }
    assert!(!fixture.outer.path().join("escape").exists());
    assert!(!Path::new("/substrate-image-escape").exists());
}

#[test]
fn a_ustar_prefix_that_escapes_is_refused() {
    let fixture = Fixture::new();
    for (prefix, expected) in [
        (b"../..".as_slice(), EntryRefusal::ParentComponent),
        (b"/".as_slice(), EntryRefusal::AbsolutePath),
    ] {
        let mut tar = Tar::default();
        tar.file("plain", 0o644, b"x");
        let mut bytes = tar.finish();
        bytes[345..345 + prefix.len()].copy_from_slice(prefix);
        bytes[148..156].copy_from_slice(b"        ");
        let sum: u64 = bytes[..512].iter().map(|&byte| u64::from(byte)).sum();
        bytes[148..155].copy_from_slice(format!("{sum:06o}\0").as_bytes());
        assert_eq!(refusal(&fixture.refused(&gzip(&bytes))), expected);
    }
}

#[test]
fn a_symbolic_link_chain_is_never_followed_by_a_later_entry() {
    let fixture = Fixture::new();
    let beside = outer_text(&fixture, "beside");
    let mut tar = Tar::default();
    tar.symlink("a", "b")
        .symlink("b", &beside)
        .file("a/kept", 0o644, b"overwritten");
    assert_eq!(
        refusal(&fixture.refused(&tar.gzip())),
        EntryRefusal::BeneathSymlink
    );
    let mut tar = Tar::default();
    tar.symlink("a", "b")
        .symlink("b", &beside)
        .directory("a/made", 0o755);
    assert_eq!(
        refusal(&fixture.refused(&tar.gzip())),
        EntryRefusal::BeneathSymlink
    );
    let mut tar = Tar::default();
    tar.symlink("root", "/").file("root/escape", 0o644, b"x");
    assert_eq!(
        refusal(&fixture.refused(&tar.gzip())),
        EntryRefusal::BeneathSymlink
    );
    for target in ["a", "a/kept", "b/kept"] {
        let mut tar = Tar::default();
        tar.symlink("a", "b")
            .symlink("b", &beside)
            .hard_link("hard", target);
        assert_eq!(
            refusal(&fixture.refused(&tar.gzip())),
            EntryRefusal::HardLinkTarget,
            "{target}"
        );
    }
    assert_eq!(
        std::fs::metadata(fixture.outer.path().join("beside/kept"))
            .expect("kept")
            .nlink(),
        1
    );
}

#[test]
fn an_entry_over_an_earlier_one_is_a_path_named_twice_whatever_the_two_kinds() {
    let fixture = Fixture::new();
    let beside = outer_text(&fixture, "beside");
    type Build = fn(&mut Tar, &str);
    let kinds: [(&str, Build); 4] = [
        ("file", |tar, name| {
            tar.file(name, 0o644, b"x");
        }),
        ("directory", |tar, name| {
            tar.directory(name, 0o755);
        }),
        ("symlink", |tar, name| {
            tar.symlink(name, "elsewhere");
        }),
        ("hard link", |tar, name| {
            tar.hard_link(name, "target");
        }),
    ];
    for (first_kind, first) in kinds {
        for (second_kind, second) in kinds {
            let mut tar = Tar::default();
            tar.file("target", 0o644, b"t");
            first(&mut tar, "name");
            second(&mut tar, "name");
            let error = fixture.refused(&tar.gzip());
            assert_eq!(
                refusal(&error),
                EntryRefusal::Duplicate,
                "{first_kind} then {second_kind}"
            );
        }
    }
    // A directory entry over an earlier symbolic link to a host directory, then a file
    // beneath it: the directory is refused, the host directory is untouched.
    let mut tar = Tar::default();
    tar.symlink("d", &beside)
        .directory("d", 0o755)
        .file("d/kept", 0o644, b"overwritten");
    assert_eq!(
        refusal(&fixture.refused(&tar.gzip())),
        EntryRefusal::Duplicate
    );
    assert_eq!(
        std::fs::read(fixture.outer.path().join("beside/kept")).expect("kept"),
        b"kept"
    );
}

#[test]
fn the_root_entry_is_a_directory_named_once() {
    let fixture = Fixture::new();
    for name in [".", "./", ".//", "././"] {
        let mut tar = Tar::default();
        tar.file(name, 0o644, b"x");
        assert_eq!(
            refusal(&fixture.refused(&tar.gzip())),
            EntryRefusal::RootNotDirectory,
            "file {name}"
        );
        let mut tar = Tar::default();
        tar.symlink(name, "/");
        assert_eq!(
            refusal(&fixture.refused(&tar.gzip())),
            EntryRefusal::RootNotDirectory,
            "symlink {name}"
        );
        let mut tar = Tar::default();
        tar.file("f", 0o644, b"x").hard_link(name, "f");
        assert_eq!(
            refusal(&fixture.refused(&tar.gzip())),
            EntryRefusal::RootNotDirectory,
            "hard link {name}"
        );
    }
    let mut tar = Tar::default();
    tar.directory(".", 0o755).directory("./", 0o755);
    assert_eq!(
        refusal(&fixture.refused(&tar.gzip())),
        EntryRefusal::Duplicate
    );
    let mut tar = Tar::default();
    tar.file("f", 0o644, b"x").hard_link("h", ".");
    assert_eq!(
        refusal(&fixture.refused(&tar.gzip())),
        EntryRefusal::HardLinkTarget
    );
}

#[test]
fn a_name_that_is_not_utf_8_stays_inside_the_tree_and_goes_with_it() {
    let fixture = Fixture::new();
    let mut tar = Tar::default();
    tar.header(&Header {
        size: 1,
        ..Header::new(b"caf\xe9", b'0')
    })
    .data(b"x")
    .header(&Header {
        mode: 0o755,
        ..Header::new(b"\xff\xfe/", b'5')
    })
    .header(&Header {
        size: 1,
        ..Header::new(b"\xff\xfe/\x80", b'0')
    })
    .data(b"y");
    let archive = tar.gzip();
    let before = fixture.outside();
    let tree = fixture.create(&archive).expect("created");
    let root = fixture.tree(&tree.digest);
    assert_eq!(
        std::fs::read(root.join(std::ffi::OsStr::from_bytes(b"caf\xe9"))).expect("file"),
        b"x"
    );
    assert_eq!(
        std::fs::read(root.join(std::ffi::OsStr::from_bytes(b"\xff\xfe/\x80"))).expect("file"),
        b"y"
    );
    assert_eq!(
        fixture.outside(),
        before,
        "a path outside the store changed"
    );
    fixture.store.delete(&tree.digest).expect("deleted");
    assert!(fixture.published().is_empty());
    fixture.staging_and_trash_are_empty();
    // A refusal of such a name answers it lossily, as archive text.
    let mut tar = Tar::default();
    tar.header(&Header::new(b"\xff/../x", b'0'));
    let error = fixture.refused(&tar.gzip());
    assert_eq!(refusal(&error), EntryRefusal::ParentComponent);
}

#[test]
fn a_nul_in_a_link_target_is_refused() {
    let fixture = Fixture::new();
    let mut tar = Tar::default();
    tar.pax(&[("linkpath", b"ok\0/etc".as_slice())])
        .symlink("link", "x");
    assert_eq!(
        refusal(&fixture.refused(&tar.gzip())),
        EntryRefusal::NulByte
    );
    let mut tar = Tar::default();
    tar.file("f", 0o644, b"x")
        .gnu_long(b'K', b"f\0../sentinel")
        .hard_link("h", "f");
    assert_eq!(
        refusal(&fixture.refused(&tar.gzip())),
        EntryRefusal::HardLinkTarget
    );
}

// Invariant 2: only the four kinds; no setuid or setgid; the store's uid.

#[test]
fn every_type_flag_but_the_four_kinds_and_the_extensions_is_refused_by_kind() {
    let fixture = Fixture::new();
    let accepted = [b'0', 0, b'7', b'1', b'2', b'5', b'x', b'g', b'L', b'K'];
    for flag in 0..=u8::MAX {
        if accepted.contains(&flag) {
            continue;
        }
        let mut tar = Tar::default();
        tar.file("first", 0o644, b"x").special("node", flag);
        let error = fixture.refused(&tar.gzip());
        assert!(
            matches!(refusal(&error), EntryRefusal::Kind(_)),
            "flag {flag}: {error}"
        );
    }
    for (flag, kind) in [
        (b'3', RefusedKind::CharacterDevice),
        (b'4', RefusedKind::BlockDevice),
        (b'6', RefusedKind::Fifo),
    ] {
        let mut tar = Tar::default();
        tar.special("node", flag);
        assert_eq!(
            refusal(&fixture.refused(&tar.gzip())),
            EntryRefusal::Kind(kind)
        );
    }
}

#[test]
fn no_entry_keeps_setuid_setgid_a_write_bit_or_the_archives_owner() {
    let fixture = Fixture::new();
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&raw_header(b"./", b'5', 0o7777, octal_size(0), 4242));
    bytes.extend_from_slice(&raw_header(b"dir/", b'5', 0o7777, octal_size(0), 4242));
    bytes.extend_from_slice(&raw_header(b"dir/tool", b'0', 0o7777, octal_size(1), 4242));
    let mut data = [0_u8; 512];
    data[0] = b'x';
    bytes.extend_from_slice(&data);
    bytes.extend_from_slice(&raw_header(
        b"implied/sub/f",
        b'0',
        0o6777,
        octal_size(0),
        4242,
    ));
    let mut hard = raw_header(b"again", b'1', 0o6777, octal_size(0), 4242);
    hard[157..165].copy_from_slice(b"dir/tool");
    hard[148..156].copy_from_slice(b"        ");
    let sum: u64 = hard.iter().map(|&byte| u64::from(byte)).sum();
    hard[148..156].copy_from_slice(format!("{sum:06o}\0 ").as_bytes());
    bytes.extend_from_slice(&hard);
    bytes.extend_from_slice(&raw_header(b"link", b'2', 0o7777, octal_size(0), 4242)[..]);
    // The symbolic link needs a target; patch it in.
    let at = bytes.len() - 512;
    bytes[at + 157] = b'x';
    bytes[at + 148..at + 156].copy_from_slice(b"        ");
    let sum: u64 = bytes[at..].iter().map(|&byte| u64::from(byte)).sum();
    bytes[at + 148..at + 156].copy_from_slice(format!("{sum:06o}\0 ").as_bytes());
    end_of_archive(&mut bytes);
    let archive = gzip(&bytes);
    let tree = fixture.create(&archive).expect("created");
    let root = fixture.tree(&tree.digest);
    let uid = nix::unistd::geteuid().as_raw();
    for (path, metadata) in walk(&root) {
        if metadata.file_type().is_symlink() {
            assert_eq!(metadata.uid(), uid, "{path:?}");
            continue;
        }
        let mode = metadata.mode();
        assert_eq!(
            mode & 0o6000,
            0,
            "{path:?} keeps setuid or setgid: {mode:o}"
        );
        assert_eq!(mode & 0o222, 0, "{path:?} keeps a write bit: {mode:o}");
        assert_eq!(metadata.uid(), uid, "{path:?} keeps the archive's owner");
    }
}

fn walk(root: &Path) -> Vec<(PathBuf, std::fs::Metadata)> {
    let mut seen = vec![(
        root.to_path_buf(),
        std::fs::symlink_metadata(root).expect("root"),
    )];
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(&directory).expect("read directory") {
            let path = entry.expect("entry").path();
            let metadata = std::fs::symlink_metadata(&path).expect("metadata");
            if metadata.is_dir() {
                pending.push(path.clone());
            }
            seen.push((path, metadata));
        }
    }
    seen
}

// Invariant 3: every bound holds against a hostile archive.

#[test]
#[ignore = "finding 1: a header whose size runs past the stream is accepted when the size is within 511 of u64::MAX"]
fn a_size_field_near_the_top_of_its_range_cannot_hide_a_header_in_an_entrys_data() {
    // A directory entry states 2^64 - 1 bytes of data. Whatever follows is that entry's data,
    // and the stream ends inside it, so the archive is refused and the header after it is
    // never read as an entry.
    let fixture = Fixture::new();
    for flag in [b'5', b'2'] {
        let mut bytes = Vec::new();
        let mut header = raw_header(b"carrier", flag, 0o755, base256_size(u64::MAX), 0);
        if flag == b'2' {
            header[157] = b'x';
            header[148..156].copy_from_slice(b"        ");
            let sum: u64 = header.iter().map(|&byte| u64::from(byte)).sum();
            header[148..156].copy_from_slice(format!("{sum:06o}\0 ").as_bytes());
        }
        bytes.extend_from_slice(&header);
        bytes.extend_from_slice(&raw_header(b"hidden", b'0', 0o644, octal_size(0), 0));
        end_of_archive(&mut bytes);
        let archive = gzip(&bytes);
        let before = fixture.outside();
        let result = fixture.create(&archive);
        if let Ok(tree) = &result {
            let hidden = fixture.tree(&tree.digest).join("hidden").exists();
            panic!(
                "flag {}: published a tree; the header inside the data became an entry: {hidden}",
                char::from(flag)
            );
        }
        let error = result.expect_err("refused");
        assert!(
            matches!(
                error.code(),
                "image.archive-invalid" | "image.archive-limit"
            ),
            "{error}"
        );
        assert!(fixture.published().is_empty());
        fixture.staging_and_trash_are_empty();
        assert_eq!(fixture.outside(), before);
    }
}

#[test]
fn a_size_that_lies_stops_at_the_inflated_bound() {
    let limits = ImageLimits {
        max_unpacked_bytes: 256 * 1024,
        ..ImageLimits::default()
    };
    let fixture = Fixture::with_limits(limits);
    // A pax size larger than the data: the reader takes what follows as data and stops at the
    // end of the stream or at the bound.
    for (flag, size) in [
        (b'0', u64::MAX / 2),
        (b'5', u64::MAX / 2),
        (b'2', 1_u64 << 40),
        (b'1', 1_u64 << 40),
    ] {
        let mut tar = Tar::default();
        tar.file("target", 0o644, b"t")
            .pax(&[("size", size.to_string().as_bytes())])
            .header(&Header {
                link: b"target",
                ..Header::new(b"liar", flag)
            })
            .data(&vec![0_u8; 300 * 1024]);
        let error = fixture.refused(&tar.gzip());
        assert!(
            matches!(
                error.code(),
                "image.archive-invalid" | "image.archive-limit"
            ),
            "flag {}: {error}",
            char::from(flag)
        );
    }
    // Zeros after the end blocks count too.
    let mut tar = Tar::default();
    tar.file("a", 0o644, b"x");
    let mut bytes = tar.finish();
    bytes.extend(std::iter::repeat_n(0, 4 * 1024 * 1024));
    let error = fixture.refused(&gzip(&bytes));
    assert!(
        matches!(
            error,
            ImageError::ArchiveLimit {
                bound: ImageBound::UnpackedBytes,
                ..
            }
        ),
        "{error}"
    );
}

#[test]
fn an_extension_header_states_a_size_over_its_bound_and_is_refused_before_it_is_read() {
    let fixture = Fixture::new();
    for (flag, size) in [
        (b'x', 1_u64 << 40),
        (b'g', 1_u64 << 40),
        (b'x', 65_537),
        (b'L', 1_u64 << 40),
        (b'K', 1_u64 << 40),
        (b'x', u64::MAX),
    ] {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&raw_header(b"ext", flag, 0o644, base256_size(size), 0));
        // Fewer data bytes than stated: a reader that allocated or read first fails later.
        bytes.extend(std::iter::repeat_n(b'a', 512));
        end_of_archive(&mut bytes);
        let error = fixture.refused(&gzip(&bytes));
        assert!(
            matches!(
                error.code(),
                "image.archive-invalid" | "image.entry-refused"
            ),
            "flag {} size {size}: {error}",
            char::from(flag)
        );
    }
}

#[test]
fn a_run_of_pax_headers_is_bounded_by_the_inflated_bound() {
    let limits = ImageLimits {
        max_unpacked_bytes: 1024 * 1024,
        ..ImageLimits::default()
    };
    let fixture = Fixture::with_limits(limits);
    let big = vec![b'p'; 60 * 1024];
    let mut tar = Tar::default();
    for _ in 0..64 {
        tar.pax(&[("comment", big.as_slice())]);
    }
    tar.file("f", 0o644, b"x");
    let error = fixture.refused(&tar.gzip());
    assert!(
        matches!(
            error,
            ImageError::ArchiveLimit {
                bound: ImageBound::UnpackedBytes,
                ..
            }
        ),
        "{error}"
    );
}

#[test]
fn a_gzip_header_name_of_a_mebibyte_is_refused_as_invalid() {
    let fixture = Fixture::new();
    let mut tar = Tar::default();
    tar.file("a", 0o644, b"x");
    let mut encoder = flate2::GzBuilder::new()
        .filename(vec![b'n'; 1024 * 1024])
        .comment(vec![b'c'; 1024 * 1024])
        .write(Vec::new(), flate2::Compression::fast());
    encoder.write_all(&tar.finish()).expect("in memory");
    let archive = encoder.finish().expect("in memory");
    let error = fixture.refused(&archive);
    assert_eq!(error.code(), "image.archive-invalid", "{error}");
}

#[test]
fn a_gzip_bomb_stops_at_the_default_inflated_bound() {
    let fixture = Fixture::new();
    // The zeros follow the end blocks, so the reader inflates them without writing a file.
    let mut tar = Tar::default();
    tar.file("a", 0o644, b"x");
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    encoder.write_all(&tar.finish()).expect("in memory");
    let zeros = vec![0_u8; 1024 * 1024];
    for _ in 0..1_026 {
        encoder.write_all(&zeros).expect("in memory");
    }
    let archive = encoder.finish().expect("in memory");
    assert!(
        (archive.len() as u64) < super::MAX_IMAGE_ARCHIVE_BYTES / 4,
        "{} bytes",
        archive.len()
    );
    let error = fixture.refused(&archive);
    assert!(
        matches!(
            error,
            ImageError::ArchiveLimit {
                bound: ImageBound::UnpackedBytes,
                limit: super::MAX_IMAGE_UNPACKED_BYTES
            }
        ),
        "{error}"
    );
}

#[test]
fn the_entry_bound_counts_refused_root_and_implied_entries() {
    let limits = ImageLimits {
        max_entries: 3,
        ..ImageLimits::default()
    };
    let fixture = Fixture::with_limits(limits);
    let mut tar = Tar::default();
    tar.directory("./", 0o755)
        .file("a", 0o644, b"x")
        .file("b", 0o644, b"x")
        .file("c", 0o644, b"x");
    assert!(matches!(
        fixture.refused(&tar.gzip()),
        ImageError::ArchiveLimit {
            bound: ImageBound::Entries,
            limit: 3
        }
    ));
}

// Invariant 4: failures publish nothing and leave no staging; concurrent uploads are safe.

#[test]
fn a_failure_after_the_tree_is_read_only_still_leaves_no_staging() {
    let fixture = Fixture::new();
    let mut tar = Tar::default();
    tar.directory("./", 0o000)
        .directory("a/", 0o000)
        .directory("a/b/", 0o000)
        .file("a/b/f", 0o000, b"x")
        .hard_link("a/h", "a/b/f")
        .symlink("a/b/up", "/");
    let mut archive = tar.gzip();
    archive.extend_from_slice(&gzip(b"second member"));
    let error = fixture.refused(&archive);
    assert_eq!(error.code(), "image.archive-invalid", "{error}");
}

#[test]
fn concurrent_uploads_of_one_digest_publish_one_tree_and_leave_no_staging() {
    let fixture = Fixture::new();
    let mut tar = Tar::default();
    for at in 0..64 {
        tar.directory(&format!("d{at}/"), 0o755)
            .file(&format!("d{at}/f"), 0o644, &[b'x'; 4096]);
    }
    let archive = tar.gzip();
    let barrier = std::sync::Barrier::new(6);
    let results: Vec<Result<bool, String>> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..6)
            .map(|_| {
                scope.spawn(|| {
                    barrier.wait();
                    fixture
                        .create(&archive)
                        .map(|tree| tree.unpacked.is_some())
                        .map_err(|error| error.to_string())
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().expect("thread"))
            .collect()
    });
    for result in &results {
        assert!(result.is_ok(), "{result:?}");
    }
    assert_eq!(fixture.published().len(), 1);
    fixture.staging_and_trash_are_empty();
    let root = fixture.tree(&digest(&archive));
    assert_eq!(std::fs::read(root.join("d63/f")).expect("file").len(), 4096);
}

#[test]
fn an_upload_dropped_before_complete_leaves_no_staging() {
    let fixture = Fixture::new();
    let mut upload = fixture.store.upload().expect("upload");
    upload.append(b"partial").expect("append");
    drop(upload);
    fixture.staging_and_trash_are_empty();
    assert!(fixture.published().is_empty());
}

// Invariant 5: the removal follows no link and cannot leave the store; open refuses what it
// did not make.

#[test]
fn delete_of_a_hostile_tree_touches_nothing_outside_it() {
    let fixture = Fixture::new();
    let keep = {
        let mut tar = Tar::default();
        tar.file("kept", 0o644, b"other tree");
        tar.gzip()
    };
    let kept = fixture.create(&keep).expect("kept tree");
    let mut tar = Tar::default();
    tar.symlink("slash", "/")
        .symlink("store", "../..")
        .symlink(
            "other",
            &format!("../{}", kept.digest.trim_start_matches("sha256:")),
        )
        .directory(".removal-1/", 0o000)
        .directory(".removal-1/.removal-2/", 0o000)
        .file(".removal-1/.removal-2/f", 0o000, b"x")
        .directory("d/", 0o000)
        .directory("d/e/", 0o000)
        .directory("d/e/.removal-1/", 0o000)
        .symlink("d/e/.removal-1/up", "../../../../..")
        .hard_link("d/h", ".removal-1/.removal-2/f");
    let archive = tar.gzip();
    let tree = fixture.create(&archive).expect("created");
    let before = fixture.outside();
    let other_before = snapshot(&fixture.tree(&kept.digest), None);
    fixture.store.delete(&tree.digest).expect("deleted");
    assert_eq!(
        fixture.outside(),
        before,
        "a path outside the store changed"
    );
    assert_eq!(
        snapshot(&fixture.tree(&kept.digest), None),
        other_before,
        "another tree changed"
    );
    assert_eq!(
        fixture.published(),
        vec![kept.digest.trim_start_matches("sha256:").to_owned()]
    );
    fixture.staging_and_trash_are_empty();
}

#[test]
fn open_follows_no_link_in_place_of_an_area_or_a_tree() {
    let outer = tempfile::tempdir().expect("temporary directory");
    let victim = outer.path().join("victim");
    let left = ulid::Ulid::generate().to_string();
    std::fs::create_dir_all(victim.join(&left)).expect("victim");
    std::fs::write(victim.join(&left).join("precious"), "x").expect("precious");
    for area in ["staging", "trash", "trees"] {
        let store = outer.path().join(format!("store-{area}"));
        std::fs::create_dir(&store).expect("store");
        std::fs::set_permissions(&store, std::fs::Permissions::from_mode(0o700)).expect("mode");
        std::os::unix::fs::symlink(&victim, store.join(area)).expect("link");
        let error = ImageStore::open(&store, ImageLimits::default())
            .err()
            .expect("refused");
        assert_eq!(error.code(), "image.store-invalid", "{area}: {error}");
        assert!(
            !error
                .to_string()
                .contains(outer.path().to_str().expect("utf-8")),
            "{error}"
        );
        assert!(victim.join(&left).join("precious").exists(), "{area}");
    }
    let store = outer.path().join("store-tree-link");
    std::fs::create_dir(&store).expect("store");
    std::fs::set_permissions(&store, std::fs::Permissions::from_mode(0o700)).expect("mode");
    ImageStore::open(&store, ImageLimits::default()).expect("first open");
    std::os::unix::fs::symlink(&victim, store.join("trees").join("a".repeat(64))).expect("link");
    let error = ImageStore::open(&store, ImageLimits::default())
        .err()
        .expect("refused");
    assert_eq!(error.code(), "image.store-invalid", "{error}");
    assert!(victim.join(&left).join("precious").exists());
}

#[test]
#[ignore = "finding 2: open accepts a directory holding entries it did not make and removes trash/ and staging/ content there"]
fn open_refuses_a_store_root_that_holds_an_entry_it_did_not_make() {
    let outer = tempfile::tempdir().expect("temporary directory");
    let root = outer.path().join("not-a-store");
    std::fs::create_dir(&root).expect("root");
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).expect("mode");
    std::fs::write(root.join("notes"), "someone else's").expect("notes");
    let left = ulid::Ulid::generate().to_string();
    std::fs::create_dir_all(root.join("trash").join(&left)).expect("trash");
    std::fs::set_permissions(root.join("trash"), std::fs::Permissions::from_mode(0o700))
        .expect("mode");
    std::fs::write(root.join("trash").join(&left).join("precious"), "x").expect("precious");
    let result = ImageStore::open(&root, ImageLimits::default());
    let precious = root.join("trash").join(&left).join("precious").exists();
    let created: Vec<_> = ["staging", "trees"]
        .into_iter()
        .filter(|area| root.join(area).exists())
        .collect();
    let error = result.err().unwrap_or_else(|| {
        panic!(
            "opened a directory holding `notes`; precious kept: {precious}; created: {created:?}"
        )
    });
    assert_eq!(error.code(), "image.store-invalid", "{error}");
    assert!(
        precious,
        "an entry of a directory the store did not make was removed"
    );
}

// Invariant 6: no answer names a host path.

#[test]
fn no_answer_of_open_create_or_delete_names_a_host_path() {
    let outer = tempfile::tempdir().expect("temporary directory");
    let host = outer.path().to_str().expect("utf-8").to_owned();
    let mut errors = Vec::new();
    errors.push(
        ImageStore::open(&outer.path().join("absent"), ImageLimits::default())
            .err()
            .expect("absent"),
    );
    let open = outer.path().join("open");
    std::fs::create_dir(&open).expect("open");
    std::fs::set_permissions(&open, std::fs::Permissions::from_mode(0o755)).expect("mode");
    errors.push(
        ImageStore::open(&open, ImageLimits::default())
            .err()
            .expect("open"),
    );
    let file = outer.path().join("file");
    std::fs::write(&file, "x").expect("file");
    errors.push(
        ImageStore::open(&file, ImageLimits::default())
            .err()
            .expect("file"),
    );
    let link = outer.path().join("link");
    std::os::unix::fs::symlink(&open, &link).expect("link");
    errors.push(
        ImageStore::open(&link, ImageLimits::default())
            .err()
            .expect("link"),
    );
    let fixture = Fixture::with_limits(ImageLimits {
        max_archive_bytes: 1024,
        max_unpacked_bytes: 64 * 1024,
        max_entries: 2,
    });
    let path_like = outer_text(&fixture, "x");
    let mut tar = Tar::default();
    tar.file("a", 0o644, b"x");
    let archive = tar.gzip();
    errors.push(
        fixture
            .store
            .create(&digest(b"other"), archive.as_slice())
            .expect_err("mismatch"),
    );
    errors.push(
        fixture
            .store
            .create("sha256:x", archive.as_slice())
            .expect_err("invalid"),
    );
    errors.push(
        fixture
            .store
            .delete(&digest(b"absent"))
            .expect_err("absent"),
    );
    errors.push(
        fixture
            .store
            .hold(&digest(b"absent"))
            .err()
            .expect("absent"),
    );
    errors.push(
        fixture
            .store
            .create(&digest(&[0; 2048]), &[0_u8; 2048][..])
            .expect_err("limit"),
    );
    let mut tar = Tar::default();
    tar.symlink("s", &path_like).file("s/x", 0o644, b"x");
    errors.push(fixture.create(&tar.gzip()).expect_err("beneath"));
    for error in &errors {
        let text = error.to_string();
        assert!(
            !text.contains(&host),
            "{} names a host path: {text}",
            error.code()
        );
        fixture.names_no_host_path(error);
    }
}
