//! Confirming cases of the bounds a Git create holds on its work and its memory (design 21,
//! § *The work and the memory of a create follow its byte limit*).
//!
//! Each case gives a pack written by hand to the indexer of the fetch, with the options the
//! fetch of a create gives it and on the threads a create names. The indexer reads a pack in
//! one of four ways ([`Way`]); the fetch of a create takes the second. The cases hold three
//! rules:
//!
//! - what the entries inflate to while the pack arrives is counted to the byte on each way,
//!   an entry is read no further than the bytes it states and one more, and a well-formed
//!   pack is indexed to the same bytes with a byte limit and without one;
//! - what the indexer keeps for an entry of a full table is no more than the table counts
//!   for it;
//! - a pack within every count that does not deflate keeps the memory of the indexer under
//!   the multiple of the byte limit the cases beside this file hold.
//!
//! The cases on memory assert where cargo-nextest runs each in a process of its own, and say
//! in one line that they do not where other cases run beside them.

use std::io::Cursor;
use std::path::Path;
use std::process::Command;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

use gix::odb::pack;
use tempfile::tempdir;

use super::{
    BEHIND, BEHIND_AN_ID, FILE, MEBIBYTE, Pack, TREE, deflated, distance, entry_header, instead,
    length, peak_kib, stated_size,
};

/// No case interrupts the indexer.
static NOT_INTERRUPTED: AtomicBool = AtomicBool::new(false);

/// The ways the indexer reads a pack: from the caller's reader or from a thread of its own,
/// each alone or beside a store of earlier objects. The fetch of a create reads from its own
/// reader beside a store that holds no object.
#[derive(Clone, Copy, Debug)]
enum Way {
    Alone,
    BesideAStore,
    Eagerly,
    EagerlyBesideAStore,
}

const WAYS: [Way; 4] = [
    Way::Alone,
    Way::BesideAStore,
    Way::Eagerly,
    Way::EagerlyBesideAStore,
];

impl Way {
    /// Whether the cases ask this way for the ids of a pack. The last way is asked only how
    /// a pack arrives: no create takes it, and the cases do not ask it for ids.
    fn computes_ids(self) -> bool {
        !matches!(self, Way::EagerlyBesideAStore)
    }
}

/// `bytes` as a length in memory.
fn in_memory(bytes: u64) -> usize {
    usize::try_from(bytes).expect("a length in memory")
}

/// The options the fetch of a create gives the indexer, with the byte limit `limit`.
fn options(limit: Option<u64>) -> pack::bundle::write::Options {
    pack::bundle::write::Options {
        thread_limit: Some(crate::git::INDEXING_THREADS),
        iteration_mode: pack::data::input::Mode::Verify,
        index_version: pack::index::Version::default(),
        alloc_limit_bytes: limit.map(in_memory),
        compression: gix::zlib::Compression::BEST_SPEED,
    }
}

/// Indexes `sent` into `directory` on `way` under `limit`.
fn write(
    sent: &[u8],
    limit: Option<u64>,
    way: Way,
    directory: &Path,
) -> Result<pack::bundle::write::Outcome, pack::bundle::write::Error> {
    let sha1 = gix::hash::Kind::Sha1;
    match way {
        Way::Alone => pack::Bundle::write_to_directory(
            &mut &sent[..],
            Some(directory),
            &mut gix::progress::Discard,
            &NOT_INTERRUPTED,
            None::<gix::objs::find::Never>,
            sha1,
            options(limit),
        ),
        Way::BesideAStore => pack::Bundle::write_to_directory(
            &mut &sent[..],
            Some(directory),
            &mut gix::progress::Discard,
            &NOT_INTERRUPTED,
            Some(gix::objs::find::Never),
            sha1,
            options(limit),
        ),
        Way::Eagerly => pack::Bundle::write_to_directory_eagerly(
            Box::new(Cursor::new(sent.to_vec())),
            None,
            Some(directory),
            &mut gix::progress::Discard,
            &NOT_INTERRUPTED,
            None::<gix::objs::find::Never>,
            sha1,
            options(limit),
        ),
        Way::EagerlyBesideAStore => pack::Bundle::write_to_directory_eagerly(
            Box::new(Cursor::new(sent.to_vec())),
            None,
            Some(directory),
            &mut gix::progress::Discard,
            &NOT_INTERRUPTED,
            Some(gix::objs::find::Never),
            sha1,
            options(limit),
        ),
    }
}

/// What the indexer wrote of a pack: what it answers, its index file and its pack file.
#[derive(Debug, PartialEq, Eq)]
struct Indexed {
    index: pack::index::write::Outcome,
    index_file: Vec<u8>,
    pack_file: Vec<u8>,
}

/// Indexes `sent` on `way` under `limit` and reads back what was written.
fn indexed(
    sent: &[u8],
    limit: Option<u64>,
    way: Way,
) -> Result<Indexed, pack::bundle::write::Error> {
    let directory = tempdir().expect("pack directory");
    let written = write(sent, limit, way, directory.path())?;
    let read = |path: Option<std::path::PathBuf>| {
        std::fs::read(path.expect("a file of the pack")).expect("the bytes of the file")
    };
    Ok(Indexed {
        index: written.index,
        index_file: read(written.index_path),
        pack_file: read(written.data_path),
    })
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

/// The error of an entry that arrived, where `error` is one.
fn of_an_entry(error: &pack::bundle::write::Error) -> Option<&pack::data::input::Error> {
    match error {
        pack::bundle::write::Error::PackIter(error)
        | pack::bundle::write::Error::IndexWrite(pack::index::write::Error::PackEntryDecode(
            error,
        )) => Some(error),
        _ => None,
    }
}

/// `size` bytes that begin with `number` where there is room for it.
fn file_of(size: u64, number: u64) -> Vec<u8> {
    let mut bytes = vec![0x5a; in_memory(size)];
    for (place, byte) in bytes.iter_mut().zip(number.to_be_bytes()) {
        *place = byte;
    }
    bytes
}

/// What arrives is counted to the byte, on each way the indexer reads a pack. The byte limit
/// is 64 KiB, so the entries may inflate to 4 MiB while the pack arrives.
///
/// A pack of 64 file objects of 64 KiB inflates to exactly that and is indexed, or on the
/// way that is asked for no ids ([`Way::computes_ids`]) ends after its arrival with an error
/// that is neither the byte limit's nor an entry's. A second
/// pack has one file object of one byte more and, after it, an entry that is no stream: the
/// indexer ends at the entry of one byte with the error a create names
/// `workspace.git-storage-limit`, and never reads the entry after it, which would end it
/// with another error.
#[test]
fn an_indexer_counts_what_arrives_to_the_byte_on_each_way_it_reads_a_pack() {
    const LIMIT: u64 = 64 * 1024;
    const ADMITTED: u64 = 64;
    let pack_of = |one_byte_more: bool| {
        let mut pack = Pack::new();
        for number in 0..ADMITTED {
            pack.whole(FILE, &file_of(LIMIT, number));
        }
        if one_byte_more {
            pack.whole(FILE, &[0x5a]);
            pack.bytes.extend(entry_header(FILE, 8));
            pack.bytes.extend([0xff; 16]);
            pack.entries += 1;
        }
        pack.sent()
    };
    let within = pack_of(false);
    let over = pack_of(true);
    for way in WAYS {
        let started = Instant::now();
        let outcome = indexed(&within, Some(LIMIT), way);
        match outcome {
            Ok(written) => assert_eq!(u64::from(written.index.num_objects), ADMITTED, "{way:?}"),
            Err(error) if !way.computes_ids() => assert!(
                !over_the_limit(&error) && of_an_entry(&error).is_none(),
                "{way:?}: a pack that inflates to the bound: {error:?}"
            ),
            Err(error) => panic!("{way:?}: a pack that inflates to the bound: {error:?}"),
        }
        let error = indexed(&over, Some(LIMIT), way)
            .expect_err("a pack that inflates to one byte more than the bound");
        eprintln!(
            "arrival to the byte: {way:?}: {:?}, one byte more ended {error:?}",
            started.elapsed()
        );
        assert!(over_the_limit(&error), "{way:?}: {error:?}");
    }
}

/// While a pack arrives the indexer inflates no more of an entry than the bytes it states
/// and one more. The byte limit is 64 KiB. The one entry of the pack states eight bytes and
/// its stream holds 16 MiB, four times what the entries may inflate to.
///
/// On each way the indexer ends as on every pack that is not well formed, with the error of
/// an entry that holds nine bytes where it stated eight, and not with the error of the byte
/// limit.
#[test]
fn an_entry_that_holds_more_than_it_states_is_read_one_byte_further_and_no_more() {
    const LIMIT: u64 = 64 * 1024;
    const STATED: u64 = 8;
    let mut pack = Pack::new();
    pack.bytes.extend(entry_header(FILE, STATED));
    pack.bytes
        .extend(deflated(&vec![0; in_memory(16 * MEBIBYTE)]));
    pack.entries += 1;
    let sent = pack.sent();
    for way in WAYS {
        let started = Instant::now();
        let error = indexed(&sent, Some(LIMIT), way).expect_err("an entry that is not well formed");
        eprintln!(
            "one byte further: {way:?}: {:?}, ended {error:?}",
            started.elapsed()
        );
        match of_an_entry(&error) {
            Some(pack::data::input::Error::IncompletePack { actual, expected }) => {
                assert_eq!((*actual, *expected), (STATED + 1, STATED), "{way:?}");
            }
            _ => panic!("{way:?}: {error:?}"),
        }
    }
}

/// `size` bytes that differ from place to place and with `seed`.
fn patterned(size: usize, seed: usize) -> Vec<u8> {
    (0..size)
        .map(|place| u8::try_from((place * 31 + seed) % 251).expect("a byte"))
        .collect()
}

/// A zlib stream that holds `bytes` as they are in blocks of `block` bytes, and after them
/// `empty` blocks that hold nothing.
fn stored_in_blocks(bytes: &[u8], block: usize, empty: usize) -> Vec<u8> {
    let mut blocks: Vec<&[u8]> = bytes.chunks(block).collect();
    blocks.extend(std::iter::repeat_n(&[][..], empty));
    if blocks.is_empty() {
        blocks.push(&[]);
    }
    let last = blocks.len() - 1;
    let mut stream = vec![0x78, 0x01];
    for (place, block) in blocks.into_iter().enumerate() {
        let size = u16::try_from(block.len()).expect("one block");
        stream.push(u8::from(place == last));
        stream.extend(size.to_le_bytes());
        stream.extend((!size).to_le_bytes());
        stream.extend(block);
    }
    let (mut low, mut high) = (1_u32, 0_u32);
    for byte in bytes {
        low = (low + u32::from(*byte)) % 65_521;
        high = (high + low) % 65_521;
    }
    stream.extend(((high << 16) | low).to_be_bytes());
    stream
}

/// A well-formed pack whose entries end at and around the sizes the indexer reads in one
/// step: a file object of no bytes, deflated, and a directory of no entries, held in three
/// empty blocks; file objects of 1, 8,191, 8,192, 8,193 and 65,536 bytes, each once
/// deflated, once as it is in blocks of 4,096 bytes and once as it is in blocks of 8,192
/// bytes with three empty blocks after them; one entry stored behind another by its place
/// and one stored behind another by its id.
fn well_formed() -> Vec<u8> {
    let mut pack = Pack::new();
    pack.whole(FILE, &[]);
    pack.bytes.extend(entry_header(TREE, 0));
    pack.bytes.extend(stored_in_blocks(&[], 8192, 3));
    pack.entries += 1;
    let mut first = None;
    let mut seed = 0;
    for size in [1_usize, 8191, 8192, 8193, 65_536] {
        for stream in 0..3 {
            seed += 1;
            let content = patterned(size, seed);
            let offset = length(&pack.bytes);
            pack.bytes.extend(entry_header(FILE, length(&content)));
            pack.bytes.extend(match stream {
                0 => deflated(&content),
                1 => stored_in_blocks(&content, 4096, 0),
                _ => stored_in_blocks(&content, 8192, 3),
            });
            pack.entries += 1;
            if size == 8191 && first.is_none() {
                first = Some((offset, content));
            }
        }
    }
    let (offset, content) = first.expect("an object to store entries behind");
    pack.behind(
        offset,
        &instead(length(&content), b"stored behind its place"),
    );
    let id = gix::objs::compute_hash(gix::hash::Kind::Sha1, gix::objs::Kind::Blob, &content)
        .expect("the id of a file object");
    let instructions = instead(length(&content), b"stored behind its id");
    pack.bytes
        .extend(entry_header(BEHIND_AN_ID, length(&instructions)));
    pack.bytes.extend(id.as_slice());
    pack.bytes.extend(deflated(&instructions));
    pack.entries += 1;
    pack.sent()
}

/// The index `git index-pack` writes of `sent`.
fn index_of_git(sent: &[u8]) -> Vec<u8> {
    let directory = tempdir().expect("pack directory");
    let pack = directory.path().join("sent.pack");
    let index = directory.path().join("sent.idx");
    std::fs::write(&pack, sent).expect("the pack");
    let output = Command::new("git")
        .current_dir(directory.path())
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .arg("index-pack")
        .arg("-o")
        .arg(&index)
        .arg(&pack)
        .output()
        .expect("git index-pack");
    assert!(
        output.status.success(),
        "git index-pack: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    std::fs::read(index).expect("the index")
}

/// A well-formed pack is indexed to the same bytes under a byte limit and without one: that
/// the indexer inflates no more of an entry than it states and one more changes nothing of
/// what it answers for an entry that holds what it states. The pack is [`well_formed`], 19
/// entries that inflate to under 4 MiB, the largest of 64 KiB.
///
/// On each way that computes ids ([`Way::computes_ids`]) the indexer answers the same, and
/// writes the same index file and the same pack file, without a byte limit, under 64 KiB
/// and under 1 MiB. The pack file is the pack that was sent, and the index file is the one
/// `git index-pack` writes of it.
#[test]
fn a_well_formed_pack_is_indexed_to_the_same_bytes_with_a_byte_limit_and_without_one() {
    let sent = well_formed();
    let git = index_of_git(&sent);
    for way in WAYS.into_iter().filter(|way| way.computes_ids()) {
        let started = Instant::now();
        let unbounded = indexed(&sent, None, way).expect("a well-formed pack");
        assert_eq!(unbounded.index.num_objects, 19, "{way:?}");
        assert!(
            unbounded.pack_file == sent,
            "{way:?}: the pack file is not the pack that was sent"
        );
        assert!(
            unbounded.index_file == git,
            "{way:?}: the index file is not the one git writes"
        );
        for limit in [64 * 1024, MEBIBYTE] {
            let bounded = indexed(&sent, Some(limit), way).expect("a well-formed pack");
            assert!(
                bounded == unbounded,
                "{way:?}: a byte limit of {limit} changes what is written: {:?} and {:?}",
                bounded.index,
                unbounded.index
            );
        }
        eprintln!("well formed: {way:?}: {:?}", started.elapsed());
    }
}

/// Whether this process runs one case alone, as cargo-nextest says in the environment of
/// each process it starts.
fn alone() -> bool {
    std::env::var("NEXTEST_EXECUTION_MODE").is_ok_and(|mode| mode == "process-per-test")
}

/// Has this process give every allocation of 128 KiB or more back to the system when it is
/// freed, so that what is resident is what is held.
fn give_back_what_is_freed() {
    #[cfg(target_env = "gnu")]
    {
        // SAFETY: `mallopt` takes two integers and no pointer.
        let set = unsafe { libc::mallopt(libc::M_MMAP_THRESHOLD, 128 * 1024) };
        assert_eq!(set, 1, "the allocator keeps the threshold");
    }
}

/// Gives the memory this process freed back to the system, starts the measure of the most
/// memory resident at once anew, and answers what is resident now, in kibibytes.
fn resident_from_here_kib() -> u64 {
    #[cfg(target_env = "gnu")]
    {
        // SAFETY: `malloc_trim` takes one integer and no pointer.
        unsafe { libc::malloc_trim(0) };
    }
    std::fs::write("/proc/self/clear_refs", "5").expect("the peak of this process, reset");
    peak_kib("VmHWM")
}

/// What the entries of a full table are.
#[derive(Clone, Copy, Debug)]
enum Entries {
    /// File objects of eight bytes: 128 bytes of the table each.
    Whole,
    /// A file object of eight bytes and one entry stored behind it: 288 bytes of the table
    /// for the two.
    Pairs,
    /// Entries that each name another id, which no object of the pack has: 288 bytes of the
    /// table each.
    NamingAnId,
}

/// A pack whose table is full under the byte limit `limit`. Beside it the number of its
/// entries.
fn full_table(entries: Entries, limit: u64) -> (Vec<u8>, u64) {
    let mut pack = Pack::new();
    match entries {
        Entries::Whole => {
            for number in 0..limit / 128 {
                pack.small(number);
            }
        }
        Entries::Pairs => {
            for number in 0..limit / 288 {
                let first = pack.small(2 * number);
                pack.small_behind(first, 2 * number + 1);
            }
        }
        Entries::NamingAnId => {
            for number in 0..limit / 288 {
                pack.small_behind_an_id(number);
            }
        }
    }
    let count = u64::from(pack.entries);
    (pack.sent(), count)
}

/// What one indexing of a full table took.
struct Taken {
    entries: u64,
    pack_bytes: u64,
    grown_kib: u64,
}

/// Indexes a full table of `entries` under `limit` as the fetch of a create does, and
/// answers by how much the most memory resident at once grew over it.
fn taken_by_a_full_table(entries: Entries, limit: u64) -> Taken {
    let (sent, count) = full_table(entries, limit);
    let directory = tempdir().expect("pack directory");
    let before = resident_from_here_kib();
    let started = Instant::now();

    let outcome = write(&sent, Some(limit), Way::BesideAStore, directory.path());

    let grown_kib = peak_kib("VmHWM").saturating_sub(before);
    eprintln!(
        "a full table: {entries:?} under {limit}: {count} entries, a pack of {} bytes, {:?}, \
         peak resident memory grown by {grown_kib} KiB, {} hundredths of the byte limit",
        sent.len(),
        started.elapsed(),
        grown_kib * 100 / (limit / 1024),
    );
    match (entries, outcome) {
        (Entries::Whole | Entries::Pairs, Ok(written)) => {
            assert_eq!(u64::from(written.index.num_objects), count);
        }
        (Entries::NamingAnId, Err(error)) => assert!(
            !over_the_limit(&error),
            "a table within its count: {error:?}"
        ),
        (_, Ok(_)) => panic!("{entries:?}: no entry has the object it names"),
        (_, Err(error)) => panic!("{entries:?}: a table within its count: {error:?}"),
    }
    Taken {
        entries: count,
        pack_bytes: length(&sent),
        grown_kib,
    }
}

/// What the indexer keeps for one entry between two full tables: what the peak resident
/// memory grew by more over `second` than over `first`, for each entry the second has more,
/// less the bytes of the pack itself, which the indexer reads from its file.
fn kept_for_an_entry(entries: Entries, first: &Taken, second: &Taken) -> u64 {
    let more = second.entries - first.entries;
    let grown = second.grown_kib.saturating_sub(first.grown_kib) * 1024 / more;
    let of_the_pack = (second.pack_bytes - first.pack_bytes) / more;
    let kept = grown.saturating_sub(of_the_pack);
    eprintln!(
        "a full table: {entries:?}: from {} to {} entries the peak grew by {grown} bytes for \
         each, {of_the_pack} of them the pack itself: {kept} bytes kept",
        first.entries, second.entries
    );
    kept
}

/// Holds what the indexer keeps for one entry of a full table of `entries` to `counted`, the
/// bytes the table counts for one. After one indexing of a small table, which takes what a
/// process pays once, the table is filled under 8 MiB, 16 MiB and 32 MiB. What the indexer
/// keeps for one entry is read between the last two ([`kept_for_an_entry`]): under 8 MiB
/// the peak the system keeps for a process is not exact enough for one entry (design 21,
/// *The memory of a full table*). The figure between the first two is printed beside it.
fn held_to_its_count(entries: Entries, counted: u64) {
    if alone() {
        give_back_what_is_freed();
    }
    taken_by_a_full_table(entries, 64 * 1024);
    let small = taken_by_a_full_table(entries, 8 * MEBIBYTE);
    let first = taken_by_a_full_table(entries, 16 * MEBIBYTE);
    let second = taken_by_a_full_table(entries, 32 * MEBIBYTE);
    kept_for_an_entry(entries, &small, &first);
    let kept = kept_for_an_entry(entries, &first, &second);
    eprintln!("a full table: {entries:?}: {kept} bytes kept for an entry, {counted} counted");
    if alone() {
        assert!(
            kept <= counted,
            "{entries:?}: the indexer keeps {kept} bytes for an entry the table counts as \
             {counted}"
        );
    } else {
        eprintln!("a full table: the peak is not asserted beside other cases");
    }
}

/// The table of entries counts an entry stored as it is as 128 bytes, and the indexer keeps
/// no more for it ([`held_to_its_count`]).
#[test]
fn an_indexer_keeps_no_more_for_an_entry_stored_as_it_is_than_its_table_counts() {
    held_to_its_count(Entries::Whole, 128);
}

/// The table of entries counts an object with one entry stored behind it as 288 bytes, 144
/// for each of the two entries, and the indexer keeps no more for them
/// ([`held_to_its_count`]). Quarantined on a C library older than glibc 2.43, the one release
/// on which the figure was measured to hold: on glibc 2.34 it moved between 99 and 150 bytes
/// from run to run (`story:full-table-memory-cases-hold-on-any-c-library`).
#[test]
fn an_indexer_keeps_no_more_for_an_entry_stored_behind_another_than_its_table_counts() {
    if let Some(version) = c_library_older_than(2, 43) {
        println!(
            "QUARANTINED (story:full-table-memory-cases-hold-on-any-c-library): glibc {version} \
             is older than 2.43, on which the figure for an entry stored behind another holds"
        );
        return;
    }
    held_to_its_count(Entries::Pairs, 144);
}

/// The version of the GNU C library this process runs on, where it is older than
/// `major.minor`; `None` on that release or a later one, and on another C library.
fn c_library_older_than(major: u32, minor: u32) -> Option<String> {
    #[cfg(target_env = "gnu")]
    {
        // SAFETY: `gnu_get_libc_version` takes nothing and answers a pointer to a static,
        // NUL-terminated string.
        let version = unsafe { std::ffi::CStr::from_ptr(libc::gnu_get_libc_version()) }
            .to_string_lossy()
            .into_owned();
        let mut parts = version
            .split('.')
            .map(|part| part.parse::<u32>().unwrap_or(0));
        let found = (parts.next().unwrap_or(0), parts.next().unwrap_or(0));
        (found < (major, minor)).then_some(version)
    }
    #[cfg(not(target_env = "gnu"))]
    {
        let _ = (major, minor);
        None
    }
}

/// The table of entries counts an entry that names an id no earlier entry named as 288
/// bytes, and the indexer keeps no more for it ([`held_to_its_count`]).
#[test]
fn an_indexer_keeps_no_more_for_an_entry_that_names_an_id_than_its_table_counts() {
    held_to_its_count(Entries::NamingAnId, 288);
}

/// `count` bytes that do not deflate.
fn unlike_bytes(count: usize) -> Vec<u8> {
    let mut state = 0x9e37_79b9_7f4a_7c15_u64;
    let mut bytes = Vec::with_capacity(count + 8);
    while bytes.len() < count {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        bytes.extend(state.to_le_bytes());
    }
    bytes.truncate(count);
    bytes
}

/// Instructions of no more than `most` bytes that make an object from one of `base` bytes
/// and take nothing of it: every instruction states 127 bytes that do not deflate. Beside
/// them the bytes of the object they make.
fn stated_whole_of_unlike_bytes(base: u64, most: u64) -> (Vec<u8>, u64) {
    let made = (most - 16) / 128 * 127;
    let mut instructions = [stated_size(base), stated_size(made)].concat();
    for stated in unlike_bytes(in_memory(made)).chunks(127) {
        instructions.push(127);
        instructions.extend(stated);
    }
    assert!(length(&instructions) <= most);
    (instructions, made)
}

/// Indexes `sent`, a pack of two entries within every bound, under `limit` as the fetch of a
/// create does, and answers by how much the most memory resident at once grew over it, in
/// kibibytes. One line says how much of what is resident afterwards is pages of files and
/// how much is not.
fn grown_over_an_indexing(sent: &[u8], limit: u64) -> u64 {
    let directory = tempdir().expect("pack directory");
    let before = resident_from_here_kib();
    let (files, other) = (peak_kib("RssFile"), peak_kib("RssAnon"));
    let started = Instant::now();

    let outcome = write(sent, Some(limit), Way::BesideAStore, directory.path());

    let grown = peak_kib("VmHWM").saturating_sub(before);
    eprintln!(
        "an indexing under {limit}: {:?}, peak resident memory grown by {grown} KiB, {} \
         hundredths of the byte limit; resident afterwards: {} KiB more of files, {} KiB more \
         of other memory",
        started.elapsed(),
        grown * 100 / (limit / 1024),
        peak_kib("RssFile").saturating_sub(files),
        peak_kib("RssAnon").saturating_sub(other),
    );
    let written = outcome.expect("a pack within every bound");
    assert_eq!(written.index.num_objects, 2);
    grown
}

/// The memory of the indexer on a pack within every bound that does not deflate, under the
/// byte limit `limit`. The pack is one file object of the byte limit and one entry stored
/// behind it, whose instructions the pack holds as they are; they fill what is left of the
/// byte limit for the pack and make
/// an object of nearly their size. The pack is no larger than the byte limit, as the pack of
/// a create is; one object waits, the byte limit; the instructions and the object they make
/// are each within it; the indexer makes less than three times the byte limit.
///
/// The pack is indexed twice as the fetch of a create indexes it: the first indexing of a
/// process also takes what the process pays once, the pages of its own program among them,
/// and is printed. Where the case is a process of its own, the most memory resident at once
/// grows over the second indexing by less than four times the byte limit, the multiple the
/// cases on a pack that deflates hold (`adversary_create_bounds_tests`).
fn memory_of_a_pack_that_does_not_deflate(limit: u64) {
    const GROWTH_BOUND: u64 = 4;
    const CHECKSUM: u64 = 20;
    if alone() {
        give_back_what_is_freed();
    }
    let mut pack = Pack::new();
    let base = pack.whole(FILE, &vec![0x5a; in_memory(limit)]);
    let begins = length(&pack.bytes);
    let room = limit - begins - CHECKSUM;
    // What the entry takes beside its instructions: its header, the bytes to the entry it is
    // stored behind, and five bytes for each block of the stream with six around them.
    let beside = 16 + 6 + 5 * (room / 65_535 + 1);
    let (instructions, made) = stated_whole_of_unlike_bytes(limit, room - beside);
    let stated = length(&instructions);
    pack.bytes.extend(entry_header(BEHIND, stated));
    pack.bytes.extend(distance(begins - base));
    pack.bytes
        .extend(stored_in_blocks(&instructions, 65_535, 0));
    pack.entries += 1;
    drop(instructions);
    let sent = pack.sent();
    assert!(length(&sent) <= limit, "a pack of {} bytes", sent.len());
    assert!(stated <= limit && made <= limit);
    assert!(limit + stated + made <= 64 * limit);
    eprintln!(
        "a pack that does not deflate: a byte limit of {limit}: a pack of {} bytes, \
         instructions of {stated} bytes that make {made}",
        sent.len()
    );
    let first = grown_over_an_indexing(&sent, limit);
    let grown = grown_over_an_indexing(&sent, limit);
    eprintln!(
        "a pack that does not deflate: the first indexing of this process grew the peak by \
         {first} KiB, the second by {grown} KiB"
    );
    if alone() {
        assert!(
            grown <= GROWTH_BOUND * limit / 1024,
            "peak resident memory grew by {grown} KiB, over {GROWTH_BOUND} times the byte limit \
             {limit}"
        );
    } else {
        eprintln!("a pack that does not deflate: the peak is not asserted beside other cases");
    }
}

/// The memory of the indexer on a pack within every bound that does not deflate, under a
/// byte limit of 16 MiB ([`memory_of_a_pack_that_does_not_deflate`]).
#[test]
fn an_indexer_keeps_its_memory_under_four_times_its_byte_limit_on_a_pack_that_does_not_deflate() {
    memory_of_a_pack_that_does_not_deflate(16 * MEBIBYTE);
}

/// The same at half the byte limit, 8 MiB: the memory follows the limit.
#[test]
fn an_indexer_keeps_its_memory_under_four_times_8_mib_on_a_pack_that_does_not_deflate() {
    memory_of_a_pack_that_does_not_deflate(8 * MEBIBYTE);
}
