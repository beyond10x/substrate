---
format: aep.planning-md/3
id: review-result:baseline-store-security-1
kind: review-result
status: active
title: Security review of the baseline store, first pass
relations:
- reviews: story:daemon-holds-the-baseline-objects-of-a-git-workspace
revision: 1
---
# Review: security review of the baseline store, first pass

Independent review of `1ec20d788` against design 21 and design 23. Verdict: confirmed findings,
none against the main rule.

Ran: with a store, an observation makes no system call that names the workspace's
`.git/objects`; the only file mappings are the store's own pack and index. The copy for a
workspace that predates the store follows no link, checks the type on the held descriptor and
counts bytes as read.

Seven findings, in the block below. Three cases were left red by the reviewer; two are kept
unchanged in `git/tests/store_failure_cases.rs`, the third was replaced by cases at the bound.
All were fixed in `6d5169e4c`; the second finding is closed by the first one's bound, because
every store then holds a baseline within it.

Not checked by the reviewer: the build without default features, the quota's limits, races by
running, malformed packs in a copy.

```findings
[
{"file":"crates/substrate-host/src/git/store.rs","line":209,"category":"boundary","severity":"warning","verdict":"CONFIRMED","origin":"introduced","message":"The write of a store walked the baseline's trees with no bound on their nesting. Rule: a baseline nested deeper than a stated bound is a named refusal before anything walks it."},
{"file":"crates/substrate-host/src/git.rs","line":602,"category":"boundary","severity":"warning","verdict":"CONFIRMED","origin":"pre-existing","message":"The reader's walk of the baseline's trees had no bound on their nesting either. Rule: the same bound, held for every store, so no reader walks deeper."},
{"file":"crates/substrate-host/src/git/store.rs","line":290,"category":"integrity","severity":"note","verdict":"INFEASIBLE","origin":"introduced","message":"A sync failure after the store's rename returned an error and left the store. Rule: a failed write leaves neither entry."},
{"file":"crates/substrate-host/src/git.rs","line":346,"category":"integrity","severity":"note","verdict":"INFEASIBLE","origin":"introduced","message":"A sync failure after the commit-id file was in place removed the store and left the file. Rule: a failed write leaves neither entry."},
{"file":"crates/substrate-host/src/git/store.rs","line":87,"category":"judgement","severity":"note","verdict":"INFEASIBLE","origin":"introduced","message":"Every failure to open a store counted as absence, and the store was removed before a replacement existed. Rule: only absence and a missing commit lead to a build, and a store is replaced only by a finished one."},
{"file":"crates/substrate-host/src/git/store.rs","line":58,"category":"concurrency","severity":"note","verdict":"CONFIRMED","origin":"introduced","message":"An observation waiting for the build turn held one of the driver's places for file operations. Rule: the wait holds none."},
{"file":"crates/substrate-host/src/git.rs","line":384,"category":"judgement","severity":"note","verdict":"CONFIRMED","origin":"pre-existing","message":"The baseline-file read loaded the object before it compared its size with max_bytes. Rule: the size comes from the store's header and the limit is answered before the object is read."}
]
```
