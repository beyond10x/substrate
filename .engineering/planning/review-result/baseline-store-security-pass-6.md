---
format: aep.planning-md/3
id: review-result:baseline-store-security-pass-6
kind: review-result
status: active
title: Security review of the baseline store, pass 6
relations:
- reviews: story:daemon-holds-the-baseline-objects-of-a-git-workspace
revision: 1
---
# Review: security review of the baseline store, sixth pass

Independent review of the fixes of the fifth pass (pull request 261): what changed since
`b72382f72` in the host crate, design 21 and the changelog, with the worktree at `67e2d254d`.
Verdict: CONFIRMED. The claims on the chain pass, its two bounds and the fetch's allocation
limit hold for a pack and an index the fetch writes. The bound on the work of a create holds
for header reads only. Counts as reported: cases executed 96 to 109, 5 red; 13 cases added. Ten
findings: three warnings, seven notes. Origin: 7 introduced, 3 pre-existing.

The three warnings exist on `main` as well and are not fixed in release 0.25.0. They are the
draft `story:git-create-work-and-memory-follow-its-limits`: the reads of whole objects during a
create, the table the fetch keeps of a pack's entries, and what the fetch holds while it
resolves an object are not yet bounded by the create's limits.

The seven notes were answered on the branch before the release. The chain pass refuses, as
`workspace.git-checkout-failed`, an index whose ids do not ascend, each once, or whose fan-out
table is not the count of their first bytes. Four sentences of public text were narrowed to what
the code holds. The text says that an allocation within the limit that the host refuses carries
the limit's name. A case holds again that an observation takes one place and waits for no other.
Nine of the reviewer's cases are kept in the repository; the four that hold the rules of the
draft story wait for it outside the repository.

Checked and found correct: the chain pass as a reader of hostile bytes (no panic, no unbounded
loop, 10 bytes of memory per entry); that the pass precedes every object read of a create; the
summed bound at 1 path, 0 paths and the largest limit; that the allocation limit is checked
before each allocation for a base, a delta buffer and a delta result. Not checked by the
reviewer: a mutation run of the guards; a run at the base; the gate, the workspace tests, the
no-default-features lane and the delegated lane.

```findings
[
{"file":"crates/substrate-host/src/git/store.rs","line":356,"category":"boundary","severity":"warning","verdict":"CONFIRMED","origin":"pre-existing","message":"The reads of whole objects during a create are not yet bounded by the create's limits before the object is read. Draft story git-create-work-and-memory-follow-its-limits."},
{"file":"crates/substrate-host/src/git.rs","line":181,"category":"integrity","severity":"warning","verdict":"CONFIRMED","origin":"pre-existing","message":"The table the fetch keeps of the entries of a pack is not yet reserved in proportion to the create's limits. Draft story git-create-work-and-memory-follow-its-limits."},
{"file":"crates/substrate-host/src/git.rs","line":120,"category":"boundary","severity":"warning","verdict":"CONFIRMED","origin":"pre-existing","message":"The memory the fetch holds while it resolves an object is not yet bounded by the byte limit; the allocation limit is per object. Draft story git-create-work-and-memory-follow-its-limits."},
{"file":"crates/substrate-host/src/git/store/chains.rs","line":98,"category":"integrity","severity":"note","verdict":"INFEASIBLE","origin":"introduced","message":"For an index whose fan-out table is not what its ids give, the pass and the Git library could find different entries for one id; no writer of such an index was found. Rule: such an index is refused workspace.git-checkout-failed."},
{"file":"CHANGELOG.md","line":81,"category":"contract-drift","severity":"note","verdict":"NEEDS-CHANGE","origin":"introduced","message":"The entry said both chain bounds refuse before the header of any object is read. That holds for the 4,095 bound; the summed bound refuses before the checkout. The entry says so now."},
{"file":"docs/design/21-connector-authorized-git-sources.md","line":119,"category":"contract-drift","severity":"note","verdict":"NEEDS-CHANGE","origin":"introduced","message":"The text said a pack with an entry stored behind itself is not one a fetch writes. A fetch can write one; the guard refuses it. The claim is removed."},
{"file":"crates/substrate-host/src/git/store.rs","line":480,"category":"contract-drift","severity":"note","verdict":"NEEDS-CHANGE","origin":"introduced","message":"The rustdoc said the work of the check does not follow how the pack stores objects. It is narrowed to reads of object headers."},
{"file":"docs/design/21-connector-authorized-git-sources.md","line":130,"category":"contract-drift","severity":"note","verdict":"CONFIRMED","origin":"introduced","message":"The text said the fetch inflates each entry once as it arrives. It says now that the fetch inflates each entry as it arrives and again when it computes the ids."},
{"file":"crates/substrate-host/src/git.rs","line":225,"category":"judgement","severity":"note","verdict":"CONFIRMED","origin":"introduced","message":"A host allocation failure within the limit is named workspace.git-storage-limit, which only a code comment stated. Design 21 and the changelog state it now."},
{"file":"crates/substrate-host/src/git/baseline_store_tests.rs","line":477,"category":"mutant","severity":"note","verdict":"CONFIRMED","origin":"introduced","message":"With the removed observation case no case held that an observation takes one file-operation place and waits for no other. A case holds it again."}
]
```
