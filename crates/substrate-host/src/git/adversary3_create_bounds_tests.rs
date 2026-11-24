//! Cases of a third adversary pass on the order an index gives the entries of one id (design
//! 21, § *The copy of `gix-pack`*): the indexer sorts its entries by id where they lie, with
//! the place an entry had before the sorting as the second key. These cases hold that rule on
//! shapes the cases beside this file do not build: entries stored behind an object they name
//! by its id, an object stored as it is twice that entries are stored behind, entries stored
//! behind an entry that is itself stored behind another, a pack of one entry and a pack of
//! none.

use super::{
    BEHIND_AN_ID, Pack, Placed, entries_of_an_index, entry_header, entry_of, index_git_writes,
    index_of_the_fetch, instead, length, order_of_an_index, stored,
};

/// The id of the file object made of the eight bytes of `number`.
fn id_of(number: u64) -> gix::ObjectId {
    gix::objs::compute_hash(
        gix::hash::Kind::Sha1,
        gix::objs::Kind::Blob,
        &number.to_be_bytes(),
    )
    .expect("the id of a file object")
}

impl Pack {
    /// Adds an entry stored behind the file object made of `base`, which it names by its id:
    /// the eight bytes of `number`. Answers where the entry begins.
    fn small_behind_the_id_of(&mut self, base: u64, number: u64) -> u64 {
        let instructions = instead(8, &number.to_be_bytes());
        let offset = length(&self.bytes);
        self.bytes
            .extend(entry_header(BEHIND_AN_ID, length(&instructions)));
        self.bytes.extend(id_of(base).as_slice());
        self.bytes.extend(stored(&instructions));
        self.entries += 1;
        offset
    }
}

/// The entries of one id keep the order they had before the sorting where some are stored
/// behind an object they name by its id, and where an object entries are stored behind is
/// stored as it is twice.
///
/// The first pack holds 16 file objects of eight bytes five times each, in five rounds: each
/// stored as it is, each stored as it is again, each stored behind the first copy of another
/// object by its place, each stored behind another object by its id (an id the pack holds
/// twice), and each stored behind an entry of the third round, which is itself stored behind
/// another. Every entry stored as it is comes before every entry stored behind another, so the
/// order the index has for one id is also the order of the pack: the index is held entry by
/// entry to [`order_of_an_index`] and byte for byte to the one `git index-pack` writes.
///
/// The second pack stores each of 16 objects first behind one object by its id and then as it
/// is: of each id the entry stored as it is comes first, which is not the order of the pack.
/// That index is held to [`order_of_an_index`].
///
/// Holds now. No case held the order of one id for entries named by an id before.
#[test]
fn an_index_keeps_the_order_of_one_id_for_entries_stored_behind_an_id_and_behind_a_line() {
    const OBJECTS: u64 = 16;
    let mut pack = Pack::new();
    let mut placed = Vec::new();
    let mut firsts = Vec::new();
    for round in 0..2 {
        for number in 0..OBJECTS {
            let offset = pack.small(number);
            placed.push(Placed {
                number,
                behind: false,
                offset,
            });
            if round == 0 {
                firsts.push(offset);
            }
        }
    }
    let mut by_place = Vec::new();
    for number in 0..OBJECTS {
        let base = firsts[usize::try_from((number + 1) % OBJECTS).expect("an object")];
        let offset = length(&pack.bytes);
        pack.small_behind(base, number);
        by_place.push(offset);
        placed.push(Placed {
            number,
            behind: true,
            offset,
        });
    }
    for number in 0..OBJECTS {
        let offset = pack.small_behind_the_id_of((number + 2) % OBJECTS, number);
        placed.push(Placed {
            number,
            behind: true,
            offset,
        });
    }
    for number in 0..OBJECTS {
        let base = by_place[usize::try_from((number + 3) % OBJECTS).expect("an object")];
        let offset = length(&pack.bytes);
        pack.small_behind(base, number);
        placed.push(Placed {
            number,
            behind: true,
            offset,
        });
    }
    let sent = pack.sent();
    let (index, checksum) = index_of_the_fetch(&sent);
    assert_eq!(placed.len(), 80);
    assert_eq!(
        entries_of_an_index(&index, placed.len()),
        order_of_an_index(&placed)
    );
    assert!(
        index == index_git_writes(&sent),
        "the index is not the one git writes"
    );
    eprintln!("ids repeated behind ids and in a line: {checksum}");

    let mut pack = Pack::new();
    let mut placed = Vec::new();
    let base = pack.small(OBJECTS);
    placed.push(Placed {
        number: OBJECTS,
        behind: false,
        offset: base,
    });
    for number in 0..OBJECTS {
        let offset = pack.small_behind_the_id_of(OBJECTS, number);
        placed.push(Placed {
            number,
            behind: true,
            offset,
        });
    }
    for number in 0..OBJECTS {
        let offset = pack.small(number);
        placed.push(Placed {
            number,
            behind: false,
            offset,
        });
    }
    let sent = pack.sent();
    let (index, checksum) = index_of_the_fetch(&sent);
    assert_eq!(placed.len(), 33);
    assert_eq!(
        entries_of_an_index(&index, placed.len()),
        order_of_an_index(&placed)
    );
    eprintln!("ids behind an id before they are stored as they are: {checksum}");
}

/// The index the indexer writes of `sent` when it is handed the entries of the pack directly,
/// under a byte limit of 1 MiB on the threads a create names. A bundle of no entry writes no
/// index file, so this reads the index the sorting feeds.
fn index_of_the_indexer(sent: &[u8]) -> Vec<u8> {
    use gix::odb::pack;
    let mut arriving = pack::data::input::BytesToEntriesIter::new_from_header(
        sent,
        pack::data::input::Mode::Verify,
        pack::data::input::EntryDataMode::Crc32,
        gix::hash::Kind::Sha1,
    )
    .expect("the header of the pack");
    let mut index = Vec::new();
    pack::index::write_data_iter_to_stream(
        pack::index::Version::default(),
        || Ok((entry_of, sent)),
        &mut arriving,
        Some(crate::git::INDEXING_THREADS),
        &mut gix::progress::Discard,
        &mut index,
        &std::sync::atomic::AtomicBool::new(false),
        gix::hash::Kind::Sha1,
        Some(1024 * 1024),
        pack::data::Version::V2,
    )
    .expect("a well-formed pack");
    index
}

/// A pack of no entry and a pack of one entry are indexed to the index `git index-pack`
/// writes of them.
///
/// Holds now. No case held the index of a pack of no entry or of one before.
#[test]
fn an_index_of_a_pack_of_no_entry_and_of_one_entry_is_the_one_git_writes() {
    let empty = Pack::new().sent();
    assert!(
        index_of_the_indexer(&empty) == index_git_writes(&empty),
        "the index of no entry is not the one git writes"
    );

    let mut one = Pack::new();
    one.small(7);
    let one = one.sent();
    assert!(
        index_of_the_indexer(&one) == index_git_writes(&one),
        "the index of one entry by the indexer is not the one git writes"
    );
    let (index, checksum) = index_of_the_fetch(&one);
    assert_eq!(
        entries_of_an_index(&index, 1),
        order_of_an_index(&[Placed {
            number: 7,
            behind: false,
            offset: 12,
        }])
    );
    assert!(
        index == index_git_writes(&one),
        "the index of one entry is not the one git writes"
    );
    eprintln!("one entry: {checksum}");
}
