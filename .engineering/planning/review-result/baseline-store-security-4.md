---
format: aep.planning-md/3
id: review-result:baseline-store-security-4
kind: review-result
status: active
title: Security review of the baseline store, fourth pass
relations:
- reviews: story:daemon-holds-the-baseline-objects-of-a-git-workspace
revision: 1
---
# Review: security review of the baseline store, fourth pass

Independent review of `c4e1d498c`, the fixes of the third pass. Verdict: needs change.

Two blockers in the build of a store for a workspace that predates it: the Git library's
indexer ran in the daemon's process on pack files a workspace wrote, with no bound on what a
refused attempt kept and none on its time. Not fixed as code: the operator decided that no such
build exists (D20), and it was removed with its tests.

Two blockers and two smaller findings in the shape check before a checkout, fixed in
`be3f47cc3`: the check counts entries as it reads, ties a tree's size to the entries it holds,
refuses a name that holds a separator, and sums the stated bytes of a baseline's files.

Checked and found correct: the memo's arithmetic at several prefix lengths and at each bound;
that the checkout refuses `.`, `..` and `.git` for every entry before any write and writes
nothing through a link a baseline names; that one driver exists per daemon.

Not checked by the reviewer: the base commit by running, the fetch library's validation, the
quota and delegated lanes. The removal of the build was not reviewed when this record was
written.

```findings
[
{"file":"crates/substrate-host/src/git/store.rs","line":257,"category":"integrity","severity":"blocker","verdict":"NEEDS-CHANGE","origin":"introduced","message":"The build of a store from a workspace's own pack files ran the Git library's indexer in the daemon's process with no bound on what a refused attempt kept. Closed by removing the build (operator's decision D20): the driver reads no pack a workspace wrote."},
{"file":"crates/substrate-host/src/git/store.rs","line":258,"category":"integrity","severity":"blocker","verdict":"NEEDS-CHANGE","origin":"introduced","message":"That indexing had no bound on time and no interruption while it held the only build turn (read, not run). Closed by removing the build."},
{"file":"crates/substrate-host/src/git/store.rs","line":489,"category":"boundary","severity":"blocker","verdict":"NEEDS-CHANGE","origin":"undecided","message":"The shape check compared the path count only when it left a tree. Rule: it counts as it reads and refuses once the distinct trees read hold more entries than the inode limit, keeping ids, not trees."},
{"file":"crates/substrate-host/src/git/store.rs","line":383,"category":"contract-drift","severity":"blocker","verdict":"NEEDS-CHANGE","origin":"undecided","message":"A tree's size was not tied to its entries. Rule: a tree that states more than 283 bytes for each entry it holds is refused."},
{"file":"crates/substrate-host/src/git/store.rs","line":455,"category":"boundary","severity":"warning","verdict":"CONFIRMED","origin":"undecided","message":"A name that holds a separator counted as one path. Rule: such a name is refused before anything is checked out."},
{"file":"crates/substrate-host/src/git/store.rs","line":464,"category":"judgement","severity":"note","verdict":"CONFIRMED","origin":"undecided","message":"The check did not sum the bytes a checkout writes. Rule: the stated sizes of a baseline's files, once per place, stay within the byte limit."},
{"file":"crates/substrate-host/src/git/tests/copy_cases.rs","line":201,"category":"judgement","severity":"warning","verdict":"NEEDS-CHANGE","origin":"introduced","message":"A test comment said more than the rule its case held. The file left with the build."},
{"file":"crates/substrate-host/src/git/store.rs","line":436,"category":"mutant","severity":"note","verdict":"CONFIRMED","origin":"undecided","message":"No case failed when the commit-size comparison of the store write was removed. One case now holds it."}
]
```
