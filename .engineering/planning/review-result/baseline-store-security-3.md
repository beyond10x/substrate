---
format: aep.planning-md/3
id: review-result:baseline-store-security-3
kind: review-result
status: active
title: Security review of the baseline store, third pass
relations:
- reviews: story:daemon-holds-the-baseline-objects-of-a-git-workspace
revision: 1
---
# Review: security review of the baseline store, third pass

Independent review of `dbe1f9092`, the fixes of the second pass. Verdict: needs change.

One blocker: the shape check bounded how many paths a baseline names and how deep, and not how
long they are. Fixed in the next commit: names, paths and their total length are bounded before
anything is checked out, and the commit and each tree are compared with their size bounds before
they are read.

One warning in the build for a workspace that predates the store: it read pack indexes the
workspace wrote. Fixed in the next commit: only pack files and loose objects are copied, and the
driver indexes each copied pack itself.

One warning on tests that depended on a duration under load; they now hold a gate.

Checked and found correct: each tree is read once and the sum saturates; a missing or wrong-typed
object is refused; no walk of the tree precedes the check at create; a refused create leaves
nothing; the turn is released on error, panic and cancellation; an observation that needs no
build never touches the turn.

Not checked by the reviewer: the fetch library's memory per object, the quota and delegated
lanes. The fixes of this pass were not reviewed independently when this record was written.

```findings
[
{"file":"crates/substrate-host/src/git/store.rs","line":325,"category":"acceptance","severity":"blocker","verdict":"NEEDS-CHANGE","origin":"undecided","message":"The check bounded path count and nesting but not the length of names and paths. Rule: a name over 255 bytes, a path over 4,096 bytes and paths that together exceed 256 bytes per admitted path are named refusals before anything is checked out."},
{"file":"crates/substrate-host/src/git/store.rs","line":304,"category":"integrity","severity":"warning","verdict":"NEEDS-CHANGE","origin":"introduced","message":"A build read pack indexes a workspace wrote. Rule: a build copies only pack files and loose objects and indexes each copied pack itself; a pack whose objects do not all resolve ends the attempt by name."},
{"file":"crates/substrate-host/src/git/baseline_store_tests.rs","line":518,"category":"concurrency","severity":"warning","verdict":"CONFIRMED","origin":"introduced","message":"A case needed sixteen builds in turn inside the 10 s wait and had about 3 s of margin under load. The cases about the turn now hold a gate and assert order, with no bound on a duration."},
{"file":"crates/substrate-host/src/git/store.rs","line":322,"category":"boundary","severity":"note","verdict":"CONFIRMED","origin":"undecided","message":"A tree was read whole up to the byte limit, and the commit before its header was compared. Rule: a tree's header is compared with the byte limit and with 283 bytes per admitted path, the commit's with the byte limit, before either is read."},
{"file":"crates/substrate-host/src/lib.rs","line":1551,"category":"concurrency","severity":"note","verdict":"CONFIRMED","origin":"introduced","message":"The build turn is also held while the build waits for one of the driver's places for file operations. No holder of a place waits for a turn, so nothing can wait in a circle. Left as it is."},
{"file":"crates/substrate-host/src/git/tests/shape_cases.rs","line":25,"category":"judgement","severity":"note","verdict":"CONFIRMED","origin":"introduced","message":"A fixture comment and one commit message said more than the rule. The comment now describes the fixture only; the commit message is pushed and stays."}
]
```
