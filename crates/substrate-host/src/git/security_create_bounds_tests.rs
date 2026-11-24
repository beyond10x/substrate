//! Cases a security review adds to the bounds a Git create holds on its work and its memory
//! (design 21, § *The baseline store*, *The work and the memory of a create follow its byte
//! limit*): each bound of the fetch at the byte where it turns, on one thread and on two; the
//! table of entries before its first entry, full, and with the lists beside its items; and
//! what the fetch inflates while its pack arrives.
//!
//! Each case runs the production fetch against the real TLS peer, or indexes a pack as that
//! fetch does, with a pack written by hand.

use std::io::Write as _;
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

use substrate_wire::{StorageLimit, StorageUsage};
use tempfile::tempdir;

use super::{
    AUTHORITY, FILE, MEBIBYTE, Pack, Server, assert_fetch_refused, baselines, driver, entry_header,
    fetch, fetched_packs, first_version, hold_peak, input, instead, length, names, peak_kib,
    refused, repository, staging_paths, stated_size, version,
};
use crate::git_baselines::DEFAULT_BYTE_LIMIT;
use crate::{Driver as _, DriverError};

/// The byte limit of the cases that turn a bound of the id pass: 1 MiB, the smallest storage
/// limit a create may request.
const LIMIT: StorageLimit = StorageLimit {
    max_bytes: MEBIBYTE,
    max_inodes: 2000,
};

/// The byte limit of the cases that fill the table of entries: 8 MiB, which is 65,536 entries.
const TABLE_LIMIT: StorageLimit = StorageLimit {
    max_bytes: 8 * MEBIBYTE,
    max_inodes: 2000,
};

/// The bytes the table of entries counts for one entry: two items of 64 bytes.
const ITEM_BYTES: u64 = 128;

/// The type a pack gives an entry stored behind an object it names by its id.
const BEHIND_AN_ID: u8 = 7;

/// The instructions that make, from an object of `base` bytes, another of `made` bytes: all
/// of the first but its last eight bytes, then `number` and as many more bytes as `made` asks
/// for.
fn remade(base: u64, made: u64, number: u64) -> Vec<u8> {
    let taken = base - 8;
    let added = made - taken;
    assert!((8..=127).contains(&added), "one instruction adds the bytes");
    assert!(
        taken > 0 && taken < 1 << 24 && taken != 1 << 16,
        "one instruction takes the bytes"
    );
    let mut instructions = [stated_size(base), stated_size(made)].concat();
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
    instructions.push(u8::try_from(added).expect("one instruction"));
    instructions.extend(number.to_be_bytes());
    instructions.extend(std::iter::repeat_n(
        0x5a,
        usize::try_from(added - 8).expect("added bytes"),
    ));
    instructions
}

/// A pack of three entries in a line: an object of `first` bytes, behind it one of `second`
/// bytes, and behind that one of eight. When the second is made both wait: the first for the
/// second, the second for the third. Nothing else of the pack waits.
fn waiting(first: u64, second: u64) -> Vec<u8> {
    let mut pack = Pack::new();
    let top = pack.whole(FILE, &first_version(first));
    let middle = pack.behind(top, &remade(first, second, 1));
    pack.behind(middle, &instead(second, b"the last"));
    assert_eq!(pack.entries, 3);
    pack.sent()
}

/// A pack for which the id pass inflates and makes exactly 64 times the byte limit of
/// [`LIMIT`], and `over` bytes more: an object of 256 KiB, 254 versions of it in a line, and
/// one object of the bytes that are left to the bound, then one of `over` bytes. Two versions
/// wait at once, half the byte limit, and no object is larger than the byte limit.
fn made(over: u64) -> Vec<u8> {
    const OBJECT: u64 = 256 * 1024;
    const VERSIONS: u64 = 254;
    let mut pack = Pack::new();
    let mut counted = OBJECT;
    let mut last = pack.whole(FILE, &first_version(OBJECT));
    for number in 1..=VERSIONS {
        let instructions = version(OBJECT, number);
        // An entry stored behind another counts what it inflates to and what it makes.
        counted += length(&instructions) + OBJECT;
        last = pack.behind(last, &instructions);
    }
    let left = 64 * LIMIT.max_bytes - counted;
    assert!(left > 0 && left <= LIMIT.max_bytes, "{left}");
    pack.whole(FILE, &vec![0x33; usize::try_from(left).expect("left")]);
    if over > 0 {
        pack.whole(FILE, &vec![0x44; usize::try_from(over).expect("over")]);
    }
    pack.sent()
}

/// The packs and the indexes in `directory`.
fn kept(directory: &Path) -> Vec<String> {
    names(directory)
        .into_iter()
        .filter(|name| {
            Path::new(name)
                .extension()
                .is_some_and(|extension| extension == "pack" || extension == "idx")
        })
        .collect()
}

/// How indexing `pack` ends on `threads` threads under the byte limit `limit`, as the fetch
/// of a create indexes it: done, or whether the create names its error
/// `workspace.git-storage-limit`. Beside it the packs and indexes the directory then holds.
fn indexed(pack: &[u8], threads: usize, limit: u64) -> (Result<(), bool>, Vec<String>) {
    let directory = tempdir().expect("pack directory");
    let mut sent = pack;
    let outcome = gix::odb::pack::Bundle::write_to_directory(
        &mut sent,
        Some(directory.path()),
        &mut gix::progress::Discard,
        &AtomicBool::new(false),
        None::<gix::OdbHandle>,
        gix::hash::Kind::Sha1,
        gix::odb::pack::bundle::write::Options {
            thread_limit: Some(threads),
            alloc_limit_bytes: Some(usize::try_from(limit).expect("limit")),
            ..Default::default()
        },
    )
    .map(|_| ())
    .map_err(|error| {
        crate::git::over_the_allocation_limit(&gix::remote::fetch::Error::Fetch(
            gix::protocol::fetch::Error::ConsumePack(Box::new(error)),
        ))
    });
    (outcome, kept(directory.path()))
}

/// The fetch step indexed its pack and ended afterwards, for a reason other than a bound: the
/// staging tree at `target` holds the pack and its index. No pack of these cases holds the
/// commit of the create, so the step never ends served.
fn assert_indexed(outcome: Result<StorageUsage, DriverError>, target: &Path) {
    let error = outcome.expect_err("a pack without the commit of the create");
    eprintln!(
        "after its pack was indexed the fetch step ended {}",
        error.code
    );
    assert_ne!(error.code, "workspace.git-storage-limit");
    assert_eq!(
        fetched_packs(target).len(),
        2,
        "the pack and its index: {:?}",
        fetched_packs(target)
    );
}

/// Beneath `root` there is no workspace, no commit-id file, no store and no staging tree.
fn assert_absent(root: &Path) {
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

/// A zlib stream that holds `bytes` as they are, in one block.
fn stored(bytes: &[u8]) -> Vec<u8> {
    let size = u16::try_from(bytes.len()).expect("one block");
    let mut stream = vec![0x78, 0x01, 0x01];
    stream.extend(size.to_le_bytes());
    stream.extend((!size).to_le_bytes());
    stream.extend(bytes);
    let (mut low, mut high) = (1_u32, 0_u32);
    for byte in bytes {
        low = (low + u32::from(*byte)) % 65_521;
        high = (high + low) % 65_521;
    }
    stream.extend(((high << 16) | low).to_be_bytes());
    stream
}

/// A pack of `entries` entries; `entry` writes each by its number.
fn pack_of(entries: u64, entry: impl Fn(u64, &mut Vec<u8>)) -> Vec<u8> {
    let mut pack = Pack::new();
    for number in 0..entries {
        entry(number, &mut pack.bytes);
        pack.entries += 1;
    }
    pack.sent()
}

/// An entry the pack stores as it is: a file object of the eight bytes of `number`.
fn whole_entry(number: u64, bytes: &mut Vec<u8>) {
    bytes.extend(entry_header(FILE, 8));
    bytes.extend(stored(&number.to_be_bytes()));
}

/// Objects that wait, at the byte where the bound turns. The fetch holds no more bytes of
/// objects that other entries wait for than the byte limit of the create: a pack that makes
/// exactly the byte limit wait is indexed, and a pack that makes one byte more wait is
/// refused `workspace.git-storage-limit` with no pack, no branch and nothing checked out in
/// the staging tree.
///
/// The byte limit is 1 MiB. Each pack is three entries in a line, so its objects wait in one
/// order whichever thread makes them: the first and the second together are the limit, then
/// one byte more.
#[test]
fn a_fetch_that_makes_its_byte_limit_wait_is_indexed_and_one_byte_more_is_refused() {
    const HALF: u64 = MEBIBYTE / 2;

    let (outcome, target) = fetch(waiting(HALF, HALF), LIMIT);
    assert_indexed(outcome, target.path());

    let (outcome, target) = fetch(waiting(HALF, HALF + 1), LIMIT);
    assert_fetch_refused(outcome, target.path());
}

/// What the fetch makes, at the byte where the bound turns. When it computes the ids the
/// fetch inflates and makes no more than 64 times the byte limit of the create: a pack for
/// which that is exactly 64 times the limit is indexed, and a pack for which it is one byte
/// more is refused `workspace.git-storage-limit` with no pack, no branch and nothing checked
/// out in the staging tree.
#[test]
fn a_fetch_that_makes_64_times_its_byte_limit_is_indexed_and_one_byte_more_is_refused() {
    let (outcome, target) = fetch(made(0), LIMIT);
    assert_indexed(outcome, target.path());

    let (outcome, target) = fetch(made(1), LIMIT);
    assert_fetch_refused(outcome, target.path());
}

/// Both bounds of the id pass turn at the same byte on one thread and on two, and a pack
/// that is refused leaves no pack and no index where the fetch writes them.
#[test]
fn the_bounds_of_the_id_pass_turn_at_the_same_byte_on_one_thread_and_on_two() {
    const HALF: u64 = MEBIBYTE / 2;
    let packs = [
        ("the byte limit waits", waiting(HALF, HALF), Ok(())),
        ("one byte more waits", waiting(HALF, HALF + 1), Err(true)),
        ("64 times the byte limit is made", made(0), Ok(())),
        ("one byte more is made", made(1), Err(true)),
    ];
    for threads in [1, 2] {
        for (what, pack, expected) in &packs {
            let started = Instant::now();
            let (outcome, kept) = indexed(pack, threads, LIMIT.max_bytes);
            eprintln!(
                "{what}, {threads} thread(s): {outcome:?} in {} ms",
                started.elapsed().as_millis()
            );
            assert_eq!(outcome, *expected, "{what}, {threads} thread(s)");
            assert_eq!(
                kept.len(),
                if expected.is_ok() { 2 } else { 0 },
                "{what}, {threads} thread(s): {kept:?}"
            );
        }
    }
}

/// The table of entries before its first entry. A pack that states exactly as many entries as
/// the byte limit holds is not over the bound, and the table is still reserved for no more
/// than 8,192 items before an entry arrives, whatever number the pack states.
///
/// A create that requests no limit has 2 GiB, which is 16,777,216 entries. The pack here is the
/// twelve bytes that state that many entries and bring none, so the create ends for the pack
/// that is cut short, `workspace.git-fetch-failed`, and not for a bound; its workspace is
/// proved absent. The address space of the process grows by less than 768 MiB over the create:
/// a table reserved for the entries the pack states would take 1 GiB and more of it.
#[tokio::test]
async fn a_create_whose_pack_states_as_many_entries_as_its_byte_limit_holds_reserves_no_table() {
    const ADDRESS_SPACE_BOUND_KIB: u64 = 768 * 1024;
    let stated = u32::try_from(DEFAULT_BYTE_LIMIT / ITEM_BYTES).expect("entries");
    assert_eq!(stated, 16_777_216);
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

    let (dispatch, error) = refused(outcome);
    assert_eq!(error.code, "workspace.git-fetch-failed");
    assert_eq!(dispatch, "contained absent");
    assert_absent(root.path());
    hold_peak(
        "the table before its first entry",
        "growth of the peak address space",
        peak_kib("VmPeak") - before,
        ADDRESS_SPACE_BOUND_KIB,
    );
}

/// The table of entries, full. A pack that brings exactly as many entries as the byte limit
/// holds is indexed, and a pack that states one more is refused
/// `workspace.git-storage-limit` with no pack, no branch and nothing checked out in the
/// staging tree.
///
/// The byte limit is 8 MiB, which is 65,536 entries; each entry is a file object of eight
/// bytes. Over the first fetch the most memory resident at once grows by less than four times
/// the byte limit.
#[test]
fn a_fetch_whose_pack_brings_as_many_entries_as_its_byte_limit_holds_is_indexed() {
    let entries = TABLE_LIMIT.max_bytes / ITEM_BYTES;
    assert_eq!(entries, 65_536);
    let full = pack_of(entries, whole_entry);
    let over = pack_of(entries + 1, whole_entry);
    assert!(length(&over) <= TABLE_LIMIT.max_bytes);
    let before = peak_kib("VmHWM");
    let started = Instant::now();

    let (outcome, target) = fetch(full, TABLE_LIMIT);

    let grown = peak_kib("VmHWM").saturating_sub(before);
    eprintln!(
        "{entries} whole entries: {} ms",
        started.elapsed().as_millis()
    );
    assert_indexed(outcome, target.path());
    hold_peak(
        "a full table of whole entries",
        "growth of the peak resident memory",
        grown,
        4 * TABLE_LIMIT.max_bytes / 1024,
    );

    let (outcome, target) = fetch(over, TABLE_LIMIT);
    assert_fetch_refused(outcome, target.path());
}

/// The table of entries and the lists beside its items. An entry stored behind an object it
/// names by its id is kept in a list of that id until the object is made, beside its item.
/// With as many such entries as the byte limit holds, each naming another id, the table and
/// its lists together stay within a fixed multiple of the byte limit: over the fetch the
/// most memory resident at once grows by less than six times the byte limit.
///
/// The byte limit is 8 MiB, which is 65,536 entries. The lists are counted in the table beside
/// the items, so a pack of that many entries that each name another id is over the bound of
/// the table: the fetch step ends `workspace.git-storage-limit` and keeps no pack. The pack
/// that fills the table with such entries, and ends `workspace.git-fetch-failed` because no
/// entry has the object it names, is in `create_bounds_tests`
/// (`a_fetch_counts_the_id_an_entry_names_and_its_list_in_its_table`).
#[test]
fn the_table_and_its_lists_of_entries_that_name_an_id_stay_within_six_times_the_byte_limit() {
    let entries = TABLE_LIMIT.max_bytes / ITEM_BYTES;
    let instructions = stored(&[1, 1, 1, b'x']);
    let pack = pack_of(entries, |number, bytes| {
        bytes.extend(entry_header(BEHIND_AN_ID, 4));
        bytes.extend(number.to_be_bytes());
        bytes.extend([0xab; 12]);
        bytes.extend(&instructions);
    });
    assert!(length(&pack) <= TABLE_LIMIT.max_bytes);
    let before = peak_kib("VmHWM");
    let started = Instant::now();

    let (outcome, target) = fetch(pack, TABLE_LIMIT);

    let grown = peak_kib("VmHWM").saturating_sub(before);
    eprintln!(
        "{entries} entries that name an id: {} ms",
        started.elapsed().as_millis()
    );
    let error = outcome.expect_err("a pack over the bound of the table");
    assert_eq!(error.code, "workspace.git-storage-limit");
    assert_eq!(fetched_packs(target.path()), Vec::<String>::new());
    hold_peak(
        "a full table of entries that name an id",
        "growth of the peak resident memory",
        grown,
        6 * TABLE_LIMIT.max_bytes / 1024,
    );
}

/// What arrives. While a pack arrives the fetch inflates each entry to find where it ends,
/// before it computes any id. Those bytes follow the byte limit of the create as the bytes of
/// the id pass do: a pack whose entries inflate to more than 64 times the byte limit is
/// refused `workspace.git-storage-limit` before the fetch has inflated all of them, with no
/// pack, no branch and nothing checked out in the staging tree.
///
/// The byte limit is 1 MiB. The pack is 256 file objects of 1 MiB each, 256 times the byte
/// limit, in less than half the byte limit of pack bytes, and it does not end as a pack ends:
/// a fetch that reads to the end of it has inflated every entry while the pack arrived.
#[test]
fn what_a_fetch_inflates_while_its_pack_arrives_is_within_64_times_its_byte_limit() {
    const OBJECTS: u64 = 256;
    let object = {
        let mut stream =
            gix::zlib::stream::deflate::Write::new(Vec::new(), gix::zlib::Compression::BEST);
        stream
            .write_all(&vec![0; usize::try_from(MEBIBYTE).expect("size")])
            .expect("deflated bytes");
        stream.flush().expect("the end of the stream");
        stream.into_inner()
    };
    let mut sent = pack_of(OBJECTS, |_, bytes| {
        bytes.extend(entry_header(FILE, MEBIBYTE));
        bytes.extend(&object);
    });
    assert!(length(&sent) <= LIMIT.max_bytes / 2, "{}", sent.len());
    *sent.last_mut().expect("a checksum") ^= 0xff;
    let started = Instant::now();

    let (outcome, target) = fetch(sent, LIMIT);

    eprintln!(
        "{OBJECTS} entries of 1 MiB under a byte limit of 1 MiB: the fetch step ended {:?} after {} ms",
        outcome.as_ref().map_err(|error| error.code),
        started.elapsed().as_millis()
    );
    assert_fetch_refused(outcome, target.path());
}
