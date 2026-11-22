//! Cases of the fifth review of the baseline store (design 21, § *The baseline store*): what a
//! create does with a baseline follows the objects of the baseline, a tree over the byte limit
//! is not read, and the store has a byte bound of its own.

use std::io::Write as _;
use std::path::Path;

use git2::{ObjectType, Oid, Repository};
use sha2::{Digest as _, Sha256};
use substrate_wire::StorageLimit;
use tempfile::tempdir;

use super::super::{finish_materialization, store};
use super::store_failure_cases::repository_holding;
use super::write_baseline;
use crate::git_baselines::{DEFAULT_INODE_LIMIT, DEFAULT_LIMIT};

const BRANCH: &str = "refs/heads/provider-default";

/// The longest chain of objects stored against each other that Git itself writes.
const LONGEST_CHAIN_GIT_WRITES: u64 = 4095;

/// A zlib stream that holds `bytes` as they are, in one stored block.
pub(super) fn stored(bytes: &[u8]) -> Vec<u8> {
    let length = u16::try_from(bytes.len()).expect("one stored block");
    let mut stream = vec![0x78, 0x01, 0x01];
    stream.extend(length.to_le_bytes());
    stream.extend((!length).to_le_bytes());
    stream.extend(bytes);
    let (mut low, mut high) = (1_u32, 0_u32);
    for byte in bytes {
        low = (low + u32::from(*byte)) % 65_521;
        high = (high + low) % 65_521;
    }
    stream.extend(((high << 16) | low).to_be_bytes());
    stream
}

/// How a pack names the object another one is stored against: by the bytes between them.
fn base_distance(mut distance: u64) -> Vec<u8> {
    let seven_bits = |value: u64| u8::try_from(value & 0x7f).expect("seven bits");
    let mut bytes = vec![seven_bits(distance)];
    distance >>= 7;
    while distance > 0 {
        distance -= 1;
        bytes.push(0x80 | seven_bits(distance));
        distance >>= 7;
    }
    bytes.reverse();
    bytes
}

/// The tree of a directory that holds `files` files of eight bytes each, whose objects are in
/// a pack this writes into the repository's object store. The pack holds one file object as
/// it is and `links` more, each stored against the one before it; each of the directory's
/// files is stored against the last of those. So the pack states the type of each of the
/// directory's files only through `links + 1` other objects.
pub(super) fn chained(repository: &Repository, links: u64, files: u64) -> Oid {
    let content = u64::to_be_bytes;
    let mut pack = b"PACK".to_vec();
    pack.extend(2_u32.to_be_bytes());
    pack.extend(
        u32::try_from(1 + links + files)
            .expect("object count")
            .to_be_bytes(),
    );
    // A file object (type 3) of eight bytes, as it is.
    let mut last_link = pack.len();
    pack.push(0x38);
    pack.extend(stored(&content(0)));
    for number in 1..=links + files {
        let offset = pack.len();
        // Stored against an earlier object (type 6) in eleven bytes: the eight bytes of that
        // object, the eight of this one, and the instruction to take eight bytes that follow.
        let mut instructions = vec![8, 8, 8];
        instructions.extend(content(number));
        pack.push(0x6b);
        pack.extend(base_distance(
            u64::try_from(offset - last_link).expect("distance"),
        ));
        pack.extend(stored(&instructions));
        if number <= links {
            last_link = offset;
        }
    }
    let mut hasher = gix::hash::hasher(gix::hash::Kind::Sha1);
    hasher.update(&pack);
    let trailer = hasher.try_finalize().expect("pack checksum");
    pack.extend(trailer.as_slice());
    let objects = repository.odb().expect("object store");
    let mut writer = objects.packwriter().expect("pack writer");
    writer.write_all(&pack).expect("pack bytes");
    writer.commit().expect("pack index");
    let mut root = Vec::new();
    for number in links + 1..=links + files {
        let file = Oid::hash_object(ObjectType::Blob, &content(number)).expect("file id");
        root.extend(format!("100644 f{number:012}\0").into_bytes());
        root.extend(file.as_bytes());
    }
    objects.write(ObjectType::Tree, &root).expect("root tree")
}

/// Design 21, § *The baseline store*: what a create does with a baseline is bounded by the
/// bounds of the shape check, and the check's work follows the objects of the baseline. The
/// baseline here is one commit, one tree and one file of eight bytes, within every bound. The
/// fetched pack stores that file behind 5,000 other objects, more than the longest chain Git
/// itself writes, and the check walks all of them to read the file's header, as it would for
/// every other file stored that way. Such a baseline is refused before its header is walked.
#[test]
fn a_baseline_file_stored_behind_a_longer_chain_than_git_writes_is_refused() {
    let behind = LONGEST_CHAIN_GIT_WRITES + 905;
    let (workspace, commit) = repository_holding(|repository| chained(repository, behind - 1, 1));
    let repository = Repository::open(workspace.path()).expect("repository");

    let checked = store::check_baseline(&repository, commit, DEFAULT_LIMIT).ok();

    assert_eq!(
        checked, None,
        "the check walked {behind} stored objects for one file header and admitted the baseline"
    );
}

/// The loose object file of `id` in the repository at `workspace`.
fn loose(workspace: &Path, id: Oid) -> std::path::PathBuf {
    let id = id.to_string();
    workspace.join(".git/objects").join(&id[..2]).join(&id[2..])
}

/// The tree of a directory that holds `files` empty files, each by a name of 255 bytes: a tree
/// of 283 bytes for each.
fn tree_of_files(repository: &Repository, files: u8) -> Oid {
    let file = repository.blob(b"").expect("file");
    let mut builder = repository.treebuilder(None).expect("tree builder");
    for letter in (b'a'..).take(usize::from(files)) {
        builder
            .insert(char::from(letter).to_string().repeat(255), file, 0o100_644)
            .expect("file");
    }
    builder.write().expect("directory")
}

/// Design 21, § *Object sizes*: a tree whose header states more than the byte limit is not
/// read and is refused `workspace.git-storage-limit`. What stands in the root tree's place
/// here states 849 bytes and is not that tree, so reading it fails its check. Under a byte
/// limit of 600 the header alone refuses it; under one of 1,024 it is read and the create ends
/// `workspace.git-checkout-failed`. The inode limit admits a far larger tree in both.
#[test]
fn a_tree_that_states_more_than_the_byte_limit_is_refused_before_it_is_read() {
    let mut trees = None;
    let (workspace, commit) = repository_holding(|repository| {
        let root = tree_of_files(repository, 1);
        trees = Some((root, tree_of_files(repository, 3)));
        root
    });
    let (root, larger) = trees.expect("trees");
    std::fs::remove_file(loose(workspace.path(), root)).expect("tree removed");
    std::fs::copy(
        loose(workspace.path(), larger),
        loose(workspace.path(), root),
    )
    .expect("another tree's bytes in the tree's place");
    let checkout = |max_bytes: u64| {
        let repository = Repository::open(workspace.path()).expect("repository");
        let limit = StorageLimit {
            max_bytes,
            max_inodes: DEFAULT_INODE_LIMIT,
        };
        finish_materialization(&repository, BRANCH, commit, limit)
            .map_or_else(|error| error.code, |()| "served")
    };

    assert_eq!(checkout(600), "workspace.git-storage-limit");
    assert_eq!(checkout(1024), "workspace.git-checkout-failed");
}

/// `length` bytes that no compression makes smaller: a SHA-256 chain.
fn incompressible(length: usize) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(length + 32);
    let mut block = Sha256::digest(b"review5");
    while bytes.len() < length {
        bytes.extend_from_slice(&block);
        block = Sha256::digest(block);
    }
    bytes.truncate(length);
    bytes
}

/// Design 21, § *Its bound*: a store larger than the byte limit is not written, also when the
/// files of the baseline together are within that limit. The baseline here is one file of
/// 8,192 bytes that do not compress. Its pack is larger than the file by the commit, the tree
/// and what frames the three objects, more than 100 and fewer than 700 bytes, and the index of
/// the pack is 1,156 bytes. Under a limit of 8,292 bytes the pack alone is over it, under one
/// of 8,892 the pack and its index together are; each is refused
/// `workspace.git-storage-limit` and leaves nothing in the baseline root. Under 12,288 the
/// store is written.
#[test]
fn a_store_over_the_byte_limit_is_refused_when_its_files_together_are_within_it() {
    const FILE: usize = 8192;
    let (workspace, commit) = repository_holding(|repository| {
        let file = repository.blob(&incompressible(FILE)).expect("file");
        let mut root = repository.treebuilder(None).expect("tree builder");
        root.insert("noise.bin", file, 0o100_644).expect("file");
        root.write().expect("root tree")
    });
    let file = u64::try_from(FILE).expect("file length");
    let write = |max_bytes: u64| {
        let baselines = tempdir().expect("baseline root");
        let written = write_baseline(
            baselines.path(),
            "ws_git",
            workspace.path(),
            &commit.to_string(),
            max_bytes,
        )
        .map_or_else(|error| error.code, |()| "written");
        let left = std::fs::read_dir(baselines.path())
            .expect("baseline root")
            .count();
        (written, left)
    };

    assert_eq!(
        write(file + 100),
        ("workspace.git-storage-limit", 0),
        "the pack alone is over the limit"
    );
    assert_eq!(
        write(file + 700),
        ("workspace.git-storage-limit", 0),
        "the pack and its index together are over the limit"
    );
    assert_eq!(
        write(file + 4096),
        ("written", 2),
        "the commit-id file and the directory of the stores"
    );
}
