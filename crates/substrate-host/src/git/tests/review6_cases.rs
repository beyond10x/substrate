//! Cases of the sixth review of the baseline store (design 21, § *The baseline store*, *Stored
//! chains*): the pass that reads the chains of a pack as a reader of bytes it did not write,
//! whether it and the Git library find the same entry by an id, the bound on the chains of a
//! baseline at its edges, and what the indexing of a fetch admits and refuses.

use std::path::Path;

use git2::{ObjectType, Oid, Repository};
use substrate_wire::StorageLimit;
use tempfile::tempdir;

use super::super::over_the_allocation_limit;
use super::super::store::chains::{Chains, MAX_CHAIN};
use super::super::store::{self, Unadmitted};
use super::review5_cases::chained;
use super::store_failure_cases::repository_holding;
use crate::git_baselines::{DEFAULT_BYTE_LIMIT, DEFAULT_LIMIT};

/// The type a pack gives a tree.
const TREE: u8 = 2;

/// The type a pack gives a file object.
const FILE: u8 = 3;

/// A delta that makes an object of no bytes from one of no bytes.
const NOTHING_FROM_NOTHING: [u8; 2] = [0, 0];

/// A zlib stream that holds `bytes` as they are, in stored blocks.
fn stored(bytes: &[u8]) -> Vec<u8> {
    let mut stream = vec![0x78, 0x01];
    let mut blocks = bytes.chunks(usize::from(u16::MAX)).peekable();
    if blocks.peek().is_none() {
        stream.extend([0x01, 0x00, 0x00, 0xff, 0xff]);
    }
    while let Some(block) = blocks.next() {
        let length = u16::try_from(block.len()).expect("one stored block");
        stream.push(u8::from(blocks.peek().is_none()));
        stream.extend(length.to_le_bytes());
        stream.extend((!length).to_le_bytes());
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

/// The seven low bits of `value`.
fn seven_bits(value: u64) -> u8 {
    u8::try_from(value & 0x7f).expect("seven bits")
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

/// A size as a delta states it.
fn delta_size(mut size: u64) -> Vec<u8> {
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

/// The id a hand-written pack gives the entry `number`.
fn id(number: u8) -> [u8; 20] {
    [number; 20]
}

/// The id of [`id`] as the Git library takes it.
fn oid(number: u8) -> Oid {
    Oid::from_bytes(&id(number)).expect("id")
}

/// The length of `bytes` as a pack states an offset.
fn length(bytes: &[u8]) -> u64 {
    u64::try_from(bytes.len()).expect("length")
}

/// A pack written by hand, and the rows its index has: the id and the offset of each entry.
struct Pack {
    bytes: Vec<u8>,
    rows: Vec<([u8; 20], u64)>,
}

impl Pack {
    fn new() -> Self {
        let mut bytes = b"PACK".to_vec();
        bytes.extend(2_u32.to_be_bytes());
        bytes.extend(0_u32.to_be_bytes());
        Self {
            bytes,
            rows: Vec::new(),
        }
    }

    /// Adds an entry the pack stores as it is, and answers where it begins.
    fn whole(&mut self, id: [u8; 20], kind: u8, content: &[u8]) -> u64 {
        let offset = length(&self.bytes);
        self.bytes.extend(entry_header(kind, length(content)));
        self.bytes.extend(stored(content));
        self.rows.push((id, offset));
        offset
    }

    /// Adds an entry stored behind the entry that begins at `base`, named by the bytes
    /// between them, and answers where it begins.
    fn behind_offset(&mut self, id: [u8; 20], base: u64, delta: &[u8]) -> u64 {
        let offset = length(&self.bytes);
        self.bytes.extend(entry_header(6, length(delta)));
        self.bytes.extend(distance(offset - base));
        self.bytes.extend(stored(delta));
        self.rows.push((id, offset));
        offset
    }

    /// Adds an entry stored behind the object `base`, named by its id.
    fn behind_id(&mut self, id: [u8; 20], base: [u8; 20], delta: &[u8]) {
        let offset = length(&self.bytes);
        self.bytes.extend(entry_header(7, length(delta)));
        self.bytes.extend(base);
        self.bytes.extend(stored(delta));
        self.rows.push((id, offset));
    }

    /// The bytes of the pack, with the number of its entries and a checksum of zeros, and the
    /// rows of its index.
    fn finish(mut self) -> (Vec<u8>, Vec<([u8; 20], u64)>) {
        let count = u32::try_from(self.rows.len()).expect("count");
        self.bytes[8..12].copy_from_slice(&count.to_be_bytes());
        self.bytes.extend([0; 20]);
        (self.bytes, self.rows)
    }

    /// The bytes of the pack as a server sends it: with the number of its entries and its
    /// checksum.
    fn sent(self) -> Vec<u8> {
        let (mut bytes, _) = self.finish();
        bytes.truncate(bytes.len() - 20);
        let mut hasher = gix::hash::hasher(gix::hash::Kind::Sha1);
        hasher.update(&bytes);
        let checksum = hasher.try_finalize().expect("pack checksum");
        bytes.extend(checksum.as_slice());
        bytes
    }
}

/// The fan-out table of an index with `rows`, sorted: for each first byte, how many ids begin
/// with it or a lower one.
fn fan_of(rows: &[([u8; 20], u64)]) -> Vec<u32> {
    (0..=u8::MAX)
        .map(|first| {
            let upto = rows.iter().filter(|(id, _)| id[0] <= first).count();
            u32::try_from(upto).expect("count")
        })
        .collect()
}

/// An index of version 2 with `rows` and the fan-out table `fan`, or the one that follows
/// from the rows. With `wide`, every offset is in the table of 64-bit offsets.
fn index_v2(rows: &[([u8; 20], u64)], fan: Option<&[u32]>, wide: bool) -> Vec<u8> {
    let mut rows = rows.to_vec();
    rows.sort_unstable();
    let mut index = b"\xfftOc".to_vec();
    index.extend(2_u32.to_be_bytes());
    for count in fan.map_or_else(|| fan_of(&rows), <[u32]>::to_vec) {
        index.extend(count.to_be_bytes());
    }
    for (id, _) in &rows {
        index.extend(id);
    }
    index.extend(vec![0; 4 * rows.len()]);
    let mut large = Vec::new();
    for (_, offset) in &rows {
        match u32::try_from(*offset) {
            Ok(small) if small < 0x8000_0000 && !wide => index.extend(small.to_be_bytes()),
            _ => {
                let place = u32::try_from(large.len()).expect("place");
                index.extend((0x8000_0000 | place).to_be_bytes());
                large.push(*offset);
            }
        }
    }
    for offset in large {
        index.extend(offset.to_be_bytes());
    }
    index.extend([0; 40]);
    index
}

/// An index of version 1 with `rows`.
fn index_v1(rows: &[([u8; 20], u64)]) -> Vec<u8> {
    let mut rows = rows.to_vec();
    rows.sort_unstable();
    let mut index = Vec::new();
    for count in fan_of(&rows) {
        index.extend(count.to_be_bytes());
    }
    for (id, offset) in &rows {
        index.extend(u32::try_from(*offset).expect("offset").to_be_bytes());
        index.extend(id);
    }
    index.extend([0; 40]);
    index
}

/// Writes the pack `name` and its index into the object store at `objects`.
fn put(objects: &Path, name: &str, pack: &[u8], index: &[u8]) {
    let directory = objects.join("pack");
    std::fs::create_dir_all(&directory).expect("pack directory");
    std::fs::write(directory.join(format!("pack-{name}.pack")), pack).expect("pack");
    std::fs::write(directory.join(format!("pack-{name}.idx")), index).expect("index");
}

/// What the pass finds for the entries `numbers` in the object store at `objects`.
fn behind<const N: usize>(objects: &Path, numbers: [u8; N]) -> Result<[u64; N], Unadmitted> {
    Chains::read(objects).map(|chains| numbers.map(|number| chains.behind(oid(number))))
}

/// A pack of three entries: one as it is, one behind it by the bytes between them, and one
/// behind that by its id.
fn three_entries() -> (Vec<u8>, Vec<([u8; 20], u64)>) {
    let mut pack = Pack::new();
    let first = pack.whole(id(0x10), FILE, b"");
    pack.behind_offset(id(0x20), first, &NOTHING_FROM_NOTHING);
    pack.behind_id(id(0x30), id(0x20), &NOTHING_FROM_NOTHING);
    pack.finish()
}

/// The pass reads an index of version 1, one of version 2, and one whose offsets are all in
/// the table of 64-bit offsets, and finds the same chains in each. Two rows of an index that
/// name the same entry have that entry's chain.
#[test]
fn the_pass_reads_both_index_versions_wide_offsets_and_two_rows_of_one_entry() {
    let (pack, rows) = three_entries();
    for (form, index) in [
        ("version 2", index_v2(&rows, None, false)),
        ("version 2, wide offsets", index_v2(&rows, None, true)),
        ("version 1", index_v1(&rows)),
    ] {
        let objects = tempdir().expect("object store");
        put(objects.path(), "a", &pack, &index);
        assert_eq!(
            behind(objects.path(), [0x10, 0x20, 0x30]),
            Ok([0, 1, 2]),
            "{form}"
        );
    }

    let mut twice = rows.clone();
    twice.push((id(0x40), rows[2].1));
    let objects = tempdir().expect("object store");
    put(objects.path(), "a", &pack, &index_v2(&twice, None, false));
    assert_eq!(
        behind(objects.path(), [0x10, 0x20, 0x30, 0x40]),
        Ok([0, 1, 2, 2])
    );
}

/// An index row that names an offset where no entry of the pack can begin is refused as an
/// object that cannot be read, whatever the offset: past the end, the end, the checksum, the
/// pack's own header, or bytes inside another entry. So is every pack cut short before its
/// last entry begins. None of them makes the pass read outside the file.
#[test]
fn the_pass_refuses_an_offset_outside_the_entries_and_a_pack_cut_short() {
    let (pack, rows) = three_entries();
    let end = length(&pack);
    for (what, offset) in [
        ("past the end, the largest offset", u64::MAX),
        ("past the end by one byte", end + 1),
        ("the end", end),
        ("the checksum", end - 20),
        ("the pack's header", 0),
        ("the second byte of the first entry", rows[0].1 + 1),
        ("the third byte of the first entry", rows[0].1 + 2),
    ] {
        let mut rows = rows.clone();
        rows.push((id(0x40), offset));
        let objects = tempdir().expect("object store");
        put(objects.path(), "a", &pack, &index_v2(&rows, None, false));
        assert_eq!(
            behind(objects.path(), [0x40]),
            Err(Unadmitted::Object),
            "{what}"
        );
    }

    let last = usize::try_from(rows[2].1).expect("offset");
    let index = index_v2(&rows, None, false);
    for cut in 0..=last {
        let objects = tempdir().expect("object store");
        put(objects.path(), "a", &pack[..cut], &index);
        assert_eq!(
            behind(objects.path(), [0x30]),
            Err(Unadmitted::Object),
            "a pack of {cut} bytes"
        );
    }
}

/// An object two packs hold counts with the longer of its chains, whichever pack the Git
/// library would read it from. An entry stored behind an object only another pack holds is
/// refused: the Git library looks for it in the entry's own pack alone.
#[test]
fn an_object_two_packs_hold_counts_with_the_longer_of_its_chains() {
    let whole = || {
        let mut pack = Pack::new();
        pack.whole(id(0x10), FILE, b"");
        pack.finish()
    };
    let behind_two = || {
        let mut pack = Pack::new();
        let first = pack.whole(id(0x20), FILE, b"");
        let second = pack.behind_offset(id(0x30), first, &NOTHING_FROM_NOTHING);
        pack.behind_offset(id(0x10), second, &NOTHING_FROM_NOTHING);
        pack.finish()
    };
    for names in [["a", "b"], ["b", "a"]] {
        let objects = tempdir().expect("object store");
        for (name, (pack, rows)) in names.into_iter().zip([whole(), behind_two()]) {
            put(objects.path(), name, &pack, &index_v2(&rows, None, false));
        }
        assert_eq!(behind(objects.path(), [0x10]), Ok([2]), "{names:?}");
    }

    let mut across = Pack::new();
    across.behind_id(id(0x40), id(0x10), &NOTHING_FROM_NOTHING);
    let (across, across_rows) = across.finish();
    let (pack, rows) = whole();
    let objects = tempdir().expect("object store");
    put(objects.path(), "a", &pack, &index_v2(&rows, None, false));
    put(
        objects.path(),
        "b",
        &across,
        &index_v2(&across_rows, None, false),
    );
    assert_eq!(behind(objects.path(), [0x40]), Err(Unadmitted::Object));
}

/// The next value of a fixed sequence (xorshift), so that the case below changes the same
/// bytes on every run.
fn next(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state
}

/// A place in `bytes`, from the fixed sequence; none in no bytes.
fn place(state: &mut u64, bytes: &[u8]) -> Option<usize> {
    let within = next(state) % u64::try_from(bytes.len().max(1)).expect("length");
    (!bytes.is_empty()).then(|| usize::try_from(within).expect("place"))
}

/// The pass ends, and without a panic, for a pack and an index with bytes changed or cut off:
/// 1,000 rounds from a fixed seed, each changing up to three bytes of a pack of six entries or
/// of its index (version 2, version 2 with wide offsets, version 1 in turn), or cutting one of
/// them short. Each round ends in a refusal or in chains within the bound of one object.
#[test]
fn the_pass_ends_without_a_panic_for_a_pack_and_an_index_with_bytes_changed() {
    let mut pack = Pack::new();
    let first = pack.whole(id(0x10), FILE, b"");
    let second = pack.behind_offset(id(0x20), first, &NOTHING_FROM_NOTHING);
    pack.behind_id(id(0x30), id(0x20), &NOTHING_FROM_NOTHING);
    pack.behind_id(id(0x05), id(0x90), &NOTHING_FROM_NOTHING);
    pack.whole(id(0x90), FILE, b"");
    pack.behind_offset(id(0xf0), second, &NOTHING_FROM_NOTHING);
    let (pack, rows) = pack.finish();
    let forms = [
        index_v2(&rows, None, false),
        index_v2(&rows, None, true),
        index_v1(&rows),
    ];
    let objects = tempdir().expect("object store");
    let mut state = 0x9e37_79b9_7f4a_7c15_u64;
    let (mut read, mut refused) = (0, 0);
    for round in 0..1000 {
        let mut pack = pack.clone();
        let mut index = forms[round % forms.len()].clone();
        for _ in 0..=next(&mut state) % 3 {
            let changed = if next(&mut state).is_multiple_of(2) {
                &mut pack
            } else {
                &mut index
            };
            let Some(at) = place(&mut state, changed) else {
                continue;
            };
            match next(&mut state) % 8 {
                0 => changed.truncate(at),
                // The tables behind the fan-out table are the last bytes of an index.
                1..=3 => {
                    let from_end = at % 160;
                    let at = changed.len() - 1 - from_end.min(changed.len() - 1);
                    changed[at] = next(&mut state).to_le_bytes()[0];
                }
                _ => changed[at] = next(&mut state).to_le_bytes()[0],
            }
        }
        put(objects.path(), "a", &pack, &index);
        let directory = objects.path();
        let outcome = std::panic::catch_unwind(|| {
            Chains::read(directory).map(|chains| {
                (0..=u8::MAX)
                    .map(|number| chains.behind(oid(number)))
                    .max()
                    .unwrap_or(0)
            })
        });
        assert!(outcome.is_ok(), "round {round}: the pass panicked");
        if let Ok(Ok(most)) = outcome {
            assert!(most <= u64::from(MAX_CHAIN), "round {round}: {most}");
            read += 1;
        } else {
            refused += 1;
        }
    }
    assert!(read > 20 && refused > 20, "{read} read, {refused} refused");
}

/// Design 21, § *Stored chains*: the pass counts what the Git library walks to read an
/// object's header. Both find an entry by its id in the pack's index. The index here holds
/// four ids in ascending order, and its fan-out table says that no id begins with the first
/// byte of the fourth and that one id begins with the byte after it. The Git library finds
/// the fourth id, which the pack stores behind three entries, and the pass must count those
/// three for it or refuse the index. The index is written here: a fetch writes its own, with
/// a fan-out table that follows from its ids.
#[test]
fn the_pass_counts_the_chain_of_an_id_the_git_library_finds_in_the_index() {
    let mut pack = Pack::new();
    let mut last = pack.whole(id(0x10), FILE, b"");
    for number in [0x20, 0x30, 0x40] {
        last = pack.behind_offset(id(number), last, &NOTHING_FROM_NOTHING);
    }
    let (pack, rows) = pack.finish();
    let mut fan = fan_of(&rows);
    assert_eq!(fan[0x40], 4);
    fan[0x40] = 3;
    let objects = tempdir().expect("object store");
    put(
        objects.path(),
        "a",
        &pack,
        &index_v2(&rows, Some(&fan), false),
    );

    let library = git2::Odb::new().expect("object database");
    library
        .add_disk_alternate(objects.path().to_str().expect("path"))
        .expect("object store");
    assert_eq!(
        library.read_header(oid(0x40)).ok(),
        Some((0, ObjectType::Blob)),
        "the Git library does not find the id, so the case shows nothing"
    );

    let counted = behind(objects.path(), [0x40]);
    assert!(
        matches!(counted, Err(_) | Ok([3])),
        "the Git library walked three entries for a header the pass counted as {counted:?}"
    );
}

/// The bound on the chains of a baseline at its edges. Under an inode limit of one path, one
/// file stored behind exactly 64 entries is within it and one behind 65 is refused. Under a
/// limit of no path nothing is admitted, and under the largest limit a number can state the
/// bound of one object still holds and nothing overflows.
#[test]
fn the_bound_on_the_chains_of_a_baseline_holds_at_one_path_at_none_and_at_the_largest_limit() {
    let checked = |links: u64, max_inodes: u64| {
        let (workspace, commit) = repository_holding(|repository| chained(repository, links, 1));
        let repository = Repository::open(workspace.path()).expect("repository");
        let limit = StorageLimit {
            max_bytes: DEFAULT_BYTE_LIMIT,
            max_inodes,
        };
        store::check_baseline(&repository, commit, limit).map(|checked| checked.chain_steps)
    };

    assert_eq!(checked(63, 1), Ok(64));
    assert_eq!(checked(64, 1), Err(Unadmitted::Limit));
    assert_eq!(checked(0, 0), Err(Unadmitted::Limit));
    assert_eq!(checked(4094, u64::MAX), Ok(4095));
    assert_eq!(checked(4095, u64::MAX), Err(Unadmitted::Limit));
}

/// The bytes each entry of [`large_entries`] states: more than the Git library keeps of an
/// entry it has applied, so that it applies each of them again for every read.
const LARGE: u64 = 0x11_0008;

/// The id of the entry `number` of [`large_entries`]: no object's.
fn large_id(number: u32) -> [u8; 20] {
    let mut id = [0xee; 20];
    id[16..].copy_from_slice(&number.to_be_bytes());
    id
}

/// A pack that holds one entry of [`LARGE`] bytes as it is and `entries - 1` more, each stored
/// behind the one before it and different from it in its last eight bytes; and each of
/// `objects`, of the type `kind`, stored behind the last of those: behind `entries` entries.
/// With 4,095 entries, the most the bound of one object admits, it is smaller than 1.3 MiB.
fn large_entries(kind: u8, entries: u32, objects: &[(Oid, Vec<u8>)]) -> Pack {
    let mut pack = Pack::new();
    let large = vec![0; usize::try_from(LARGE).expect("length")];
    let mut last = pack.whole(large_id(0), kind, &large);
    for number in 1..entries {
        let mut delta = delta_size(LARGE);
        delta.extend(delta_size(LARGE));
        // Take 0x11_0000 bytes from the start of the entry before, then the eight that follow.
        delta.extend([0xc0, 0x11, 8]);
        delta.extend(u64::from(number).to_be_bytes());
        last = pack.behind_offset(large_id(number), last, &delta);
    }
    for (id, content) in objects {
        let mut delta = delta_size(LARGE);
        delta.extend(delta_size(length(content)));
        delta.push(u8::try_from(content.len()).expect("one instruction"));
        delta.extend(content);
        let id: [u8; 20] = id.as_bytes().try_into().expect("id");
        pack.behind_offset(id, last, &delta);
    }
    pack
}

/// Indexes `pack` into the directory `packs` as the fetch of a create does: with
/// `Bundle::write_to_directory`, two threads, and `limit` as the most it allocates for one
/// object.
fn index_as_the_fetch(
    packs: &Path,
    pack: &[u8],
    limit: usize,
) -> Result<(), gix::odb::pack::bundle::write::Error> {
    let mut sent = pack;
    gix::odb::pack::Bundle::write_to_directory(
        &mut sent,
        Some(packs),
        &mut gix::progress::Discard,
        &std::sync::atomic::AtomicBool::new(false),
        None::<gix::OdbHandle>,
        gix::hash::Kind::Sha1,
        gix::odb::pack::bundle::write::Options {
            thread_limit: Some(2),
            alloc_limit_bytes: Some(limit),
            ..Default::default()
        },
    )
    .map(|_| ())
}

/// How many directories the baseline of the next cases holds.
const DIRECTORIES: usize = 2;

/// The root tree of a baseline with [`DIRECTORIES`] directories of one empty file each. The
/// tree of each directory is stored by `store`; the root tree is in the object store as it
/// is.
fn directories(repository: &Repository, store: impl FnOnce(&[(Oid, Vec<u8>)])) -> Oid {
    let file = repository.blob(b"").expect("file");
    let mut root = Vec::new();
    let mut trees = Vec::new();
    for number in 0..DIRECTORIES {
        let mut tree = format!("100644 f{number}\0").into_bytes();
        tree.extend(file.as_bytes());
        let id = Oid::hash_object(ObjectType::Tree, &tree).expect("tree id");
        root.extend(format!("40000 d{number}\0").into_bytes());
        root.extend(id.as_bytes());
        trees.push((id, tree));
    }
    store(&trees);
    let objects = repository.odb().expect("object store");
    objects.write(ObjectType::Tree, &root).expect("root tree")
}

/// The indexing of a fetch admits a pack that stores a tree behind entries of another kind of
/// bytes: it reads no object as what its type says. The pack here stores the tree of each
/// directory behind eight entries of 1.1 MiB of zeros that it calls trees, each stored behind
/// the one before it, so the indexing holds two of them at once; indexed as the fetch
/// indexes, under an allocation limit of 4 MiB, it is written with its index, the pass finds
/// eight entries before each tree, and the check admits the baseline.
#[test]
fn the_fetch_indexes_a_pack_that_stores_a_tree_behind_large_entries_and_the_check_admits_it() {
    let (workspace, commit) = repository_holding(|repository| {
        directories(repository, |trees| {
            let pack = large_entries(TREE, 8, trees).sent();
            index_as_the_fetch(
                &repository.path().join("objects/pack"),
                &pack,
                4 * 1024 * 1024,
            )
            .expect("the fetch indexes the pack");
        })
    });
    let repository = Repository::open(workspace.path()).expect("repository");

    let checked = store::check_baseline(&repository, commit, DEFAULT_LIMIT);

    assert_eq!(checked.map(|checked| checked.chain_steps), Ok(16));
}

/// A fetch can write a pack in which the index names an entry as stored behind itself. The
/// pack here holds one file as it is, and the same eight bytes again as an entry stored behind
/// the object of the file's id. The fetch indexes it: its index holds the id twice, and the
/// search for the id ends at the second entry, the one stored behind that id. The pass
/// refuses the pack as one whose objects cannot be read, and ends.
#[test]
fn the_pass_refuses_a_pack_the_fetch_indexes_whose_index_names_an_entry_behind_itself() {
    let content = b"baseline";
    let file = Oid::hash_object(ObjectType::Blob, content).expect("file id");
    let mut pack = Pack::new();
    pack.whole(id(0), FILE, content);
    // Eight bytes from the start of the object of that id.
    pack.behind_id(
        id(0),
        file.as_bytes().try_into().expect("id"),
        &[8, 8, 0x90, 8],
    );
    let objects = tempdir().expect("object store");
    let packs = objects.path().join("pack");
    std::fs::create_dir(&packs).expect("pack directory");

    index_as_the_fetch(&packs, &pack.sent(), 1024 * 1024).expect("the fetch indexes the pack");

    assert_eq!(
        Chains::read(objects.path()).map(|chains| chains.behind(file)),
        Err(Unadmitted::Object)
    );
}

/// How indexing `pack` ends under an allocation limit of 1 MiB, as the fetch indexes: done,
/// or whether the create names its error an object over the allocation limit.
fn indexed_under_one_mebibyte(pack: &[u8]) -> Result<(), bool> {
    let directory = tempdir().expect("pack directory");
    index_as_the_fetch(directory.path(), pack, 1024 * 1024).map_err(|error| {
        over_the_allocation_limit(&gix::remote::fetch::Error::Fetch(
            gix::protocol::fetch::Error::ConsumePack(Box::new(error)),
        ))
    })
}

/// A pack of one file of eight bytes and one entry stored behind it with the instructions
/// `delta`.
fn file_and_one_behind_it(delta: &[u8]) -> Vec<u8> {
    let mut pack = Pack::new();
    let file = pack.whole(id(0x10), FILE, b"baseline");
    pack.behind_offset(id(0x20), file, delta);
    pack.sent()
}

/// Design 21, § *Object sizes*: an object that states more than the byte limit ends the fetch
/// `workspace.git-storage-limit` also when the pack stores it behind another entry. The entry
/// here is stored behind a file of eight bytes and states that it makes 2 MiB of it; indexing
/// ends before those bytes are made, in the error the create names a storage limit. So does an
/// entry whose instructions alone are 2 MiB. The same entry making sixteen bytes is indexed,
/// and one that states a wrong size for the file it is stored behind ends in an error the
/// create does not name a limit.
#[test]
fn an_object_behind_another_entry_that_states_more_than_the_limit_ends_indexing_as_a_limit() {
    let over = [delta_size(8), delta_size(2 * 1024 * 1024)].concat();
    assert_eq!(
        indexed_under_one_mebibyte(&file_and_one_behind_it(&over)),
        Err(true)
    );

    let instructions = vec![0; 2 * 1024 * 1024];
    assert_eq!(
        indexed_under_one_mebibyte(&file_and_one_behind_it(&instructions)),
        Err(true)
    );

    // Take eight bytes from the start of the file, twice.
    let within = [8, 16, 0x90, 8, 0x90, 8];
    assert_eq!(
        indexed_under_one_mebibyte(&file_and_one_behind_it(&within)),
        Ok(())
    );

    let another_file = [9, 16, 0x90, 8, 0x90, 8];
    assert_eq!(
        indexed_under_one_mebibyte(&file_and_one_behind_it(&another_file)),
        Err(false)
    );
}
