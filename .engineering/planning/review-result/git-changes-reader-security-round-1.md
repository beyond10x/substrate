---
format: aep.planning-md/3
id: review-result:git-changes-reader-security-round-1
kind: review-result
status: active
title: 'Security review, round 1: the Git changes reader'
relations:
- reviews: story:git-changes-reader-reads-only-the-admitted-workspace
revision: 1
---
# Security review, round 1: the Git changes reader (pull request 255, `232d9fe72`)

Verdict: NEEDS-CHANGE. The worktree side held the rule; the object-store side did not. Three red
cases.

- Held: every worktree open beneath the pinned root; a link read as its own target text; the
  baseline identity kept outside every workspace, mode 0600; object content bound to its id.
- Did not hold: a size from an unchecked object header; the object store resolved by path; an
  entry that is not a regular file opened blocking; the per-file bound claimed for every file.
- Not verified: anything at base by running; the daemon's vectors and the delegated lane.
- All findings were answered in round 2 (`8b6613866`).

```findings
[
{"file":"crates/substrate-host/src/git.rs","line":771,"category":"acceptance","severity":"blocker","verdict":"NEEDS-CHANGE","origin":"introduced","message":"A baseline size is taken from an object header that is not hash-checked, so a link in the object store puts a number read from a file outside the root into the answer and the unchanged decision."},
{"file":"crates/substrate-host/src/git.rs","line":553,"category":"integrity","severity":"warning","verdict":"CONFIRMED","origin":"undecided","message":"The object store is resolved by path below the pinned descriptor with links and info/alternates honoured, so baseline objects are read from a store outside the root; content stays hash-bound."},
{"file":"crates/substrate-host/src/git.rs","line":771,"category":"boundary","severity":"warning","verdict":"CONFIRMED","origin":"undecided","message":"An object-store entry that is not a regular file is opened without O_NONBLOCK and the observation waits without bound."},
{"file":"CHANGELOG.md","line":12,"category":"contract-drift","severity":"warning","verdict":"NEEDS-CHANGE","origin":"introduced","message":"The text and the test name at git.rs:1274 claim the per-file bound for every file the reader opens, but the index, object files and info/alternates are unbounded and ignore files use MAX_IO_BYTES."}
]
```
