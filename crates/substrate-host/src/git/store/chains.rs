//! How the packs of an object store the driver wrote store their objects behind each other
//! (design 21, § *The baseline store*, *Stored chains*).
//!
//! A pack may store an object as its difference from another entry, and that entry as its
//! difference from a third: a chain. The pack states the type of such an object only at the
//! end of its chain, so the Git library walks the whole chain each time it reads the object's
//! header, and keeps nothing of the walk. This module reads how many entries each entry of a
//! pack is stored behind, once and before the Git library reads any header.
//!
//! The pass reads the header of each entry at the offset the pack's index names, and nothing
//! of the object. It follows an entry's chain only as far as the first entry it already
//! knows, so it reads each entry once: its work is one step for each entry of the pack, with
//! one search among the offsets, or in the index for a base named by its id, for each entry
//! stored behind another. It keeps ten bytes for each entry: its offset and how many entries
//! it is stored behind.

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

/// One pack of the object store.
struct Pack {
    index: pack::index::File,
    /// The offsets of its entries, ascending.
    offsets: Vec<u64>,
    /// How many entries the entry at each of those offsets is stored behind.
    behind: Vec<u16>,
}

impl Pack {
    /// Reads the pack whose index is the file `index`, and the chain of each of its entries.
    fn read(index: &Path) -> Result<Self, Unadmitted> {
        let hash = gix::hash::Kind::Sha1;
        let data = pack::data::File::at(index.with_extension("pack"), hash)
            .map_err(|_| Unadmitted::Object)?;
        let index = pack::index::File::at(index, hash).map_err(|_| Unadmitted::Object)?;
        let offsets = index.sorted_offsets();
        let mut behind = vec![UNREAD; offsets.len()];
        // The entries of the chain the pass is following, the one it started at first. An
        // entry stored behind as many entries as the list holds is over the bound, so the
        // list is never longer than the bound.
        let mut chain = Vec::new();
        for first in 0..offsets.len() {
            if behind[first] != UNREAD {
                continue;
            }
            let mut entry = first;
            // How many entries the last entry of the list is stored behind.
            let mut steps = loop {
                if chain.len() > usize::from(MAX_CHAIN) {
                    return Err(Unadmitted::Limit);
                }
                chain.push(entry);
                behind[entry] = FOLLOWED;
                let Some(base) = base_of(&data, &index, &offsets, entry)? else {
                    break 0;
                };
                match behind[base] {
                    UNREAD => entry = base,
                    // Entries stored behind each other in a circle: no object can be read
                    // from them, and no fetch writes them.
                    FOLLOWED => return Err(Unadmitted::Object),
                    known => break known + 1,
                }
            };
            while let Some(entry) = chain.pop() {
                if steps > MAX_CHAIN {
                    return Err(Unadmitted::Limit);
                }
                behind[entry] = steps;
                steps += 1;
            }
        }
        Ok(Self {
            index,
            offsets,
            behind,
        })
    }

    /// How many entries the object `id` is stored behind in this pack; none when the pack
    /// does not hold it.
    fn behind(&self, id: &gix::hash::oid) -> Option<u16> {
        let offset = self.index.pack_offset_at_index(self.index.lookup(id)?);
        let entry = self.offsets.binary_search(&offset).ok()?;
        Some(self.behind[entry])
    }
}

/// The entry the entry `entry` is stored behind, as its place among `offsets`; none for an
/// entry the pack stores as it is.
///
/// # Errors
///
/// [`Unadmitted::Object`] for an entry whose header cannot be read, and for one stored behind
/// what is no entry of this pack: an offset that is not an earlier entry's, or an id the
/// index does not hold. The pack of a fetch is complete, so neither is what a fetch wrote.
fn base_of(
    data: &pack::data::File,
    index: &pack::index::File,
    offsets: &[u64],
    entry: usize,
) -> Result<Option<usize>, Unadmitted> {
    let offset = offsets[entry];
    let header = data.entry(offset).map_err(|_| Unadmitted::Object)?.header;
    let base = match header {
        Header::OfsDelta { base_distance } => offset
            .checked_sub(base_distance)
            .filter(|base| *base < offset),
        Header::RefDelta { base_id } => index
            .lookup(base_id)
            .map(|held| index.pack_offset_at_index(held)),
        Header::Commit | Header::Tree | Header::Blob | Header::Tag => return Ok(None),
    };
    base.and_then(|base| offsets.binary_search(&base).ok())
        .map(Some)
        .ok_or(Unadmitted::Object)
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
    /// pack or an index that cannot be read as one, and for an entry of [`base_of`]'s errors
    /// or one stored behind itself.
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
}
