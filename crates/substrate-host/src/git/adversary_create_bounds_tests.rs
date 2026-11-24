//! Adversary cases of the bounds a Git create holds on its work and its memory (design 21,
//! § *The work and the memory of a create follow its byte limit*).
//!
//! The rule these cases hold the bounds to: whether a create is within a bound follows its
//! source and its byte limit. The same pack under the same byte limit gets the same answer
//! every time, and a pack of which one line of versions at a time is within the bound of the
//! objects that wait is indexed.
//!
//! Two cases index a pack with the indexer of the fetch itself, eight times each on the
//! threads the fetch of a create names. One runs the production fetch against the real TLS
//! peer and `git upload-pack`. Two more measure a pack within every bound and a pack of whole
//! entries over the bound of what the fetch makes.

use std::io::Write as _;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

use git2::{Oid, Repository, Signature};
use gix::odb::pack;
use substrate_wire::StorageLimit;
use tempfile::{TempDir, tempdir};
use zeroize::Zeroizing;

use super::baseline_store_tests::fetched_packs;
use super::{AUTHORITY, Mode, Server, materialize, repository};
use crate::DriverErrorClass;

const MEBIBYTE: u64 = 1024 * 1024;

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

/// `bytes` as a length in memory.
fn in_memory(bytes: u64) -> usize {
    usize::try_from(bytes).expect("a length in memory")
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
/// of the first but its last eight bytes, then `number`.
fn version(size: u64, number: u64) -> Vec<u8> {
    let taken = size - 8;
    assert!(
        taken < 1 << 24 && taken != 1 << 16,
        "one instruction takes the bytes"
    );
    let mut instructions = [stated_size(size), stated_size(size)].concat();
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
    instructions.push(8);
    instructions.extend(number.to_be_bytes());
    instructions
}

/// Instructions of no more than `most` bytes that make an object from one of `base` bytes
/// and take nothing of it: every instruction states 127 bytes, the first of them `number`.
/// Beside them the bytes of the object they make.
fn stated_whole(base: u64, most: u64, number: u64) -> (Vec<u8>, u64) {
    const STATED: u64 = 127;
    let count = (most - 16) / (STATED + 1);
    let made = count * STATED;
    let mut instructions = [stated_size(base), stated_size(made)].concat();
    for instruction in 0..count {
        instructions.push(seven_bits(STATED));
        let mut stated = [0_u8; 127];
        if instruction == 0 {
            stated[..8].copy_from_slice(&number.to_be_bytes());
        }
        instructions.extend(stated);
    }
    assert!(length(&instructions) <= most);
    (instructions, made)
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

/// A file object of `size` bytes that begins with `number`.
fn file_object(size: u64, number: u64) -> Vec<u8> {
    let mut bytes = vec![0x5a; in_memory(size)];
    bytes[..8].copy_from_slice(&number.to_be_bytes());
    bytes
}

/// The most memory that was resident at once in this process, in kibibytes.
fn peak_resident_kib() -> u64 {
    let status = std::fs::read_to_string("/proc/self/status").expect("process status");
    status
        .lines()
        .find_map(|line| line.strip_prefix("VmHWM:"))
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|number| number.parse().ok())
        .expect("a line of the process status")
}

/// Whether this process runs one case alone, as cargo-nextest says in the environment of
/// each process it starts.
fn alone() -> bool {
    std::env::var("NEXTEST_EXECUTION_MODE").is_ok_and(|mode| mode == "process-per-test")
}

/// How the indexer of the fetch ends on `pack` with `threads` threads under the byte limit
/// `limit`, with the options the fetch gives it.
fn indexed(pack: &[u8], limit: u64, threads: usize) -> Result<(), pack::bundle::write::Error> {
    let directory = tempdir().expect("pack directory");
    pack::Bundle::write_to_directory(
        &mut &pack[..],
        Some(directory.path()),
        &mut gix::progress::Discard,
        &AtomicBool::new(false),
        None::<gix::objs::find::Never>,
        gix::hash::Kind::Sha1,
        pack::bundle::write::Options {
            thread_limit: Some(threads),
            iteration_mode: pack::data::input::Mode::Verify,
            index_version: pack::index::Version::default(),
            alloc_limit_bytes: Some(in_memory(limit)),
            compression: gix::zlib::Compression::BEST_SPEED,
        },
    )
    .map(|_| ())
}

/// How the indexer ends on `pack` as [`indexed`] has it, and with a store of earlier objects
/// to look in that holds none: what the fetch of a create gives it.
fn indexed_as_fetched(
    pack: &[u8],
    limit: u64,
    threads: usize,
) -> Result<(), pack::bundle::write::Error> {
    let directory = tempdir().expect("pack directory");
    pack::Bundle::write_to_directory(
        &mut &pack[..],
        Some(directory.path()),
        &mut gix::progress::Discard,
        &AtomicBool::new(false),
        Some(gix::objs::find::Never),
        gix::hash::Kind::Sha1,
        pack::bundle::write::Options {
            thread_limit: Some(threads),
            iteration_mode: pack::data::input::Mode::Verify,
            index_version: pack::index::Version::default(),
            alloc_limit_bytes: Some(in_memory(limit)),
            compression: gix::zlib::Compression::BEST_SPEED,
        },
    )
    .map(|_| ())
}

/// Starts the measure of the most memory resident at once anew, and answers what is resident
/// now, in kibibytes.
fn resident_from_here_kib() -> u64 {
    std::fs::write("/proc/self/clear_refs", "5").expect("the peak of this process, reset");
    peak_resident_kib()
}

/// Whether `error` is the one the indexer has for an allocation over the byte limit, which a
/// create names `workspace.git-storage-limit`.
fn over_the_limit(error: &pack::bundle::write::Error) -> bool {
    matches!(
        error,
        pack::bundle::write::Error::IndexWrite(pack::index::write::Error::TreeTraversal(
            pack::cache::delta::traverse::Error::OutOfMemory
        ))
    )
}

/// Indexes `pack` under `limit` `attempts` times on the threads the fetch of a create names
/// (`INDEXING_THREADS`, which the settings of that fetch are held to in `create_bounds_tests`).
/// Answers how many of the attempts ended over the byte limit; any other error fails the
/// case.
fn refused_of(case: &str, pack: &[u8], limit: u64, attempts: u32) -> u32 {
    let threads = crate::git::INDEXING_THREADS;
    let mut refused = 0;
    for attempt in 0..attempts {
        match indexed(pack, limit, threads) {
            Ok(()) => {}
            Err(error) if over_the_limit(&error) => refused += 1,
            Err(error) => panic!("{case}: attempt {attempt}: {error:?}"),
        }
    }
    eprintln!(
        "{case}: on {threads} thread(s) the indexer ended over the byte limit in {refused} of \
         {attempts} attempts"
    );
    refused
}

/// Objects that wait: files that have nothing to do with each other. The pack is 24 file
/// objects of 1.2 MiB, each with one entry stored behind it, under a byte limit of 2 MiB. An
/// object waits only while the one entry behind it is made, so one line of versions never
/// holds more than one of them, 1.2 MiB. The pack is within every other bound: 48 entries,
/// each object under the byte limit, and 58 MiB made of the 128 MiB the fetch may make.
///
/// The indexer of the fetch indexes the pack on the threads a create names, each of eight
/// times: the same pack and byte limit get the same answer every time.
#[test]
fn a_pack_of_unrelated_files_within_its_byte_limit_is_indexed_in_each_attempt() {
    const OBJECT: u64 = 1_258_291;
    const FILES: u64 = 24;
    const ATTEMPTS: u32 = 8;
    let limit = 2 * MEBIBYTE;
    assert!((OBJECT..2 * OBJECT).contains(&limit));
    let mut pack = Pack::new();
    for number in 0..FILES {
        let first = pack.whole(FILE, &file_object(OBJECT, number));
        pack.behind(first, &version(OBJECT, number));
    }
    assert_eq!(pack.entries, 48);
    assert!(u64::from(pack.entries) * (OBJECT + 32) <= 64 * limit);
    let started = Instant::now();

    let refused = refused_of("unrelated files", &pack.sent(), limit, ATTEMPTS);

    eprintln!("unrelated files: {:?}", started.elapsed());
    assert_eq!(
        refused, 0,
        "the indexer ends over the byte limit on a pack one line of versions holds within it"
    );
}

/// How many lines each file of [`two_files`] has, how many versions, and how many lines one
/// version changes.
const LINES: usize = 28_000;
const VERSIONS: usize = 6;
const CHANGED: usize = 3_000;

/// The file `name` of [`two_files`] at the version `version`: version 0 is the newest.
/// Each version before it differs from the one after it in 3,000 lines of its own. The two
/// files have no line in common.
fn file_at(name: char, version: usize) -> String {
    (0..LINES)
        .map(|line| {
            let changed = line / CHANGED;
            let written = if (1..=version).contains(&changed) {
                changed
            } else {
                0
            };
            if name == 'a' {
                format!(
                    "file a line {line:06} version {written:02} {:016x}\n",
                    line * 7919
                )
            } else {
                format!(
                    "{:016x} the second file v{written:02} at {line:06}\n",
                    line * 104_729 + 13
                )
            }
        })
        .collect()
}

/// A repository whose branch `provider-default` holds six commits of two text files of
/// 1.26 MiB, each commit changing 3,000 of the 28,000 lines of both. Beside it the tip and
/// the bytes of one file.
fn two_files() -> (TempDir, Oid, u64) {
    let directory = tempdir().expect("Git source directory");
    let repository = Repository::init(directory.path()).expect("repository");
    let signature =
        Signature::now("Substrate Test", "substrate@example.invalid").expect("signature");
    let mut tip: Option<Oid> = None;
    let mut bytes = 0;
    for version in (0..VERSIONS).rev() {
        let mut root = repository.treebuilder(None).expect("root");
        for name in ['a', 'b'] {
            let content = file_at(name, version);
            bytes = length(content.as_bytes());
            let file = repository.blob(content.as_bytes()).expect("file");
            root.insert(format!("{name}.txt"), file, 0o100_644)
                .expect("file");
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
                    &format!("version {version}"),
                    &tree,
                    &parent.iter().collect::<Vec<_>>(),
                )
                .expect("commit"),
        );
    }
    repository
        .set_head("refs/heads/provider-default")
        .expect("provider HEAD");
    (directory, tip.expect("a commit"), bytes)
}

/// The byte limit of the cases on [`two_files`]: 3.5 MiB. The two files are 2.5 MiB and
/// their pack is under 0.5 MiB, so the workspace is within it. The versions of one file are
/// stored one behind the other, which holds two of them at once: 2.5 MiB.
const TWO_FILES_LIMIT: u64 = 3 * MEBIBYTE + MEBIBYTE / 2;

/// The pack `git pack-objects` writes of the branch of the repository at `source`, with
/// entries stored behind others named by the bytes between them, as a fetch asks for them.
fn packed(source: &Path) -> Vec<u8> {
    let mut git = Command::new("git")
        .arg("-C")
        .arg(source)
        .args([
            "pack-objects",
            "--revs",
            "--stdout",
            "--delta-base-offset",
            "--threads=1",
            "-q",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("git pack-objects");
    git.stdin
        .take()
        .expect("standard input")
        .write_all(b"provider-default\n")
        .expect("the branch");
    let output = git.wait_with_output().expect("the pack");
    assert!(output.status.success(), "git pack-objects failed");
    output.stdout
}

/// Objects that wait: two files with history. The source is the repository of
/// [`two_files`], packed by `git pack-objects`, under a byte limit of 3.5 MiB that holds its
/// workspace. One line of versions never holds more than two versions of one file, 2.5 MiB.
///
/// The indexer of the fetch indexes the pack on the threads a create names, each of eight
/// times: the same pack and byte limit get the same answer every time.
#[test]
fn a_pack_of_two_files_with_history_within_its_byte_limit_is_indexed_in_each_attempt() {
    const ATTEMPTS: u32 = 8;
    let (source, _, file) = two_files();
    assert!(2 * file <= TWO_FILES_LIMIT);
    let pack = packed(source.path());
    assert!(2 * file + length(&pack) <= TWO_FILES_LIMIT);
    eprintln!(
        "two files with history: files of {file} bytes, a pack of {} bytes",
        pack.len()
    );
    let started = Instant::now();

    let refused = refused_of("two files with history", &pack, TWO_FILES_LIMIT, ATTEMPTS);

    eprintln!("two files with history: {:?}", started.elapsed());
    assert_eq!(
        refused, 0,
        "the indexer ends over the byte limit on a pack one line of versions holds within it"
    );
}

/// A create of two files with history is within every bound under a byte limit that holds
/// its workspace with a sixth to spare: the repository of [`two_files`] under 3.5 MiB,
/// fetched from `git upload-pack` by the production fetch. The fetch step is served, each of
/// four times, and checks out both files.
#[test]
fn a_fetch_of_two_files_with_history_within_its_byte_limit_is_served() {
    const ATTEMPTS: u32 = 4;
    let (source, commit, file) = two_files();
    let server = Server::start(source.path(), Mode::V2);
    let limit = StorageLimit {
        max_bytes: TWO_FILES_LIMIT,
        max_inodes: 2000,
    };
    let started = Instant::now();
    let mut refused = Vec::new();
    for attempt in 0..ATTEMPTS {
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
        match outcome {
            Ok(usage) => {
                eprintln!("two files fetched: attempt {attempt}: served, {usage:?}");
                assert!(usage.used_bytes <= TWO_FILES_LIMIT);
                for name in ["a.txt", "b.txt"] {
                    let held = std::fs::metadata(target.path().join(name)).expect("a file");
                    assert_eq!(held.len(), file);
                }
            }
            Err(error) => {
                eprintln!(
                    "two files fetched: attempt {attempt}: refused {}",
                    error.code
                );
                refused.push(error.code);
            }
        }
    }
    eprintln!("two files fetched: {:?}", started.elapsed());
    assert_eq!(
        refused,
        Vec::<&str>::new(),
        "a fetch within its byte limit was refused in {} of {ATTEMPTS} attempts",
        refused.len()
    );
}

/// Has this process give every allocation of 128 KiB or more back to the system when it is
/// freed, so that what is resident is what is held: without it, memory a case freed stays
/// resident and is used again, and what grows afterwards reads as less than what is held.
fn give_back_what_is_freed() {
    #[cfg(target_env = "gnu")]
    {
        // SAFETY: `mallopt` takes two integers and no pointer.
        let set = unsafe { libc::mallopt(libc::M_MMAP_THRESHOLD, 128 * 1024) };
        assert_eq!(set, 1, "the allocator keeps the threshold");
    }
}

/// The memory of the indexer on a pack within every bound under the byte limit `limit`. The
/// pack is one file object of the byte limit and six entries stored behind it, each with
/// instructions of the byte limit that make an object of nearly the byte limit. One object
/// waits, the byte limit; the one thread works on one entry at a time, its instructions and
/// the object they make. The indexer makes 13 times the byte limit of the 64 it may.
///
/// The pack is indexed on the threads a create names: as [`indexed`] has it, or with
/// `beside_a_store` beside a store of earlier objects, as the fetch of a create has it. Where
/// the case is a process of its own, the process gives back what it frees
/// ([`give_back_what_is_freed`]), and over the indexing the most memory resident at once
/// grows by less than four times the byte limit: about three times was measured, the object
/// that waits, the instructions and the object they make, and the fourth is the margin.
/// Answers the most memory that was resident at once, in kibibytes.
fn memory_within_every_bound(limit: u64, beside_a_store: bool) -> u64 {
    const BEHIND_IT: u64 = 6;
    const GROWTH_BOUND: u64 = 4;
    let threads = crate::git::INDEXING_THREADS;
    if alone() {
        give_back_what_is_freed();
    }
    let mut pack = Pack::new();
    let base = pack.whole(FILE, &file_object(limit, 0));
    let mut made_in_all = limit;
    for number in 1..=BEHIND_IT {
        let (instructions, made) = stated_whole(limit, limit, number);
        made_in_all += length(&instructions) + made;
        pack.behind(base, &instructions);
    }
    assert!(made_in_all <= 64 * limit);
    let pack = pack.sent();
    assert!(length(&pack) <= limit);
    let highest = peak_resident_kib();
    let before = resident_from_here_kib();
    let started = Instant::now();

    let outcome = if beside_a_store {
        indexed_as_fetched(&pack, limit, threads)
    } else {
        indexed(&pack, limit, threads)
    };

    let peak = peak_resident_kib();
    let grown = peak.saturating_sub(before);
    eprintln!(
        "memory within every bound: a byte limit of {limit}, {threads} thread(s), beside a \
         store {beside_a_store}: {:?}, a pack of {} bytes, peak resident memory {peak} KiB \
         ({before} KiB before the indexer, {highest} KiB at the most while the pack was made), \
         grown by {} hundredths of the byte limit; the indexer made {made_in_all} bytes",
        started.elapsed(),
        pack.len(),
        grown * 100 / (limit / 1024),
    );
    outcome.expect("a pack within every bound");
    if alone() {
        assert!(
            grown <= GROWTH_BOUND * limit / 1024,
            "peak resident memory grew by {grown} KiB, over {GROWTH_BOUND} times the byte limit \
             {limit}"
        );
    } else {
        eprintln!("memory within every bound: the peak is not asserted beside other cases");
    }
    peak.max(highest)
}

/// The memory of the indexer on a pack within every bound, under a byte limit of 16 MiB
/// ([`memory_within_every_bound`]). The most memory resident at once in the process also
/// stays under eight times the byte limit, 128 MiB.
#[test]
fn an_indexer_within_every_bound_keeps_its_memory_under_four_times_its_byte_limit() {
    const LIMIT: u64 = 16 * MEBIBYTE;
    let peak = memory_within_every_bound(LIMIT, false);
    if alone() {
        assert!(
            peak <= 8 * LIMIT / 1024,
            "peak resident memory {peak} KiB is over eight times the byte limit"
        );
    }
}

/// The same at half the byte limit, 8 MiB: the memory follows the limit.
#[test]
fn an_indexer_within_every_bound_keeps_its_memory_under_four_times_a_byte_limit_of_8_mib() {
    memory_within_every_bound(8 * MEBIBYTE, false);
}

/// The same beside a store of earlier objects, as the fetch of a create indexes its pack,
/// under a byte limit of 16 MiB.
#[test]
fn an_indexer_beside_a_store_keeps_its_memory_under_four_times_its_byte_limit() {
    memory_within_every_bound(16 * MEBIBYTE, true);
}

/// The same beside a store of earlier objects under a byte limit of 8 MiB.
#[test]
fn an_indexer_beside_a_store_keeps_its_memory_under_four_times_a_byte_limit_of_8_mib() {
    memory_within_every_bound(8 * MEBIBYTE, true);
}

/// What the fetch makes, of entries stored as they are. The byte limit is 1 MiB. The pack is
/// 96 file objects of 1 MiB, none stored behind another: 96 MiB to inflate, over the
/// 64 MiB the fetch may make. The pack is within every other bound: 96 entries, each object
/// the byte limit, nothing waits, and the pack itself is under the byte limit.
///
/// The fetch step ends `workspace.git-storage-limit` and leaves no pack in the staging tree.
#[test]
fn a_fetch_whose_whole_entries_inflate_to_more_than_64_times_its_byte_limit_is_refused() {
    const ENTRIES: u64 = 96;
    let limit = StorageLimit {
        max_bytes: MEBIBYTE,
        max_inodes: 2000,
    };
    let mut pack = Pack::new();
    for number in 0..ENTRIES {
        pack.whole(FILE, &file_object(MEBIBYTE, number));
    }
    let pack = pack.sent();
    assert!(length(&pack) <= limit.max_bytes, "{} bytes", pack.len());
    let sent = pack.len();
    let (source, commit) = repository(0);
    let server = Server::start_with_pack(source.path(), pack);
    let target = tempdir().expect("staging tree");
    let started = Instant::now();

    let outcome = materialize(
        target.path(),
        &server.locator,
        &server.binding,
        &server.source(commit),
        &Zeroizing::new(AUTHORITY.to_owned()),
        limit,
        &Arc::new(AtomicBool::new(false)),
    );

    eprintln!(
        "whole entries: a pack of {sent} bytes that inflates to {} MiB ended in {:?}",
        ENTRIES,
        started.elapsed()
    );
    let error = outcome.expect_err("a pack over the bound of what the fetch makes");
    assert_eq!(error.code, "workspace.git-storage-limit");
    assert_eq!(error.class, DriverErrorClass::Exhausted);
    assert_eq!(
        fetched_packs(target.path()),
        Vec::<String>::new(),
        "the fetch finished its pack"
    );
}
