---
format: aep.planning-md/3
id: review-result:baseline-store-security-2
kind: review-result
status: active
title: Security review of the baseline store, second pass
relations:
- reviews: story:daemon-holds-the-baseline-objects-of-a-git-workspace
revision: 1
---
# Review: security review of the baseline store, second pass

Independent review of `6d5169e4c`, the fixes of the first pass. Verdict: needs change.

One blocker: work followed the paths a baseline names, not its objects, in the checkout of a
create, in the write of a store and in each observation. The nesting bound did not hold it.
Fixed in `dbe1f9092`: the check sums the paths of a baseline, reading each tree once, refuses
more than the inode limit, and runs before anything is checked out.

Also: the design sentence that the path count needed no bound was false; the wait for a build
turn had no bound and the turn outlasted the build; a comment was off by one. All in
`dbe1f9092`. One note needed no code: a store whose index cannot be read as one counts as not
holding the commit, and the design now says so.

Checked and found correct: the nesting check precedes the pack write on both paths and does not
recurse; no exit leaves one entry of a create; the turn is released on error, panic and
cancellation; the baseline-file read takes its size from the store and still checks the object.

The reviewer's three cases that asserted a duration were replaced by cases that assert the
refusal by name.

```findings
[
{"file":"crates/substrate-host/src/git.rs","line":277,"category":"property","severity":"blocker","verdict":"NEEDS-CHANGE","origin":"pre-existing","message":"The checkout of a create did work in the paths a baseline names, before any check. Rule: the path count of a baseline is checked against the inode limit, each tree read once, before anything is checked out."},
{"file":"crates/substrate-host/src/git/store.rs","line":327,"category":"property","severity":"blocker","verdict":"NEEDS-CHANGE","origin":"introduced","message":"The write of a store did work in the paths of a baseline the nesting check admitted. Rule: the same path bound, held before the store is written, at create and in a build."},
{"file":"crates/substrate-host/src/git.rs","line":632,"category":"property","severity":"warning","verdict":"NEEDS-CHANGE","origin":"pre-existing","message":"Every changes observation did work in the paths the stored baseline names. Rule: every store holds a baseline within the path bound, so an observation's walk is bounded by it."},
{"file":"docs/design/21-connector-authorized-git-sources.md","line":125,"category":"contract-drift","severity":"warning","verdict":"NEEDS-CHANGE","origin":"introduced","message":"The design said the path count needed no bound because the checkout stayed within the inode limit. A directory that holds nothing costs no inode; the sentence was false and is replaced by the stated bound."},
{"file":"crates/substrate-host/src/lib.rs","line":1470,"category":"concurrency","severity":"warning","verdict":"CONFIRMED","origin":"introduced","message":"The wait for the build turn had no bound and the turn was held through the comparison. Rule: the turn ends when the store is in place or the build is refused, and a wait over its bound is a named refusal."},
{"file":"crates/substrate-host/src/git/store.rs","line":119,"category":"integrity","severity":"note","verdict":"INFEASIBLE","origin":"introduced","message":"A store whose pack index cannot be read as one is answered as not holding the commit and replaced. Nothing was found that reaches that state; the design now states this reading."},
{"file":"crates/substrate-host/src/git/store.rs","line":27,"category":"judgement","severity":"note","verdict":"CONFIRMED","origin":"introduced","message":"The comment of MAX_BASELINE_DEPTH allowed one level more than the code and its cases."}
]
```
