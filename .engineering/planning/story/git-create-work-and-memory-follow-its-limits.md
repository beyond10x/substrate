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
  path: crates/substrate-host/src/git
revision: 2
---
# Story: the work and the memory of a Git create follow its limits

## Outcome

A Git create spends, on what its source sends, work and memory in proportion to the limits the
create was admitted with (the byte limit and the inode limit), and is refused
`workspace.git-storage-limit` when the source asks for more.

## The rule

- Every read of an object during a create is bounded by the create's byte limit before the
  object is read, the reads the store's writer makes included.
- The table the fetch keeps of the entries of a pack is reserved in proportion to the inode
  limit, not to the count the pack's header states.
- The memory the fetch holds while it resolves an object is bounded by the byte limit whatever
  the depth the object is stored at.

## Design

Design 21, § *The baseline store*, states the limits a create holds. This story adds no limit
and no refusal code; it moves three places where the limit is decided to before the work.

## Acceptance

One case for each rule, against a source served on loopback: the create is refused
`workspace.git-storage-limit`, the workspace is proved absent, and the test process's peak
resident memory stays under a bound the case states. Each case runs under 5 s.

