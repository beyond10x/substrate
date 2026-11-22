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

/// An object store whose one pack holds `entries` in this order, each an empty object, and
/// the index of that pack: the entry at place `n` has the id of `n` repeated twenty times.
/// The index is written here and names every entry at its offset, whatever the entry is
/// stored behind, as no fetch writes one.
fn object_store_of(entries: &[Behind]) -> tempfile::TempDir {
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
    for first_byte in 0..=u8::MAX {
        let upto = entries.len().min(usize::from(first_byte) + 1);
        index.extend(u32::try_from(upto).expect("count").to_be_bytes());
    }
    for number in 0..entries.len() {
        index.extend([u8::try_from(number).expect("fewer than 256 entries"); 20]);
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
/// each other in a circle, is not one a fetch wrote: it is refused as an object that cannot
/// be read, which a create names `workspace.git-checkout-failed`, and its chains are not
/// followed further. The pass ends for each of them.
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
