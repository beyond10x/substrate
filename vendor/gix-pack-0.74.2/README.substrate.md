# Substrate gix-pack bounds patch

This directory is the published `gix-pack` 0.74.2 crate, the version `gix` 0.87.1 resolves. It was
made from the crate whose crates.io checksum is
`3d199d515cbdcf05f531f3859be9771fc12acac7b7fe0f5b581b2c1ab1c3a61e`. Substrate adds four counts
to the indexing of a pack that arrives. Each is held to a multiple of the allocation limit the
caller gives (`gitoxide.objects.allocLimit`), the most the crate allocates for one object; a
caller that gives none gets none of the four. Indexing over one of them ends with the error the
crate has for an allocation over that limit, `traverse::Error::OutOfMemory`. No public item of
the crate changes. The change adds two counters the threads of a traversal share, one count to
the iterator of the entries that arrive and one to the loop that fills the table, no thread and
no lock. It fills the queue of roots of a traversal from the items, with no list of that work
beside the queue, and sorts the items by id where they lie, with no list beside them.

| count | multiple of the limit | held in |
|---|---|---|
| the bytes the entries inflate to while the pack arrives | 64 | `src/data/input/bytes_to_entries.rs` |
| the bytes of the table of entries: two items for each entry and the lists beside them | 1 | `src/index/write/mod.rs`, counted in `src/cache/delta/tree.rs` |
| the bytes of the objects that wait for the entries stored behind them | 1 | `src/cache/delta/traverse/resolve.rs` |
| the bytes the traversal inflates and makes | 64 | `src/cache/delta/traverse/resolve.rs` |

## What differs from the published crate

- `src/data/input/bytes_to_entries.rs`. `BytesToEntriesIter::with_alloc_limit`, visible in the
  crate only, gives the iterator a budget of `INFLATED_BYTES_PER_LIMIT` (64) times the limit.
  With a budget, `next_inner` inflates no more of an entry than the bytes the entry states and
  one more, and no more than the budget has left and one more. An entry that inflates to more
  than it states is the size mismatch the crate already has (`input::Error::IncompletePack`).
  The entry that would take what was inflated over the budget ends the iteration with an
  `input::Error::Io` that holds the private marker `OverInflateBudget`, which
  `BytesToEntriesIter::is_over_inflate_budget` recognises. Without a budget an entry is
  inflated to its end, as in the published crate.
- `src/bundle/write/mod.rs`. Each of the four iterators `write_to_directory` and
  `write_to_directory_eagerly` make is given `options.alloc_limit_bytes` through
  `with_alloc_limit`.
- `src/cache/delta/tree.rs`. `Tree::with_capacity` reserves for no more than `RESERVED_ITEMS`
  (4,096) items in each of its two lists, 8,192 in all, whatever count it is given; the lists
  grow with the items that are added. `Tree::bytes_of_items` states the bytes a count of
  entries is counted as: `ITEMS_PER_ENTRY` (2) items of 64 bytes each, the one the tree keeps
  and one for what the indexing keeps for the entry beside the tree (a place in the queue of
  the id pass, the list the items are merged into).
  `Tree::add_child` and `Tree::add_child_by_id` answer the bytes the tree keeps for the
  child beside its item: four for each index the list it joins newly reserves for, `LIST_BYTES`
  (16) for a list that reserved for none before, and `NAMED_ID_BYTES` (128) for an id no
  earlier child named.
- `src/index/write/mod.rs`. `table_over_limit` holds the bound of the table:
  `Tree::bytes_of_items` of its entries and what `add_child` and `add_child_by_id` answered
  take no more bytes than the limit. It is asked for
  the count the pack states, before the first entry is read, and after each entry that was
  added. The error of an iterator over its budget is answered as
  `traverse::Error::OutOfMemory`. The items are sorted by id where they lie
  (`sort_unstable_by_key`), so the sorting keeps no list beside them. Its second key is the
  place an item has before the sorting, written to `next_offset`, which the traversal reads
  last and the writing of the index does not read. Items of one id therefore keep the order a
  sorting that leaves equal ids in place gives them, the roots in the order of the pack and
  after them the children in the order of the pack, and the index of a pack is the bytes the
  published crate writes of it.
- `src/cache/delta/traverse/resolve.rs`. A `Budget` holds the two counters and the limit, and
  the functions of the traversal take it where they took the limit.
  - `Budget::hold` holds the bound of the objects that wait. An object that entries are stored
    behind is kept until each of them has been made. It is kept at its own length
    (`shrink_to_fit`) and counted as that length when it is kept, whatever the buffer it was
    made in had reserved for an entry before it, and given back when it is dropped (`Charge`).
    The object that would take the count over the limit ends the traversal.
  - `Budget::resolve` holds the bound of what the traversal makes. The bytes an entry inflates
    to and the bytes of the object a delta makes are counted before they are allocated, each
    once; the traversal ends before the count is over `RESOLVED_BYTES_PER_LIMIT` (64) times the
    limit.
  - `resize_with_limit` reserves exactly the bytes it is asked for.
  - `all` gives `resolve_parallel` the work of the roots as an iterator, and `resolve_parallel`
    fills its queue of roots from it: no list of that work is kept beside the queue. Without
    the crate's `parallel` feature `all` collects the iterator into the stack `resolve_serial`
    works on, as the published crate does. The roots are queued in the order of the items, as
    before.
- `rustfmt.toml` is Substrate's: `max_width = 120`, the width the crate's source is written in.
  The workspace's `cargo fmt --all --check` reads a `[patch]` source, and with this file the
  published source is what it expects.
- `.cargo-ok`, the empty marker the extraction of the published crate leaves, as
  `vendor/axum-0.8.9` has it.
- This file.

Every other file is the published one, `.cargo_vcs_info.json` and `Cargo.toml.orig` included.

Four things a create never reaches, and what holds them:

- More entries than a pack states arrive only where the caller gives a store of earlier objects
  that holds an object an entry names: the iterator then puts that object before the entry. A
  create fetches into an empty store. The check after each entry is held for that path by a case
  that hands `index::write_data_iter_to_stream` an iterator that states no count
  (`an_indexer_counts_each_entry_that_arrives_whatever_number_was_stated`).
- An object put before an entry this way is read from the store, not inflated from the pack:
  the count of what the entries inflate to while the pack arrives holds the entries of the
  pack, and no object of a store. The store of a create is empty, so every object its fetch
  indexes is an entry of the pack and is counted.
- The count of what arrives ends the indexing with `traverse::Error::OutOfMemory` where the
  pack is read with `input::Mode::Verify`, which the fetch gives (`gix` 0.87.1,
  `src/remote/connection/fetch/receive_pack.rs`): the iterator answers the error of the entry
  over its budget, and `src/index/write/mod.rs` answers it as that one. Under
  `input::Mode::Restore` the iterator ends at the first entry it does not read to its end, as
  the published crate has it for every such entry, and answers no error: the budget still ends
  the read there, at the bytes it has left and one more, and the indexing goes on with the
  entries before it.
- `write_to_directory_eagerly` is not called by a fetch. Its iterators take the budget the same
  way. Cases run it on a pack at the budget and one byte over
  (`an_indexer_counts_what_arrives_to_the_byte_on_each_way_it_reads_a_pack` in
  `review2_create_bounds_tests.rs`).

## Why

The work and the memory of a Git create follow its byte limit (design 21, § *The baseline
store*). A create gives its byte limit as the allocation limit, and the four counts make what
the entries inflate to while the pack arrives, the table of entries, the objects that wait and
the bytes the indexing makes follow it. A create computes the ids on one thread
(`pack.threads=1`), so which objects wait at once follows the pack alone. The host crate's cases
in `crates/substrate-host/src/git/create_bounds_tests.rs`, `security_create_bounds_tests.rs` and
`adversary_create_bounds_tests.rs` run a fetch against a pack within every other bound and over
one of them, at the byte or the entry where each bound turns, and state the refusal and the
memory of the test process. Those in `review2_create_bounds_tests.rs` give a pack to the
indexer on each of the four ways it reads one, with the options the fetch gives it.

## What an update of gix must preserve

`cargo xtask check-packages` refuses the `[patch.crates-io]` entry when `Cargo.lock` does not
resolve `gix-pack` to this directory, and when it holds a second `gix-pack` from any source,
whatever its version: the registry, a `sparse+` registry, a Git source or another path. So an
update of `gix` that resolves another `gix-pack` fails the gate until the entry is moved. Such
an update takes the published source of the `gix-pack` it resolves as
`vendor/gix-pack-<version>`, makes the change above in it again and runs those cases. What the
change must still do in the new source, file by file:

- the iterator of the entries that arrive counts every byte it inflates, stops an entry at the
  bytes it states and one more, and is given the limit by every caller that indexes a pack;
- the fetch reads its pack with `input::Mode::Verify`, the mode in which the iterator answers
  the error of the entry over its budget, and into a store that holds no object when the
  fetch of a create begins;
- the table counts each entry as two items, each list at what it reserves and each id that
  entries name, and is checked for the stated count and after each entry;
- the id pass allocates no more for a root than its place in the queue of roots, the sorting
  by id keeps no list beside the items, and the indexing keeps no more than one item for an
  entry beside the tree at any one time. The three cases
  `an_indexer_keeps_no_more_for_an_entry_…_than_its_table_counts` in
  `review2_create_bounds_tests.rs` measure it between full tables of 16 MiB and of 32 MiB; a
  source that sorts the items another way, or queues the roots another way, is measured again;
- the items of one id are written in the order they have before the sorting, the roots and
  after them the children, each in the order of the pack, and nothing reads `next_offset` of an
  item after the traversal. `an_index_lists_the_entries_of_one_id_in_the_order_they_had_before_the_sorting`
  in `create_bounds_tests.rs` holds two indexes of packs that hold objects more than once to
  that order, to their checksums and, where it is the order of the pack, to the index
  `git index-pack` writes;
- an object that waits is kept and counted at its own length, and given back when it is dropped;
- what the traversal inflates and makes is counted before it is allocated;
- each of the four ends as `traverse::Error::OutOfMemory`, the error a create names
  `workspace.git-storage-limit`.

The copy is removed with the update whose published `gix-pack` passes the cases unchanged. The
request for these counts in the published crate is
<https://github.com/GitoxideLabs/gitoxide/issues/3244>.

The upstream crate's licences remain in `LICENSE-APACHE` and `LICENSE-MIT`.
