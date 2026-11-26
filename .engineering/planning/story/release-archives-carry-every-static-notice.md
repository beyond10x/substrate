---
format: aep.planning-md/3
id: story:release-archives-carry-every-static-notice
kind: story
status: draft
title: Every release archive carries the notices of the code statically linked into it
summary: The Rust standard library's notice and statically built C libraries are not in the archives' notices
revision: 1
---
# Story: every release archive carries the notices of the code statically linked into it

## Outcome

Each binary archive a release attaches, `gnu` and musl, carries the licence notices of every
piece of code linked into its executable that requires its notice in each copy: the Rust
standard library and the C code the `-sys` crates build and link statically (OpenSSL in the
musl build, aws-lc, SQLite, libgit2, curl, zlib), beside `THIRD_PARTY_LICENSES.html` and, in the
musl archives, `musl-COPYRIGHT`.

## Where the requirement comes from

Found on 2026-11-26 by the adversary pass of `story:release-publishes-static-musl-archives`. A
pre-existing finding, filed as a draft; it enters no wave until accepted. Stakeholder: the
repository owner.

## What was found

- `THIRD_PARTY_LICENSES.html` is made by cargo-about from the crate graph. It lists each crate's
  licence, not the licence of the C library a `-sys` crate builds, where the two differ, and it
  does not list the Rust standard library, whose MIT notice is in no archive.
- The musl archives carry musl's `COPYRIGHT` since the musl story.

## Open questions

- Whether cargo-about's template can add static sections for the standard library and each
  statically built C library, or whether the archives carry notice files beside it.

## Acceptance

A test of `xtask/tests/release_workflow.rs` requires, for each archive, a notice for the Rust
standard library and for each C library the build links statically, read from the build's own
dependency graph.
