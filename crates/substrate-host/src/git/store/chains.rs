//! How the packs of an object store the driver wrote store their objects behind each other
//! (design 21, § *The baseline store*, *Stored chains*).
//!
//! A pack may store an object as its difference from another entry, and that entry as its
//! difference from a third: a chain. The pack states the type of such an object only at the
//! end of its chain, so the Git library walks the whole chain each time it reads the object's
//! header, and keeps nothing of the walk. This module reads how many entries each entry of a
//! pack is stored behind, once and before the Git library reads any header.
//!
//! The pass first reads the ids of the pack's index once, in their order, and its fan-out
//! table ([`searched_alike`]): after that the Git library and the pass find the same entry by
//! an id. It then reads the header of each entry at the offset the index names, and of an
//! entry stored behind another the first bytes, at most [`SIZES_BYTES`] of what it inflates
//! to: they state the bytes of the object the entry makes. It reads nothing else of an
//! object. It follows an entry's chain only as far as the first entry it already knows, so it
//! reads each entry once: its work is one step for each entry of the pack, with one search
//! among the offsets, or in the index for a base named by its id, for each entry stored
//! behind another. It keeps eighteen bytes for each entry: its offset, how many entries it is
//! stored behind, and the bytes a whole read of it inflates and makes.
//!
//! **Whole reads** (design 21, § *The baseline store*, *The work and the memory of a create
//! follow its byte limit*). Reading an object whole inflates every entry the object is stored
//! behind and makes the object of each. For one entry those are the bytes its header states
//! it inflates to and, for an entry stored behind another, the bytes of the object it makes.
//! The pass keeps for each entry the sum of its own and of those of every entry it is stored
//! behind ([`Chains::whole`]).

use std::os::unix::fs::FileExt as _;
use std::path::Path;

use git2::Oid;
use gix::odb::pack;
use gix::odb::pack::data::entry::Header;

use super::Unadmitted;

/// How many entries of its pack an object may be stored behind: the deepest chain
/// `git pack-objects --depth` writes.
pub(in crate::git) const MAX_CHAIN: u16 = 4095;

/// An entry the pass has not come to.
const UNREAD: u16 = u16::MAX;

/// An entry whose chain the pass is following.
const FOLLOWED: u16 = u16::MAX - 1;

/// The first bytes an id can have: an index has one number of its fan-out table for each.
const FIRST_BYTES: usize = 256;

/// The bytes one size at the start of an entry stored behind another may take: each byte
/// holds seven bits of a number of 64.
const SIZE_BYTES: usize = 10;

/// The bytes the pass inflates of an entry stored behind another: the two sizes it begins
/// with, of the object it is stored behind and of the object it makes.
const SIZES_BYTES: usize = 2 * SIZE_BYTES;

/// Whether the Git library and this pass find the same entry by every id of `index`, the
/// index in the file `path`. Both search an id among the ids of the index, between the two
/// places the fan-out table gives for its first byte. So the index holds its ids in ascending
/// order, each once, and the table states for each first byte how many ids begin with it or
/// with a lower one: then every search for an id ends at the one entry of that id.
///
/// The ids are read once, in their order, and the table from the file, which is where the Git
/// library reads it. Nothing is kept beside one count for each first byte.
///
/// # Errors
///
/// [`Unadmitted::Object`] for an index that holds an id twice or out of order, for a table
/// that is not those counts, and for a file the table cannot be read from.
fn searched_alike(index: &pack::index::File, path: &Path) -> Result<(), Unadmitted> {
    let mut begin_with = [0_u32; FIRST_BYTES];
    let mut before: Option<&[u8]> = None;
    for row in 0..index.num_objects() {
        let id = index.oid_at_index(row).as_bytes();
        if before.is_some_and(|before| before >= id) {
            return Err(Unadmitted::Object);
        }
        begin_with[usize::from(id[0])] += 1;
        before = Some(id);
    }
    // An index of version 2 begins with eight bytes that say so; the table follows them.
    let table_at = match index.version() {
        pack::index::Version::V1 => 0,
        pack::index::Version::V2 => 8,
    };
    let mut table = [0_u8; 4 * FIRST_BYTES];
    std::fs::File::open(path)
        .and_then(|file| file.read_exact_at(&mut table, table_at))
        .map_err(|_| Unadmitted::Object)?;
    let mut upto = 0_u32;
    for (count, stated) in begin_with.iter().zip(table.chunks_exact(4)) {
        upto += count;
        if stated != upto.to_be_bytes() {
            return Err(Unadmitted::Object);
        }
    }
    Ok(())
}

/// One pack of the object store.
struct Pack {
    index: pack::index::File,
    /// The offsets of its entries, ascending.
    offsets: Vec<u64>,
    /// How many entries the entry at each of those offsets is stored behind.
    behind: Vec<u16>,
    /// The bytes a whole read of the entry at each of those offsets inflates and makes: its
    /// own and those of every entry it is stored behind.
    whole: Vec<u64>,
}

impl Pack {
    /// Reads the pack whose index is the file `path`, and the chain of each of its entries.
    fn read(path: &Path) -> Result<Self, Unadmitted> {
        let hash = gix::hash::Kind::Sha1;
        let data = pack::data::File::at(path.with_extension("pack"), hash)
            .map_err(|_| Unadmitted::Object)?;
        let index = pack::index::File::at(path, hash).map_err(|_| Unadmitted::Object)?;
        searched_alike(&index, path)?;
        let offsets = index.sorted_offsets();
        let mut behind = vec![UNREAD; offsets.len()];
        // Of an entry the pass has read and whose chain it is following: its own bytes.
        let mut whole = vec![0_u64; offsets.len()];
        let mut inflate = gix::zlib::Inflate::default();
        // The entries of the chain the pass is following, the one it started at first. An
        // entry stored behind as many entries as the list holds is over the bound, so the
        // list is never longer than the bound.
        let mut chain = Vec::new();
        for first in 0..offsets.len() {
            if behind[first] != UNREAD {
                continue;
            }
            let mut entry = first;
            // How many entries the last entry of the list is stored behind, and the bytes a
            // whole read of the entry it is stored behind inflates and makes.
            let (mut steps, mut bytes) = loop {
                if chain.len() > usize::from(MAX_CHAIN) {
                    return Err(Unadmitted::Limit);
                }
                chain.push(entry);
                behind[entry] = FOLLOWED;
                let read = Read::of(&data, &index, &offsets, entry, &mut inflate)?;
                whole[entry] = read.bytes;
                let Some(base) = read.base else {
                    break (0, 0);
                };
                match behind[base] {
                    UNREAD => entry = base,
                    // Entries stored behind each other in a circle: no object can be read
                    // from them.
                    FOLLOWED => return Err(Unadmitted::Object),
                    known => break (known + 1, whole[base]),
                }
            };
            while let Some(entry) = chain.pop() {
                if steps > MAX_CHAIN {
                    return Err(Unadmitted::Limit);
                }
                behind[entry] = steps;
                steps += 1;
                bytes = bytes.saturating_add(whole[entry]);
                whole[entry] = bytes;
            }
        }
        Ok(Self {
            index,
            offsets,
            behind,
            whole,
        })
    }

    /// The place of the object `id` among the entries of this pack; none when the pack does
    /// not hold it.
    fn entry(&self, id: &gix::hash::oid) -> Option<usize> {
        let offset = self.index.pack_offset_at_index(self.index.lookup(id)?);
        self.offsets.binary_search(&offset).ok()
    }

    /// How many entries the object `id` is stored behind in this pack; none when the pack
    /// does not hold it.
    fn behind(&self, id: &gix::hash::oid) -> Option<u16> {
        Some(self.behind[self.entry(id)?])
    }

    /// The bytes a whole read of the object `id` inflates and makes in this pack; none when
    /// the pack does not hold it.
    fn whole(&self, id: &gix::hash::oid) -> Option<u64> {
        Some(self.whole[self.entry(id)?])
    }
}

/// What the pass reads of one entry.
struct Read {
    /// The entry it is stored behind, as its place among the offsets; none for an entry the
    /// pack stores as it is.
    base: Option<usize>,
    /// The bytes a whole read inflates of this entry and, for an entry stored behind
    /// another, makes of it.
    bytes: u64,
}

impl Read {
    /// Reads the entry `entry` of `offsets`: its header, and of an entry stored behind
    /// another the two sizes it begins with ([`made`]).
    ///
    /// # Errors
    ///
    /// [`Unadmitted::Object`] for an entry whose header cannot be read; for one stored behind
    /// what is no entry of this pack, an offset that is not an earlier entry's or an id the
    /// index does not hold; and for one stored behind another whose first bytes do not state
    /// the two sizes.
    fn of(
        data: &pack::data::File,
        index: &pack::index::File,
        offsets: &[u64],
        entry: usize,
        inflate: &mut gix::zlib::Inflate,
    ) -> Result<Self, Unadmitted> {
        let offset = offsets[entry];
        let read = data.entry(offset).map_err(|_| Unadmitted::Object)?;
        let base = match read.header {
            Header::OfsDelta { base_distance } => offset
                .checked_sub(base_distance)
                .filter(|base| *base < offset),
            Header::RefDelta { base_id } => index
                .lookup(base_id)
                .map(|held| index.pack_offset_at_index(held)),
            Header::Commit | Header::Tree | Header::Blob | Header::Tag => {
                return Ok(Self {
                    base: None,
                    bytes: read.decompressed_size,
                });
            }
        };
        let base = base
            .and_then(|base| offsets.binary_search(&base).ok())
            .ok_or(Unadmitted::Object)?;
        let end = u64::try_from(data.pack_end()).map_err(|_| Unadmitted::Object)?;
        let stored = data
            .entry_slice(read.data_offset..end)
            .ok_or(Unadmitted::Object)?;
        Ok(Self {
            base: Some(base),
            bytes: read
                .decompressed_size
                .saturating_add(made(inflate, stored)?),
        })
    }
}

/// The bytes of the object an entry stored behind another makes. `stored` begins with what
/// the pack stores of the entry behind its header. An entry stored behind another inflates to
/// two sizes, of the object it is stored behind and of the object it makes, and then the
/// instructions that make it. No more than [`SIZES_BYTES`] are inflated, whatever the entry
/// states.
///
/// # Errors
///
/// [`Unadmitted::Object`] for what does not inflate, and for what does not begin with two
/// sizes of at most [`SIZE_BYTES`] each: no object can be read from such an entry.
fn made(inflate: &mut gix::zlib::Inflate, stored: &[u8]) -> Result<u64, Unadmitted> {
    let mut first = [0_u8; SIZES_BYTES];
    inflate.reset();
    let (_, _, inflated) = inflate
        .once(stored, &mut first)
        .map_err(|_| Unadmitted::Object)?;
    let mut bytes = first[..inflated].iter();
    let _stored_behind = size(&mut bytes)?;
    size(&mut bytes)
}

/// One size as an entry stored behind another states it: seven bits in each byte, the lowest
/// first, and a byte with its highest bit clear last.
fn size(bytes: &mut std::slice::Iter<'_, u8>) -> Result<u64, Unadmitted> {
    let mut size = 0_u64;
    for (place, byte) in bytes.take(SIZE_BYTES).enumerate() {
        size |= u64::from(byte & 0x7f) << (7 * place);
        if byte & 0x80 == 0 {
            return Ok(size);
        }
    }
    Err(Unadmitted::Object)
}

/// How the packs of one object store store their objects behind each other.
pub(in crate::git) struct Chains {
    packs: Vec<Pack>,
}

impl Chains {
    /// Reads the chains of every pack in the object store at `objects`, a directory the
    /// driver wrote. An object store without a pack has no chain.
    ///
    /// # Errors
    ///
    /// [`Unadmitted::Limit`] for a pack with an entry stored behind more than [`MAX_CHAIN`]
    /// entries, found before that chain is followed further. [`Unadmitted::Object`] for a
    /// pack or an index that cannot be read as one, for an index of [`searched_alike`]'s
    /// errors, and for an entry of [`Read::of`]'s errors or one stored behind itself.
    pub(in crate::git) fn read(objects: &Path) -> Result<Self, Unadmitted> {
        let listing = match std::fs::read_dir(objects.join("pack")) {
            Ok(listing) => listing,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self { packs: Vec::new() });
            }
            Err(_) => return Err(Unadmitted::Object),
        };
        let mut packs = Vec::new();
        for entry in listing {
            let path = entry.map_err(|_| Unadmitted::Object)?.path();
            if path.extension().is_some_and(|extension| extension == "idx") {
                packs.push(Pack::read(&path)?);
            }
        }
        Ok(Self { packs })
    }

    /// How many entries the Git library walks to read the header of the object `id`: the
    /// most any pack stores it behind, and none for an object no pack holds.
    pub(in crate::git) fn behind(&self, id: Oid) -> u64 {
        let Ok(id) = gix::hash::oid::try_from_bytes(id.as_bytes()) else {
            return 0;
        };
        self.packs
            .iter()
            .filter_map(|pack| pack.behind(id))
            .max()
            .map_or(0, u64::from)
    }

    /// The bytes a whole read of the object `id` inflates and makes: of the entry a pack
    /// holds it as and of every entry that one is stored behind, the most of any pack that
    /// holds it. None for an object no pack holds.
    pub(in crate::git) fn whole(&self, id: Oid) -> Option<u64> {
        let id = gix::hash::oid::try_from_bytes(id.as_bytes()).ok()?;
        self.packs.iter().filter_map(|pack| pack.whole(id)).max()
    }
}
