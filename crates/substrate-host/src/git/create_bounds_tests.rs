//! Cases of the bounds a Git create holds on its work and its memory (design 21, § *The
//! baseline store*, *The work and the memory of a create follow its byte limit*). Each runs
//! the production fetch against the real TLS peer, which answers the fetch with a pack
//! written by hand: a pack within every other bound and over the one the case is for.
//!
//! Each case states a bound on the memory of the process that runs it. cargo-nextest runs
//! each case in a process of its own, so the peak of that process is the case's own, and the
//! bound is asserted. Where other cases run in the same process, as under `cargo test`, the
//! peak is theirs too: the case then prints that it does not assert the bound, and asserts
//! everything else.

use std::io::Write as _;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use git2::{ObjectType, Oid, Repository, Signature};
use substrate_wire::{StorageLimit, StorageUsage, Workspace};
use tempfile::{TempDir, tempdir};
use zeroize::Zeroizing;

use super::baseline_store_tests::{
    baselines, driver, fetched_packs, input, names, refused, served, staging_paths,
};
use super::{AUTHORITY, Server, materialize, repository};
use crate::git::tests::objects_of;
use crate::git::tests::store_failure_cases::repository_holding;
use crate::git_baselines::DEFAULT_BYTE_LIMIT;
use crate::{DispatchOutcome, Driver as _, DriverError, DriverErrorClass};

const MEBIBYTE: u64 = 1024 * 1024;

/// The most memory that may be resident at once in the process of a case, in kibibytes:
/// 96 MiB.
const RESIDENT_BOUND_KIB: u64 = 96 * 1024;

/// The type a pack gives a commit.
const COMMIT: u8 = 1;

/// The type a pack gives a tree.
const TREE: u8 = 2;

/// The type a pack gives a file object.
const FILE: u8 = 3;

/// The type a pack gives an entry stored behind an earlier one, named by the bytes between
/// them.
const BEHIND: u8 = 6;

/// A zlib stream of `bytes`.
fn deflated(bytes: &[u8]) -> Vec<u8> {
    let mut stream =
        gix::zlib::stream::deflate::Write::new(Vec::new(), gix::zlib::Compression::BEST_SPEED);
    stream.write_all(bytes).expect("deflated bytes");
    stream.flush().expect("the end of the stream");
    stream.into_inner()
}

/// The seven low bits of `value`.
fn seven_bits(value: u64) -> u8 {
    u8::try_from(value & 0x7f).expect("seven bits")
}

/// The length of `bytes` as a pack states it.
fn length(bytes: &[u8]) -> u64 {
    u64::try_from(bytes.len()).expect("length")
}

/// The header of a pack entry of the type `kind` that states `size` bytes.
fn entry_header(kind: u8, mut size: u64) -> Vec<u8> {
    let mut bytes = vec![(kind << 4) | u8::try_from(size & 0xf).expect("four bits")];
    size >>= 4;
    while size > 0 {
        *bytes.last_mut().expect("a first byte") |= 0x80;
        bytes.push(seven_bits(size));
        size >>= 7;
    }
    bytes
}

/// How a pack names the entry another one is stored behind: by the bytes between them.
fn distance(mut between: u64) -> Vec<u8> {
    let mut bytes = vec![seven_bits(between)];
    between >>= 7;
    while between > 0 {
        between -= 1;
        bytes.push(0x80 | seven_bits(between));
        between >>= 7;
    }
    bytes.reverse();
    bytes
}

/// A size as the instructions of an entry stored behind another state it.
fn stated_size(mut size: u64) -> Vec<u8> {
    let mut bytes = Vec::new();
    loop {
        let low = seven_bits(size);
        size >>= 7;
        if size == 0 {
            bytes.push(low);
            return bytes;
        }
        bytes.push(0x80 | low);
    }
}

/// The instructions that make, from an object of `size` bytes, another of `size` bytes: all
/// of the first but its last eight bytes, then `number`. Every `number` makes another object.
fn version(size: u64, number: u64) -> Vec<u8> {
    let taken = size - 8;
    assert!(
        taken < 1 << 24 && taken != 1 << 16,
        "one instruction takes the bytes"
    );
    let mut instructions = [stated_size(size), stated_size(size)].concat();
    // Take `taken` bytes from the start: the instruction names each byte of that number
    // that is not zero.
    let mut take = 0x80_u8;
    let mut operands = Vec::new();
    for (place, byte) in taken.to_le_bytes().into_iter().take(3).enumerate() {
        if byte != 0 {
            take |= 0x10 << place;
            operands.push(byte);
        }
    }
    instructions.push(take);
    instructions.extend(operands);
    // Then the eight bytes that follow.
    instructions.push(8);
    instructions.extend(number.to_be_bytes());
    instructions
}

/// The instructions that make `content` from an object of `base` bytes, taking nothing of
/// it.
fn instead(base: u64, content: &[u8]) -> Vec<u8> {
    let mut instructions = [stated_size(base), stated_size(length(content))].concat();
    instructions.push(u8::try_from(content.len()).expect("one instruction"));
    instructions.extend(content);
    instructions
}

/// A pack written by hand.
struct Pack {
    bytes: Vec<u8>,
    entries: u32,
}

impl Pack {
    fn new() -> Self {
        let mut bytes = b"PACK".to_vec();
        bytes.extend(2_u32.to_be_bytes());
        bytes.extend(0_u32.to_be_bytes());
        Self { bytes, entries: 0 }
    }

    /// Adds an entry of the type `kind` that the pack stores as it is, and answers where it
    /// begins.
    fn whole(&mut self, kind: u8, content: &[u8]) -> u64 {
        let offset = length(&self.bytes);
        self.bytes.extend(entry_header(kind, length(content)));
        self.bytes.extend(deflated(content));
        self.entries += 1;
        offset
    }

    /// Adds an entry stored behind the entry that begins at `base`, which `instructions` make
    /// its object of, and answers where it begins.
    fn behind(&mut self, base: u64, instructions: &[u8]) -> u64 {
        let offset = length(&self.bytes);
        self.bytes
            .extend(entry_header(BEHIND, length(instructions)));
        self.bytes.extend(distance(offset - base));
        self.bytes.extend(deflated(instructions));
        self.entries += 1;
        offset
    }

    /// The bytes of the pack as a server sends it: with the number of its entries and its
    /// checksum.
    fn sent(mut self) -> Vec<u8> {
        self.bytes[8..12].copy_from_slice(&self.entries.to_be_bytes());
        let mut hasher = gix::hash::hasher(gix::hash::Kind::Sha1);
        hasher.update(&self.bytes);
        let checksum = hasher.try_finalize().expect("pack checksum");
        self.bytes.extend(checksum.as_slice());
        self.bytes
    }
}

/// A file object of `size` bytes for the versions of [`version`].
fn first_version(size: u64) -> Vec<u8> {
    vec![0x5a; usize::try_from(size).expect("size")]
}

/// A number of the line `key` of this process's status, in kibibytes: `VmHWM` is the most
/// memory that was resident at once, `VmPeak` the most address space it had.
fn peak_kib(key: &str) -> u64 {
    let status = std::fs::read_to_string("/proc/self/status").expect("process status");
    status
        .lines()
        .find_map(|line| line.strip_prefix(key)?.strip_prefix(':'))
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|number| number.parse().ok())
        .expect("a line of the process status")
}

/// Holds `measured_kib`, a peak of this process the case names `what`, to `bound_kib`, where
/// this process runs the case alone: cargo-nextest says so in the environment of each
/// process it starts. Elsewhere the peak is also that of the cases beside it, and one line
/// says that it is not asserted.
fn hold_peak(case: &str, what: &str, measured_kib: u64, bound_kib: u64) {
    let alone =
        std::env::var("NEXTEST_EXECUTION_MODE").is_ok_and(|mode| mode == "process-per-test");
    if alone {
        eprintln!("{case}: {what} {measured_kib} KiB, bound {bound_kib} KiB");
        assert!(
            measured_kib <= bound_kib,
            "{case}: {what} {measured_kib} KiB is over {bound_kib} KiB"
        );
    } else {
        eprintln!("{case}: {what} is not asserted: this process runs other cases beside this one");
    }
}

/// The create was refused `workspace.git-storage-limit` and its workspace is proved absent:
/// beneath `root` there is no workspace, no commit-id file, no store and no staging tree.
fn assert_refused_and_absent(root: &Path, outcome: DispatchOutcome<Workspace>) {
    let (dispatch, error) = refused(outcome);
    assert_eq!(error.code, "workspace.git-storage-limit");
    assert_eq!(error.class, DriverErrorClass::Exhausted);
    assert_eq!(dispatch, "contained absent");
    assert!(!root.join("ws_a").exists(), "a workspace is left");
    let baselines = baselines(root);
    assert!(!baselines.join("ws_a").exists(), "a commit-id file is left");
    assert_eq!(names(&baselines.join(".objects")), Vec::<String>::new());
    assert!(
        names(&baselines).iter().all(|name| name == ".objects"),
        "{:?}",
        names(&baselines)
    );
    assert!(staging_paths(root).is_empty());
}

/// How the fetch step of a create ends under `limit` when its source answers the fetch with
/// `pack`, and the staging tree the step ran in.
fn fetch(pack: Vec<u8>, limit: StorageLimit) -> (Result<StorageUsage, DriverError>, TempDir) {
    let (source, commit) = repository(0);
    let server = Server::start_with_pack(source.path(), pack);
    let target = tempdir().expect("staging tree");
    let outcome = materialize(
        target.path(),
        &server.locator,
        &server.binding,
        &server.source(commit),
        &Zeroizing::new(AUTHORITY.to_owned()),
        limit,
        &Arc::new(AtomicBool::new(false)),
    );
    (outcome, target)
}

/// The fetch step ended `workspace.git-storage-limit` while it indexed the pack: the staging
/// tree at `target` holds no pack, no branch and nothing checked out. It is the error and the
/// step of a pack that states more entries than the byte limit holds, for which
/// [`a_create_whose_pack_states_more_entries_than_its_byte_limit_holds_is_refused`] proves
/// through the driver that the workspace is absent: the driver removes the staging tree of a
/// step that failed, whatever the step left in it.
fn assert_fetch_refused(outcome: Result<StorageUsage, DriverError>, target: &Path) {
    let error = outcome.expect_err("a pack over a bound of the fetch");
    assert_eq!(error.code, "workspace.git-storage-limit");
    assert_eq!(error.class, DriverErrorClass::Exhausted);
    assert_eq!(
        fetched_packs(target),
        Vec::<String>::new(),
        "the fetch finished its pack"
    );
    assert!(
        Repository::open(target)
            .expect("staging repository")
            .refname_to_id("refs/remotes/origin/provider-default")
            .is_err(),
        "the fetch wrote the branch"
    );
    assert_eq!(names(target), [".git"], "something is checked out");
}

/// Whole reads. Reading an object whole inflates every entry its pack stores it behind and
/// makes the object of each; those bytes, for the commit, each tree and each file object of a
/// baseline, once for each place the baseline names the object, are at most 64 times the byte
/// limit of the create.
///
/// The baseline here is one file of eight bytes under 4,096 names. The pack stores the file
/// behind 65 versions of an object of 1 MiB, each stored behind the one before it, so one
/// whole read of the file inflates and makes more than 65 MiB, and the 4,096 places come to
/// more than 260 GiB: over the 128 GiB of a create that requests no limit, which has 2 GiB.
/// The pack is within every other bound: it has 68 entries, its versions hold 2 MiB at once
/// while the fetch computes their ids, and the fetch makes about 65 MiB of them.
///
/// The create is refused `workspace.git-storage-limit` and the workspace is proved absent.
/// The check ends before any file object is read, so nothing is checked out. The most memory
/// resident at once in the process stays under 96 MiB.
#[tokio::test]
async fn a_create_whose_whole_reads_come_to_more_than_64_times_its_byte_limit_is_refused() {
    const NAMES: u64 = 4096;
    const VERSIONS: u64 = 65;
    const CONTENT: &[u8; 8] = b"baseline";
    const { assert!(NAMES * VERSIONS * MEBIBYTE > 64 * DEFAULT_BYTE_LIMIT) };
    let (source, commit) = repository_holding(|repository| {
        let file = repository.blob(CONTENT).expect("file");
        let mut root = repository.treebuilder(None).expect("tree builder");
        for number in 0..NAMES {
            root.insert(format!("f{number:04}"), file, 0o100_644)
                .expect("file");
        }
        root.write().expect("root tree")
    });
    let mut pack = Pack::new();
    {
        let repository = Repository::open(source.path()).expect("source repository");
        let objects = repository.odb().expect("object store");
        let tree = repository.find_commit(commit).expect("commit").tree_id();
        for (id, kind, as_type) in [
            (commit, COMMIT, ObjectType::Commit),
            (tree, TREE, ObjectType::Tree),
        ] {
            let object = objects.read(id).expect("object");
            assert_eq!(object.kind(), as_type);
            pack.whole(kind, object.data());
        }
    }
    let mut last = pack.whole(FILE, &first_version(MEBIBYTE));
    for number in 1..VERSIONS {
        last = pack.behind(last, &version(MEBIBYTE, number));
    }
    pack.behind(last, &instead(MEBIBYTE, CONTENT));
    assert_eq!(pack.entries, 68);
    let server = Server::start_with_pack(source.path(), pack.sent());
    let root = tempdir().expect("workspace root");
    let driver = driver(root.path(), &server);

    let outcome = driver
        .create_workspace_authorized("ws_a", "ws_a", &input(&server, commit), Some(AUTHORITY))
        .await;

    assert_refused_and_absent(root.path(), outcome);
    hold_peak(
        "whole reads",
        "peak resident memory",
        peak_kib("VmHWM"),
        RESIDENT_BOUND_KIB,
    );
}

/// The table of entries. While a pack arrives the fetch keeps one item of 64 bytes for each
/// entry, and those items take no more bytes than the byte limit of the create: a pack that
/// states more entries is refused before its first entry is read, and before the table is
/// reserved for more than 8,192 items.
///
/// A create that requests no limit has 2 GiB, which is 33,554,432 items. The pack here is the
/// twelve bytes that state one entry more, so it is within every other bound.
///
/// The create is refused `workspace.git-storage-limit` and the workspace is proved absent.
/// The address space of the process grows by less than 1 GiB over the create: a table
/// reserved for the entries the pack states would take 2 GiB of it, none of it resident.
#[tokio::test]
async fn a_create_whose_pack_states_more_entries_than_its_byte_limit_holds_is_refused() {
    const ITEM_BYTES: u64 = 64;
    const ADDRESS_SPACE_BOUND_KIB: u64 = 1024 * 1024;
    let stated = u32::try_from(DEFAULT_BYTE_LIMIT / ITEM_BYTES + 1).expect("entries");
    assert_eq!(stated, 33_554_433);
    let mut pack = b"PACK".to_vec();
    pack.extend(2_u32.to_be_bytes());
    pack.extend(stated.to_be_bytes());
    let (source, commit) = repository(0);
    let server = Server::start_with_pack(source.path(), pack);
    let root = tempdir().expect("workspace root");
    let driver = driver(root.path(), &server);
    let before = peak_kib("VmPeak");

    let outcome = driver
        .create_workspace_authorized("ws_a", "ws_a", &input(&server, commit), Some(AUTHORITY))
        .await;

    assert_refused_and_absent(root.path(), outcome);
    hold_peak(
        "the table of entries",
        "growth of the peak address space",
        peak_kib("VmPeak") - before,
        ADDRESS_SPACE_BOUND_KIB,
    );
}

/// Adds `levels` levels of entries behind the entry at `top`, each object of `size` bytes: at
/// each level two entries that nothing is stored behind and one that the next level is stored
/// behind, all three behind that one of the level before. `number` counts the objects.
fn levels_behind(pack: &mut Pack, top: u64, levels: u32, size: u64, number: &mut u64) {
    let mut above = top;
    for _ in 0..levels {
        let mut next = above;
        for _ in 0..3 {
            *number += 1;
            next = pack.behind(above, &version(size, *number));
        }
        above = next;
    }
}

/// Objects that wait. When the fetch computes the ids, an object that other entries are
/// stored behind is held until each of them has been made. The fetch holds no more bytes
/// this way than the byte limit of the create: the object that would take it over ends the
/// fetch.
///
/// The byte limit here is 8 MiB and every object of the pack is 4 MiB. Two entries are stored
/// behind the first, and behind each of those twenty levels of three entries. Whichever of
/// its two threads makes which entry, the third object the fetch would hold comes while the
/// first two are held: each of the two entries behind the first is held until an entry behind
/// it is, and the first until both are. That is 12 MiB. The pack is within every other bound:
/// it has 123 entries, each object is half the byte limit, and making all of them is 492 MiB
/// of the 512 MiB the fetch may make.
///
/// The fetch step ends `workspace.git-storage-limit` and leaves no pack, no branch and
/// nothing checked out in the staging tree. The most memory resident at once in the process
/// stays under 96 MiB.
#[test]
fn a_fetch_whose_pack_makes_more_objects_wait_than_its_byte_limit_holds_is_refused() {
    const OBJECT: u64 = 4 * MEBIBYTE;
    const LEVELS: u32 = 20;
    let limit = StorageLimit {
        max_bytes: 2 * OBJECT,
        max_inodes: 2000,
    };
    let mut pack = Pack::new();
    let first = pack.whole(FILE, &first_version(OBJECT));
    let mut number = 0;
    for _ in 0..2 {
        number += 1;
        let top = pack.behind(first, &version(OBJECT, number));
        levels_behind(&mut pack, top, LEVELS, OBJECT, &mut number);
    }
    assert_eq!(pack.entries, 123);
    assert!(u64::from(pack.entries) * (OBJECT + 32) <= 64 * limit.max_bytes);

    let (outcome, target) = fetch(pack.sent(), limit);

    assert_fetch_refused(outcome, target.path());
    hold_peak(
        "objects that wait",
        "peak resident memory",
        peak_kib("VmHWM"),
        RESIDENT_BOUND_KIB,
    );
}

/// What the fetch makes. When it computes the ids the fetch inflates each entry once and
/// makes the object of each entry stored behind another once. It counts those bytes before
/// it allocates for them, and ends before they come to more than 64 times the byte limit of
/// the create.
///
/// The byte limit here is 1 MiB. The pack is one object of 256 KiB and 300 versions of it,
/// each stored behind the one before it: making them is more than 75 MiB, over the 64 MiB
/// the fetch may make. The pack is within every other bound: it has 301 entries, each object
/// is a quarter of the byte limit, and versions stored in a line hold two of them at once,
/// half the byte limit.
///
/// The fetch step ends `workspace.git-storage-limit` and leaves no pack, no branch and
/// nothing checked out in the staging tree. The most memory resident at once in the process
/// stays under 96 MiB.
#[test]
fn a_fetch_whose_pack_makes_more_than_64_times_its_byte_limit_is_refused() {
    const OBJECT: u64 = 256 * 1024;
    const VERSIONS: u64 = 300;
    let limit = StorageLimit {
        max_bytes: MEBIBYTE,
        max_inodes: 2000,
    };
    assert!((VERSIONS + 1) * OBJECT > 64 * limit.max_bytes);
    let mut pack = Pack::new();
    let mut last = pack.whole(FILE, &first_version(OBJECT));
    for number in 1..=VERSIONS {
        last = pack.behind(last, &version(OBJECT, number));
    }
    assert_eq!(pack.entries, 301);

    let (outcome, target) = fetch(pack.sent(), limit);

    assert_fetch_refused(outcome, target.path());
    hold_peak(
        "what the fetch makes",
        "peak resident memory",
        peak_kib("VmHWM"),
        RESIDENT_BOUND_KIB,
    );
}

/// How many directories the repository of [`history`] has, and how many files each holds.
const DIRECTORIES: usize = 20;
const FILES_EACH: usize = 50;

/// The lines of the file `file` of [`history`] before any commit changed it.
fn lines_of(file: usize) -> Vec<String> {
    (0..60)
        .map(|line| {
            format!(
                "file {file:04} line {line:02} {:016x}\n",
                file * 7919 + line
            )
        })
        .collect()
}

/// A repository whose branch `provider-default` holds `commits` commits of 1,000 files in
/// twenty directories, each commit changing one line in each of twenty files. Beside it the
/// tip and what each file holds there, by its path.
fn history(commits: usize) -> (TempDir, Oid, Vec<(String, String)>) {
    let directory = tempdir().expect("Git source directory");
    let repository = Repository::init(directory.path()).expect("repository");
    let signature =
        Signature::now("Substrate Test", "substrate@example.invalid").expect("signature");
    let mut lines: Vec<Vec<String>> = (0..DIRECTORIES * FILES_EACH).map(lines_of).collect();
    let mut files: Vec<Option<Oid>> = vec![None; lines.len()];
    let mut directories: Vec<Option<Oid>> = vec![None; DIRECTORIES];
    let mut tip: Option<Oid> = None;
    for number in 0..commits {
        for changed in 0..20 {
            let file = (number * 37 + changed * 53) % lines.len();
            lines[file][(number * 11 + changed) % 60] =
                format!("file {file:04} changed in commit {number:02}\n");
            files[file] = None;
            directories[file / FILES_EACH] = None;
        }
        let mut root = repository.treebuilder(None).expect("root");
        for (place, written) in directories.iter_mut().enumerate() {
            let tree = written.unwrap_or_else(|| {
                let mut builder = repository.treebuilder(None).expect("directory");
                for file in place * FILES_EACH..(place + 1) * FILES_EACH {
                    let object = *files[file].get_or_insert_with(|| {
                        repository
                            .blob(lines[file].concat().as_bytes())
                            .expect("file")
                    });
                    builder
                        .insert(
                            format!("file{:02}.txt", file % FILES_EACH),
                            object,
                            0o100_644,
                        )
                        .expect("file");
                }
                builder.write().expect("directory tree")
            });
            *written = Some(tree);
            root.insert(format!("dir{place:02}"), tree, 0o040_000)
                .expect("directory");
        }
        let tree = repository
            .find_tree(root.write().expect("root tree"))
            .expect("tree");
        let parent = tip.map(|tip| repository.find_commit(tip).expect("parent"));
        tip = Some(
            repository
                .commit(
                    Some("refs/heads/provider-default"),
                    &signature,
                    &signature,
                    &format!("commit {number}"),
                    &tree,
                    &parent.iter().collect::<Vec<_>>(),
                )
                .expect("commit"),
        );
    }
    repository
        .set_head("refs/heads/provider-default")
        .expect("provider HEAD");
    let held = lines
        .iter()
        .enumerate()
        .map(|(file, lines)| {
            (
                format!(
                    "dir{:02}/file{:02}.txt",
                    file / FILES_EACH,
                    file % FILES_EACH
                ),
                lines.concat(),
            )
        })
        .collect();
    (directory, tip.expect("a commit"), held)
}

/// A create of an ordinary source is within every bound: 1,000 files in twenty directories
/// with fifty commits of history, each commit changing one line in each of twenty files. It
/// is served. The workspace holds every file as the tip has it and the fifty commits, and the
/// store holds the commit, its twenty-one trees and its 1,000 file objects.
#[tokio::test]
async fn a_create_of_1000_files_with_50_commits_of_history_is_served() {
    let (source, commit, held) = history(50);
    assert_eq!(held.len(), 1000);
    let server = Server::start(source.path(), super::Mode::V2);
    let root = tempdir().expect("workspace root");
    let driver = driver(root.path(), &server);

    let outcome = driver
        .create_workspace_authorized("ws_a", "ws_a", &input(&server, commit), Some(AUTHORITY))
        .await;

    served(outcome);
    let workspace = root.path().join("ws_a");
    for (path, content) in &held {
        assert_eq!(
            std::fs::read_to_string(workspace.join(path)).expect("a file of the workspace"),
            *content,
            "{path}"
        );
    }
    let repository = Repository::open(&workspace).expect("workspace repository");
    let mut walk = repository.revwalk().expect("history");
    walk.push(commit).expect("history start");
    assert_eq!(walk.count(), 50);
    assert_eq!(
        objects_of(&baselines(root.path()).join(".objects/ws_a")).len(),
        1 + 1 + DIRECTORIES + held.len()
    );
}
