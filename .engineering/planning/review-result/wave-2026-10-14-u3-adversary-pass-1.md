---
format: aep.planning-md/3
id: review-result:wave-2026-10-14-u3-adversary-pass-1
kind: review-result
status: active
title: Adversary pass 1, unit u3 (directory listing special entries)
relations:
- reviews: story:directory-listing-survives-special-entries
revision: 1
---
fixed

Adversary pass 1 on the directory-listing unit, against 7de8157e.

Findings:
- **Confirmed, fixed.** A FIFO the listing now omits stays addressable by path: a v1 file read
  opened it without `O_NONBLOCK` and blocked waiting for a writer before the regular-file check
  could refuse it (`a_fifo_omitted_from_listings_still_blocks_a_v1_file_read_of_its_path`, red:
  "a file read of a FIFO answers within 3 s instead of blocking").
- **Confirmed, fixed.** The same open in `read_complete_from` blocked v2 digested reads and the
  current-content observation of replace, edit and patch
  (`a_fifo_omitted_from_listings_still_blocks_a_v2_digested_read_of_its_path`, red; and
  `every_compare_and_set_mutation_of_a_fifo_path_answers_instead_of_blocking`, red).
- **Probe, green.** A deep omission still reports `truncated`; an excluded hidden FIFO is no
  omission; an exact-limit tree is not truncated; omitted entries never consume `limit_items`.

Fix: 348bc20c. Both reading opens use one `FILE_READ_FLAGS` that includes `O_NONBLOCK`; the
existing `fstat` check refuses a non-regular file with the refusal it already answered. A scan case
(`no_open_of_a_workspace_path_can_wait_for_a_fifo_writer`) holds every open in `fs.rs` to a
non-blocking shape and went red when the old flags were restored. No wire change.

Not established: the Git storage-measure walk (`git.rs`) opens a file after a
`symlink_metadata().is_file()` check; a swap between the two needs a writer into the staging
target during materialization, inferred unreachable, not measured.
