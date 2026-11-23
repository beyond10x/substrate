//! Cases of the stored chains of a baseline (design 21, § *The baseline store*, *Stored
//! chains*): how many entries a fetched pack stores an object behind is read before the Git
//! library reads any header, and bounded for one object and for the objects of the baseline
//! together.

use std::path::Path;

use git2::{Oid, Repository};
use substrate_wire::StorageLimit;
use tempfile::tempdir;

use super::super::store::chains::{Chains, MAX_CHAIN};
use super::super::store::{self, Unadmitted};
use super::super::{finish_materialization, write_baseline};
use super::review5_cases::{chained, stored};
use super::store_failure_cases::repository_holding;
use crate::git_baselines::{DEFAULT_BYTE_LIMIT, DEFAULT_LIMIT};

const BRANCH: &str = "refs/heads/provider-default";

/// How both steps of a create that read the baseline end for the repository `chained` writes:
/// the checkout step and the write of the store. With them, how many entries the checkout
/// step left beside `.git`, and how many the write left in the baseline root.
fn create(links: u64, files: u64, limit: StorageLimit) -> ([&'static str; 2], usize, usize) {
    let (workspace, commit) = repository_holding(|repository| chained(repository, links, files));
    let repository = Repository::open(workspace.path()).expect("repository");
    let checkout = finish_materialization(&repository, BRANCH, commit, limit);
    let baselines = tempdir().expect("baseline root");
    let written = write_baseline(
        baselines.path(),
        "ws_git",
        workspace.path(),
        &commit.to_string(),
        limit,
    );
    let count = |directory: &Path| std::fs::read_dir(directory).expect("directory").count();
    (
        [checkout, written].map(|step| step.map_or_else(|error| error.code, |()| "served")),
        count(workspace.path()) - 1,
        count(baselines.path()),
    )
}

/// What the check finds in the repository `chained` writes.
fn checked(links: u64, files: u64, limit: StorageLimit) -> Result<store::Checked, Unadmitted> {
    let (workspace, commit) = repository_holding(|repository| chained(repository, links, files));
    let repository = Repository::open(workspace.path()).expect("repository");
    store::check_baseline(&repository, commit, limit)
}

/// One object is stored behind at most 4,095 entries of its pack, the deepest chain
/// `git pack-objects --depth` writes. A baseline whose one file is stored behind exactly that
/// many is served: checked out, and its store written. One stored behind 4,096 is refused
/// `workspace.git-storage-limit` by both steps, before anything is checked out or written.
#[test]
fn an_object_stored_behind_4095_entries_is_served_and_one_behind_4096_is_refused() {
    assert_eq!(MAX_CHAIN, 4095);

    assert_eq!(
        checked(4094, 1, DEFAULT_LIMIT).map(|checked| checked.chain_steps),
        Ok(4095)
    );
    assert_eq!(create(4094, 1, DEFAULT_LIMIT), (["served"; 2], 1, 2));

    assert_eq!(checked(4095, 1, DEFAULT_LIMIT), Err(Unadmitted::Limit));
    assert_eq!(
        create(4095, 1, DEFAULT_LIMIT),
        (["workspace.git-storage-limit"; 2], 0, 0)
    );
}

/// The objects of a baseline are stored behind at most 64 entries together for each path the
/// inode limit admits. Under a limit of 100 paths, 64 files that are each stored behind 100
/// entries, 6,400 together, are served. With one file more, 6,500 together, the baseline is
/// refused `workspace.git-storage-limit` by both steps, before anything is checked out or
/// written. No file here is stored behind more than 100 entries, and 65 paths are within the
/// limit of 100. A baseline with a file for every path the limit admits, each stored behind 50
/// entries, the depth Git writes by default, is within the bound.
#[test]
fn the_objects_of_a_baseline_together_are_stored_behind_at_most_64_entries_for_each_admitted_path()
{
    assert_eq!(store::CHAIN_STEPS_PER_PATH, 64);
    let limit = StorageLimit {
        max_bytes: DEFAULT_BYTE_LIMIT,
        max_inodes: 100,
    };

    assert_eq!(
        checked(49, 100, limit).map(|checked| (checked.paths, checked.chain_steps)),
        Ok((100, 5000))
    );
    assert_eq!(
        checked(99, 64, limit).map(|checked| (checked.paths, checked.chain_steps)),
        Ok((64, 6400))
    );
    assert_eq!(create(99, 64, limit), (["served"; 2], 64, 2));

    assert_eq!(checked(99, 65, limit), Err(Unadmitted::Limit));
    assert_eq!(
        create(99, 65, limit),
        (["workspace.git-storage-limit"; 2], 0, 0)
    );
}

/// What an entry of a hand-written pack is stored behind.
#[derive(Clone, Copy)]
enum Behind {
    /// Nothing: the pack stores it as it is.
    Nothing,
    /// The object of this id.
    Id(u8),
    /// What begins this many bytes before it.
    Bytes(u8),
}

/// The id a hand-written pack gives the entry `number`.
fn id(number: u8) -> Oid {
    Oid::from_bytes(&[number; 20]).expect("id")
}

/// For each first byte, how many of `ids` begin with it or with a lower one: the fan-out table
/// of an index that holds them.
fn counted(ids: &[[u8; 20]]) -> [u32; 256] {
    let mut table = [0_u32; 256];
    for (first_byte, upto) in table.iter_mut().enumerate() {
        let count = ids
            .iter()
            .filter(|id| usize::from(id[0]) <= first_byte)
            .count();
        *upto = u32::try_from(count).expect("count");
    }
    table
}

/// An object store whose one pack holds `entries` in this order, each an empty object, and
/// the index of that pack: the entry at place `n` has the id of `n` repeated twenty times.
/// The index is written here and names every entry at its offset, whatever the entry is
/// stored behind.
fn object_store_of(entries: &[Behind]) -> tempfile::TempDir {
    let ids: Vec<[u8; 20]> = (0..entries.len())
        .map(|number| [u8::try_from(number).expect("fewer than 256 entries"); 20])
        .collect();
    object_store_with(entries, &ids, &counted(&ids))
}

/// The object store of [`object_store_of`] with an index that holds `ids` in this order, the
/// id at place `n` for the entry at place `n`, and the fan-out table `table`.
fn object_store_with(
    entries: &[Behind],
    ids: &[[u8; 20]],
    table: &[u32; 256],
) -> tempfile::TempDir {
    assert_eq!(entries.len(), ids.len(), "one id for each entry");
    let objects = tempdir().expect("object store");
    let mut pack = b"PACK".to_vec();
    pack.extend(2_u32.to_be_bytes());
    pack.extend(u32::try_from(entries.len()).expect("count").to_be_bytes());
    let mut offsets = Vec::new();
    for behind in entries {
        offsets.push(u32::try_from(pack.len()).expect("offset"));
        match *behind {
            // A file object (type 3) of no bytes.
            Behind::Nothing => pack.push(0x30),
            // Stored behind the object of an id (type 7).
            Behind::Id(number) => {
                pack.push(0x70);
                pack.extend([number; 20]);
            }
            // Stored behind what begins some bytes before it (type 6).
            Behind::Bytes(distance) => {
                assert!(distance < 0x80, "one byte names the distance");
                pack.extend([0x60, distance]);
            }
        }
        pack.extend(stored(&[]));
    }
    pack.extend([0; 20]);
    let mut index = b"\xfftOc".to_vec();
    index.extend(2_u32.to_be_bytes());
    for upto in table {
        index.extend(upto.to_be_bytes());
    }
    for id in ids {
        index.extend(id);
    }
    index.extend(vec![0; 4 * entries.len()]);
    for offset in offsets {
        index.extend(offset.to_be_bytes());
    }
    index.extend([0; 40]);
    let directory = objects.path().join("pack");
    std::fs::create_dir(&directory).expect("pack directory");
    std::fs::write(directory.join("pack-written.pack"), pack).expect("pack");
    std::fs::write(directory.join("pack-written.idx"), index).expect("index");
    objects
}

/// An entry stored behind an object named by its id is followed to that object wherever the
/// pack holds it, also behind a later entry, and one named by the bytes between them to the
/// entry that begins there. Each entry is read once: the first entry's chain gives the second
/// and the third theirs.
#[test]
fn the_chain_of_an_entry_is_followed_by_id_and_by_distance_wherever_its_base_lies() {
    // An empty object takes eleven bytes behind its header. The third entry has a header of
    // one byte, so the fourth begins 12 bytes after it; the fourth has a header of two bytes,
    // so the fifth begins 13 bytes after that.
    let objects = object_store_of(&[
        Behind::Id(1),
        Behind::Id(2),
        Behind::Nothing,
        Behind::Bytes(12),
        Behind::Bytes(13),
    ]);

    let chains = Chains::read(objects.path()).expect("a pack whose chains end");

    assert_eq!(
        [0, 1, 2, 3, 4].map(|number| chains.behind(id(number))),
        [2, 1, 0, 1, 2]
    );
    assert_eq!(chains.behind(id(9)), 0, "an object no pack holds");
}

/// A pack with an entry stored behind what is no entry of it, or with entries stored behind
/// each other in a circle, is refused as an object that cannot be read, which a create names
/// `workspace.git-checkout-failed`, and its chains are not followed further. The pass ends
/// for each of them.
#[test]
fn a_pack_with_an_entry_stored_behind_no_entry_or_behind_itself_is_refused() {
    let refused = |entries: &[Behind]| {
        let objects = object_store_of(entries);
        Chains::read(objects.path()).err()
    };

    assert_eq!(
        refused(&[Behind::Nothing, Behind::Id(7)]),
        Some(Unadmitted::Object),
        "an id the pack does not hold"
    );
    assert_eq!(
        refused(&[Behind::Nothing, Behind::Bytes(3)]),
        Some(Unadmitted::Object),
        "bytes before it where no entry begins"
    );
    assert_eq!(
        refused(&[Behind::Nothing, Behind::Bytes(0)]),
        Some(Unadmitted::Object),
        "itself, by a distance of no bytes"
    );
    assert_eq!(
        refused(&[Behind::Id(1), Behind::Id(0)]),
        Some(Unadmitted::Object),
        "each other"
    );
    assert_eq!(
        refused(&[Behind::Id(0)]),
        Some(Unadmitted::Object),
        "itself, by its id"
    );
}

/// Five ids for an index, ascending: one that begins with the lowest first byte, two that
/// share a first byte, one after first bytes no id begins with, and one that begins with the
/// highest first byte.
fn five_ids() -> [[u8; 20]; 5] {
    let id = |first: u8, last: u8| {
        let mut id = [first; 20];
        id[19] = last;
        id
    };
    [
        id(0x00, 0),
        id(0x10, 1),
        id(0x10, 2),
        id(0x40, 0),
        id(0xff, 0),
    ]
}

/// The pass and the Git library find an entry by its id in the index of the pack, between the
/// two places the fan-out table gives for the id's first byte. An index that holds its ids in
/// ascending order, each once, with a table that states for each first byte how many ids
/// begin with it or with a lower one, is read, and the pass finds the chain of each id: also
/// of two ids that share a first byte, and of ids before and after first bytes no id begins
/// with. Each of the five entries here is stored behind the one before it.
#[test]
fn an_index_whose_table_counts_its_ids_in_ascending_order_is_read() {
    let ids = five_ids();
    let table = counted(&ids);
    assert_eq!(
        [0x00, 0x0f, 0x10, 0x3f, 0x40, 0xfe, 0xff].map(|first_byte: usize| table[first_byte]),
        [1, 1, 3, 3, 4, 4, 5]
    );
    // An empty object takes eleven bytes behind its header: one byte for an entry the pack
    // stores as it is, two for one stored behind another.
    let entries = [
        Behind::Nothing,
        Behind::Bytes(12),
        Behind::Bytes(13),
        Behind::Bytes(13),
        Behind::Bytes(13),
    ];
    let objects = object_store_with(&entries, &ids, &table);

    let chains = Chains::read(objects.path()).expect("an index the pass reads");

    assert_eq!(
        ids.map(|id| chains.behind(Oid::from_bytes(&id).expect("id"))),
        [0, 1, 2, 3, 4]
    );
}

/// An index in which a search for an id could end at another entry than the one of that id,
/// or at none, is refused as a pack that cannot be read, which a create names
/// `workspace.git-checkout-failed`: one whose fan-out table counts one id too few for a first
/// byte two ids begin with, one whose table counts an id for a first byte before the id's
/// own, one that holds two ids out of order, and one that holds an id twice. Each differs in
/// that one thing from an index that is read.
#[test]
fn an_index_whose_table_does_not_count_its_ids_or_whose_ids_do_not_ascend_is_refused() {
    let ids = five_ids();
    let read = |ids: &[[u8; 20]; 5], table: &[u32; 256]| {
        let objects = object_store_with(&[Behind::Nothing; 5], ids, table);
        Chains::read(objects.path()).map(|_| ())
    };
    assert_eq!(read(&ids, &counted(&ids)), Ok(()));

    let mut one_too_few = counted(&ids);
    one_too_few[0x10] = 2;
    assert_eq!(
        read(&ids, &one_too_few),
        Err(Unadmitted::Object),
        "a first byte two ids begin with counts one of them"
    );

    let mut one_too_early = counted(&ids);
    one_too_early[0x3f] = 4;
    assert_eq!(
        read(&ids, &one_too_early),
        Err(Unadmitted::Object),
        "a first byte no id begins with counts the id after it"
    );

    let mut out_of_order = ids;
    out_of_order.swap(1, 2);
    assert_eq!(
        read(&out_of_order, &counted(&ids)),
        Err(Unadmitted::Object),
        "two ids out of order"
    );

    let mut twice = ids;
    twice[2] = twice[1];
    assert_eq!(
        read(&twice, &counted(&ids)),
        Err(Unadmitted::Object),
        "an id twice"
    );
}
