---
format: aep.planning-md/3
id: review-result:git-changes-reader-adversary-round-1
kind: review-result
status: active
title: 'Adversary pass, round 1: the Git changes reader'
relations:
- reviews: story:git-changes-reader-reads-only-the-admitted-workspace
revision: 1
---
# Adversary pass, round 1: the Git changes reader (pull request 255, `232d9fe72`)

Verdict: blocker, NEEDS-CHANGE. Ten cases added, three red; three more red cases were the
security reviewer's.

- The rule the changelog stated did not hold for the object store, and one default behaviour
  regressed without being named: the index vouched for "unchanged" on time and size alone.
- A regression table against the earlier reader's library and options, run through the vendored
  libgit2, listed eleven behaviours; nine differed and were not named.
- Could not break: index flags, conflict stages, a 24-path ignore battery equal to Git 2.56,
  `max_files` and `max_total_bytes` truncation, FIFOs as worktree entries.
- Not checked: the old reader built from base, the daemon's vectors, the delegated lane.
- All findings were answered in round 2 (`8b6613866`).

```findings
[
{"file":"crates/substrate-host/src/git.rs","line":555,"category":"acceptance","severity":"blocker","verdict":"NEEDS-CHANGE","origin":"undecided","message":"The object store is given to libgit2 by path, so alternates and links beneath objects choose directories outside the root and a non-regular entry makes the observation wait."},
{"file":"crates/substrate-host/src/git.rs","line":774,"category":"acceptance","severity":"warning","verdict":"NEEDS-CHANGE","origin":"introduced","message":"The baseline size in the answer and in the over-bound unchanged test comes from an object header that is not verified against the daemon-held commit."},
{"file":"crates/substrate-host/src/git.rs","line":687,"category":"contract-drift","severity":"warning","verdict":"CONFIRMED","origin":"introduced","message":"The index vouches on modification time and size alone, so a changed tracked file within the bound is not reported; the earlier reader reported it and the commit message does not name the change."},
{"file":"crates/substrate-host/src/git/worktree.rs","line":231,"category":"boundary","severity":"warning","verdict":"CONFIRMED","origin":"introduced","message":"An ignore file that starts with a byte-order mark loses its first rule, which Git and the earlier reader apply."},
{"file":"crates/substrate-host/src/git/worktree.rs","line":152,"category":"boundary","severity":"note","verdict":"CONFIRMED","origin":"introduced","message":"A nest of more than 64 directories that holds no file refuses the whole observation; the earlier reader answered up to 100."},
{"file":"crates/substrate-host/src/git.rs","line":824,"category":"contract-drift","severity":"note","verdict":"CONFIRMED","origin":"introduced","message":"Workspace attributes and filters no longer shape the comparison or the patch text, and the commit message does not name it."}
]
```
