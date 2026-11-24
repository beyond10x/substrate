---
format: aep.planning-md/3
id: review-result:git-create-limits-adversary
kind: review-result
status: active
title: 'Adversary pass: the work and the memory of a Git create follow its byte limit'
relations:
- reviews: story:git-create-work-and-memory-follow-its-limits
revision: 1
---
# Adversary pass: the work and the memory of a Git create follow its byte limit

2026-11-24, on the first commit of the branch `fix/git-create-work-and-memory-limits`
(`3a2504977`, base `16ddbec11`). The adversary added five cases in
`crates/substrate-host/src/git/adversary_create_bounds_tests.rs` and did not edit the
implementation. Host Git cases executed 144 to 149, three red.

## Held

- A charge for a waiting object is given back once.
- Neither counter wraps at the contract's limits.
- An allocation limit of zero is a limit of zero.
- Entries stored whole are counted by the bound of what the id pass makes.
- A `[[patch.unused]]` entry is refused by `check-packages`.

## Findings and what was done

1. Rule asked (design 21): what waits at once follows how the entries of the pack branch, so
   the same source and limit are served every time. With two indexing threads the count also
   followed which entries the threads made at the same time. Done in the second commit: the
   fetch of a create indexes on one thread (`INDEXING_THREADS`), a case holds the setting, and
   the adversary's fetch case
   `a_fetch_of_two_files_with_history_within_its_byte_limit_is_served` is kept as written. Its
   two cases that compared one thread with two now require the same answer in each of eight
   attempts. Indexing the ordinary-create pack takes 35 to 58 ms on one thread and 27 to 38 ms
   on two.
2. The design named no figure for the memory an indexer keeps. Done: design 21 states it (3.03
   to 3.23 times the byte limit, measured at 8 MiB and 16 MiB), and four cases hold it under 4
   times. The adversary's one case under 8 times became those four.
3. Rule asked: the bytes inflated while the pack arrives are counted. Done, as finding 1 of
   `review-result:git-create-limits-security`.
4. Rule asked: a waiting object is charged what it holds, whatever was made before it. Done: it
   is kept and charged at its own length, with a case.
5. `check-packages` accepted a second registry package of a patched name. Done, as finding 3 of
   the security review.
6. The per-entry table check had no case. Done:
   `an_indexer_counts_each_entry_that_arrives_whatever_number_was_stated` reaches it, and
   `README.substrate.md` says that no create does.

## Not established

A fetch from a large public repository, thin packs whose bases are found, a hosted runner's
four CPUs, and macOS.
