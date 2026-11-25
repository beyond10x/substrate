---
format: aep.planning-md/3
id: story:the-not-owned-writable-root-case-holds-in-the-build-pool
kind: story
status: draft
title: The not-owned writable-root case holds on every host, the build pool's included
relations:
- informed_by: story:exec-start-carries-executable-lists
revision: 1
---
# Story: the not-owned writable-root case holds on every host, the build pool's included

## Outcome

`writable_roots::tests::a_changed_root_is_refused_by_name_with_its_code` in
`crates/substrate-host/src/writable_roots.rs` passes or prints `ABSENT (` in a job of the
workstation's build pool, and its `exec.writable-root-not-owned` case no longer depends on who
owns `/usr`.

## What was seen

2026-11-25, in the build pool, release profile, at commit `c5b4d0037c` and at its child
`0a12d40099` (the first two commits of `feat/executable-lists-on-the-wire`):

```
cargo nextest run -p b10x-substrate-host --release --locked --lib \
  -E 'test(a_changed_root_is_refused_by_name_with_its_code)'
```

failed with `/usr: [PinnedWritableRoot { … host_path: "/usr", mount: "/srv/cache" }]`: the pin of
`/usr` as a writable root succeeded, so `/usr` belonged to the effective uid of the job, and that
uid was not 0 (the case adds `/usr` only then). 810 of the 811 library cases of the host and
exec-gate packages passed in the same run. On the hosted runner the gate's test step is green at
`main` (`68d76a45d8`), so `/usr` is not the job's there.

Not read: why `/usr` belongs to the job's user in the pool (a user namespace that maps the uid,
or a build root of the user's own). Neither commit touches `writable_roots.rs`.

## Where the requirement comes from

Found by the session that works this repository while it checked step 2 of
`story:exec-start-carries-executable-lists`. `AGENTS.md` § *The gate*: a missing guarantee is a
named absence, never a failed or a passed case that did not hold its rule (invariant 3), and the
collection's rule that every build and test runs in the pool. Stakeholder: the operator.

## What changes

1. The case stops assuming an owner of `/usr`. It asks the file system: a directory that exists
   and whose `st_uid` differs from the effective uid (candidates, in order: `/`, `/usr`,
   `/proc/1` where it can be opened as a directory). With one found, the pin of it is
   `exec.writable-root-not-owned`.
2. With none found (every candidate belongs to the effective uid, or the uid is 0) the case
   prints `ABSENT (no directory of another owner on this host)` for that row and holds its other
   five rows.
3. Any other library case of the crate that names an owner of a system path gets the same form:
   `git grep -n '"/usr"\|"/etc"\|"/proc/1"' -- crates/substrate-host/src` is read line by line.

## Acceptance

- In a job of the build pool the command above exits 0, and its output shows either that the
  not-owned row ran against a named directory or the `ABSENT (` line.
- On the hosted runner the row runs: the gate's test step shows no `ABSENT (` line for it.
- The other five rows of the case are unchanged.

## Boundaries

- Not the driver's rule: `pin` keeps refusing a root of another owner
  (`crates/substrate-host/src/writable_roots.rs`, the `st_uid` comparison).
- Not the pool's environment: that is the slot repository's.

## Scope

`crates/substrate-host/src/writable_roots.rs` (its test module only).
