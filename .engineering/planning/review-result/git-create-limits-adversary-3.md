---
format: aep.planning-md/3
id: review-result:git-create-limits-adversary-3
kind: review-result
status: active
title: 'Adversary pass 3: the limits of a Git create, and the patch rule of check-packages'
relations:
- reviews: story:git-create-work-and-memory-follow-its-limits
revision: 1
---
# Adversary pass 3: the limits of a Git create, and the patch rule of `check-packages`

2026-11-24, on the third commit of the branch `fix/git-create-work-and-memory-limits`
(`a7c306e6a`, parent `3c91673bb`). The adversary added two cases in
`crates/substrate-host/src/git/adversary3_create_bounds_tests.rs` and three in the test module
of `xtask/src/packages.rs`, and did not edit the implementation. Verdict: the bound and the
vendored copy of gix-pack held; the patch rule of `cargo xtask check-packages` did not, in three
cases. Host Git cases executed 173 to 175, red 0; `xtask` 726 to 729, red 3. S1: 0. S2: 3.

## Held

- The count of a pack's entries is checked for the stated number before the first entry and
  after every entry added, with saturating arithmetic; the reservation is at most the count.
- The roots of a traversal are the same, in the same order, and the copy builds without the
  `parallel` feature.
- An index keeps the order of one id for entries stored behind an id and behind a line: 80
  entries equal `git index-pack` byte for byte, and 33 entries with reference children before
  their roots keep the roots-first order.
- An index of a pack of no entry and of one entry is the one Git writes.
- The figures of the design and of the vendored copy's README agree with the code and the
  cases: 16,777,216, 13,421,772, 7,456,540 and 8,192.

Not established by the pass: that the build without `parallel` gives the same results at run
time (built, not run); resident memory under the allocator's defaults; no mutant was run.

## Findings and what was done

Each finding is a rule of design 21 § *The copy of gix-pack* that the check did not hold for a
hand-built manifest. No file of the repository has such an entry.

1. S2. Rule asked: a second package of a patched name from a Git source is refused, whatever
   its revision. Done in `240dfa35e`: a Git entry is held to its reference.
2. S2. Rule asked: an entry is held to the lock under every table cargo applies to crates.io.
3. S2. Rule asked: an entry in a cargo configuration is held to the lock as one in the
   manifest is.

Findings 2 and 3 were first answered in `240dfa35e` by reading more tables and files. Pass 4
(`review-result:git-create-limits-adversary-4`) showed that an open list of the places cargo reads
cannot be complete, and the rule was closed: only `[patch.crates-io]` in the root manifest
carries a patch, and the places that record lists are refused by name.
