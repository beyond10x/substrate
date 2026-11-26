---
format: aep.planning-md/3
id: story:full-table-memory-cases-hold-on-any-c-library
kind: story
status: draft
title: The full-table memory cases hold the indexer to its count on any C library
summary: The peak-RSS figure moves with the glibc allocator by more than the case's margin
revision: 2
---
# Story: the full-table memory cases hold the indexer to its count on any C library

## Outcome

The cases of `git::review2_create_bounds_tests` and `git::adversary_create_bounds_tests` that
hold what the pack indexer keeps for one entry of a full table pass or fail by what the indexer
holds, on every glibc a workstation or a runner uses, and not by how the C library's allocator
places the indexer's threads.

## Where the requirement comes from

Found on 2026-11-26 while running the host crate's tests on a build host with glibc 2.34. A
pre-existing finding, filed as a draft; it enters no wave until accepted. Stakeholder: the
repository owner.

## What was measured

`an_indexer_keeps_no_more_for_an_entry_stored_behind_another_than_its_table_counts` reads the
growth of the process's peak resident memory (`VmHWM`) between tables filled under 16 MiB and
32 MiB, per entry, less the pack's own bytes, against the 144 bytes the table counts.

| C library | arenas | bytes kept for an entry | runs |
|---|---|---|---|
| glibc 2.34 | the library's own | 99 to 150, about 21 of 26 over 144 | 26 |
| glibc 2.34 | `M_ARENA_MAX` 1 (tunable or `mallopt`) | 128 to 133 | 18 |
| glibc 2.43 (hosted runner) | the library's own | 107 to 112; every gate run since 2026-11-24 passed | about 25 |
| glibc 2.43 (hosted runner) | `M_ARENA_MAX` 1 (`mallopt`) | 146 | 1 (gate run 45765288815) |

The peak under 32 MiB is bimodal on 2.34 (about 30.8 or 34.8 MiB) and was 34.6 MiB in the one
run on 2.43 with one arena. The figure moves by more than the margin between what the indexer
keeps and what the table counts.

## Open questions

- Whether the measure should read what the indexer allocates (a counting global allocator in the
  test binary) instead of what the process has resident.
- Whether the margin of 144 bytes holds the indexer to its count or to the allocator's overhead.

## Acceptance

The cases pass on glibc 2.34 and on the hosted runner's glibc in every run of a repeat
measurement recorded in the planning store, and fail when the indexer keeps one entry's bytes
more than its table counts.

The quarantine of that case on glibc older than 2.43 (its `QUARANTINED` line and the
`success-output` override in `.config/nextest.toml`) is removed by this story.
