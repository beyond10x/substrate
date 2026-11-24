---
format: aep.planning-md/3
id: story:git-create-work-and-memory-follow-its-limits
kind: story
status: draft
title: The work and the memory of a Git create follow its limits
tags:
- security
scope:
- confidence: inferred
  path: CHANGELOG.md
- confidence: inferred
  path: Cargo.lock
- confidence: inferred
  path: Cargo.toml
- confidence: inferred
  path: crates/substrate-host/src/git
- confidence: cited
  path: crates/substrate-host/src/git.rs
- confidence: inferred
  path: crates/substrate-host/src/git/materialization_tests.rs
- confidence: cited
  path: docs/design/21-connector-authorized-git-sources.md
- confidence: inferred
  path: vendor
- confidence: inferred
  path: xtask/src
revision: 5
---
# Story: the work and the memory of a Git create follow its limits

## Outcome

A Git create spends, on what its source sends, work and memory in proportion to the byte limit
the create was admitted with, and is refused `workspace.git-storage-limit` when the source asks
for more.

## The rule

Design 21, § *The baseline store*, amendment of 2026-11-24, states four bounds, each a multiple
of the create's byte limit:

1. Whole reads. The bytes that whole reads of the baseline's objects inflate and make, summed
   over the places the baseline names them, are at most 64 times the byte limit. The check
   holds it before any file object is read.
2. The table of entries. The table the fetch keeps of a pack's entries grows with the entries
   that arrive, is reserved for at most 8,192 items before the first, and takes no more bytes
   than the byte limit.
3. Objects that wait. The objects the fetch holds while it computes ids take no more than the
   byte limit, however the entries of the pack branch.
4. What the fetch makes. The bytes the fetch inflates and makes while it computes ids are at
   most 64 times the byte limit.

Changed against the first text of this story, on the design spike of 2026-11-23 to 24: rule 2
is tied to the byte limit and not the inode limit, because the fetch receives the byte limit
only and a pack's entries include history; rule 4 is new, because without it the work rule 1
bounds is done inside the fetch; and rule 3 is a bound the design did not have, so the story
does add limits. It adds no refusal code.

## Design

Design 21, § *The baseline store*, the amendment named above. Bound 1 is held in
`crates/substrate-host/src/git/store`. Bounds 2 to 4 are held in a copy of `gix-pack` the
repository carries as `vendor/gix-pack-0.74.2`, selected by `[patch.crates-io]` as
`vendor/axum-0.8.9` is; the amendment states its upkeep.

## What changes

1. `vendor/gix-pack-0.74.2`: the published source with the three bounds (about 100 added lines
   in three files, no public item changed), its `README.substrate.md`, the `[patch.crates-io]`
   entry, `Cargo.lock`, the third-party notice and whatever `cargo xtask check-licenses` and
   `check-advisories` ask for a carried copy.
2. `cargo xtask check-packages` refuses a `[patch.crates-io]` entry that `Cargo.lock` does not
   resolve to, with a case for both sides.
3. The whole-read bound in the reading of the chains and the baseline check.
4. A loopback source that serves a hand-built pack (a new mode of the existing fixture in
   `crates/substrate-host/src/git/materialization_tests.rs`), and the four cases.
5. `CHANGELOG.md`, in the form the repository's rule for fixes asks.

## Acceptance

One case for each of the four bounds, against a source served on loopback: a pack within every
other bound and over that one. The create is refused `workspace.git-storage-limit`, the
workspace is proved absent, and the memory of the test process stays under a bound the case
states (peak resident memory; for the table of entries peak address space). Each case runs
under 5 s. The host crate's existing cases of a create pass against the copy unchanged, and an
ordinary create of 1,000 files with 50 commits of history passes.

## Where the requirement comes from

`review-result:baseline-store-security-pass-6`. Stakeholder: the operator.
