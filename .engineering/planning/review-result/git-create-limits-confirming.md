---
format: aep.planning-md/3
id: review-result:git-create-limits-confirming
kind: review-result
status: active
title: 'Confirming review: the review fixes of the Git create limits'
relations:
- reviews: story:git-create-work-and-memory-follow-its-limits
revision: 1
---
# Confirming review: the review fixes of the Git create limits

2026-11-24, on the second commit of the branch `fix/git-create-work-and-memory-limits`
(`3c91673bb`). The reviewer added eight cases in
`crates/substrate-host/src/git/review2_create_bounds_tests.rs` and did not edit the
implementation. Host Git cases executed 164 to 172, two red.

## Held

- What arrives is counted to the byte on each of the four ways the indexer reads a pack: 64
  times the byte limit is indexed and one byte more is refused.
- An entry that holds more than it states is read one byte further and no more.
- A well-formed pack is indexed to the same pack file and the same index with a byte limit and
  without one, and the index is the one `git index-pack` writes.
- A waiting object is kept at its own length, and its charge is given back once, also when the
  hold fails.
- `pack.threads=1` is the only thread setting of a create, and nothing in the traversal is
  ordered by a thread, a hash map or a time.
- Both `check-packages` refusals hold for the lock shapes the commit names, and the lock of the
  tree passes.
- The two thread cases fail when `INDEXING_THREADS` moves; the four memory cases are tighter
  than the case they replaced.

## Findings and what was done

1. Rule asked: the table count is at least what the indexer keeps per entry, the id pass's own
   lists included. Done in the third commit: an entry counts two items, the one the table keeps
   and the one the id pass keeps for it (128, 160 or 288 bytes with its lists). The reviewer's
   two cases hold it between full tables under 16 MiB and 32 MiB, with the figures of the
   count moved:
   `an_indexer_keeps_no_more_for_an_entry_stored_as_it_is_than_its_table_counts` and
   `an_indexer_keeps_no_more_for_an_entry_stored_behind_another_than_its_table_counts`.
   So that nothing is kept beside the two items, the queue of the id pass is filled from the
   items and the entries are sorted by id where they lie, with their place before the sorting
   as the second key;
   `an_index_lists_the_entries_of_one_id_in_the_order_they_had_before_the_sorting` holds that
   the index is the same bytes. Not held by a case, and stated in design 21 as measured: a
   table of up to 125,000 entries (read by a sample on a timer: at most 132.1 bytes for each
   entry of a pair against 144 counted); what an indexing takes once, which put entries in a
   line 10 to 35 KiB and entries all behind one object up to 88 KiB over the count of their
   table; and what is resident under the allocator's defaults, which the daemon runs with.
2. The memory paragraph of design 21 left out the pack file, which the fetch reads through a
   mapping, and the pages a process pays for once. Done: the paragraph states both, with the
   measured range for a pack that deflates and for one that does not, and the reviewer's two
   cases on a pack that does not deflate are in the tree.
3. Two paths of the vendored crate that no create takes were not named. Done:
   `README.substrate.md` names them, and that the fetch reads its pack with `Mode::Verify`.
4. Rule asked: `check-packages` refuses a second package of a patched name from any source.
   Done, with a case for each source kind.

One case of the first security review no longer fills its table under the count of a named id;
`a_fetch_counts_the_id_an_entry_names_and_its_list_in_its_table` holds the full table.

## Not established

The reviewer's cases through the TLS fetch (they use the indexer), more than one thread, a full
table together with a waiting byte limit, and the first commit.
