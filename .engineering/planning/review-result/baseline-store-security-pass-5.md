---
format: aep.planning-md/3
id: review-result:baseline-store-security-pass-5
kind: review-result
status: active
title: Security review of the baseline store, pass 5
relations:
- reviews: story:daemon-holds-the-baseline-objects-of-a-git-workspace
revision: 1
---
# Review: security review of the baseline store, fifth pass

Independent review of `b72382f72` (pull request 261), the reduced change after the store build
was removed. Verdict: NEEDS-CHANGE. Counts as reported: cases executed 84 to 87, 1 red; the
selection ran 87 cases, 86 passed and 1 failed, in 33.7 s. Seven findings: one blocker, two
warnings, four notes. Origin: 4 introduced, 3 pre-existing.

The blocker: the shape check and the pack builder read each object's header by walking its whole
stored chain, so the header work of a create followed files times chain length. Fixed in
`451ddedbd`: one pass reads how deep each pack entry is stored before any header is read. A pack
with an entry stored behind more than 4,095 entries is refused `workspace.git-storage-limit`,
and so is a baseline stored behind more than 64 entries together for each admitted path.

Both warnings are answered in the same commit. The fetch allocates for one object no more than
the create's byte limit. Design 21 and the changelog state the bounds the create holds, and the
changelog clause on what an earlier release accepted is deleted.

The notes: four cases in `crates/substrate-host/src/git/tests/guard_cases.rs` hold the no-replace
rename, the no-clobber persist and both link checks; the fix round names no case for the bound
on the pack stream. The leftovers of the removed build are removed or made private. Two notes
stay: the reads an observation makes of the baseline commit, which go to
`story:git-read-limits-bound-the-work`, and three timing bounds that are pre-existing. The fix
round ended at 96 cases, none red, and the sixth pass reviewed it.

Checked and found correct: no path of an observation, a create, a destroy or a start opens,
stats, maps or waits beneath a workspace's own object store; the shape bounds as written and
before the checkout; the order, failure points and modes of the store write. Not checked by the
reviewer: the fetch library against a real server; the quota, delegated and namespace lanes;
the daemon crate; the wire bundle.

```findings
[
{"file":"crates/substrate-host/src/git/store.rs","line":228,"category":"property","severity":"blocker","verdict":"NEEDS-CHANGE","origin":"introduced","message":"The shape check and the pack builder read each object's header by walking its whole stored chain. Rule: one pass reads the chains before any header is read; a pack entry stored behind more than 4,095 entries, or a baseline stored behind more than 64 entries for each admitted path, is refused before the checkout."},
{"file":"crates/substrate-host/src/git.rs","line":118,"category":"boundary","severity":"warning","verdict":"CONFIRMED","origin":"pre-existing","message":"The fetch indexed the pack with no allocation limit. Rule: the fetch allocates for one object no more than the create's byte limit, and an object that states more ends the fetch workspace.git-storage-limit."},
{"file":"crates/substrate-host/src/git/store.rs","line":91,"category":"boundary","severity":"note","verdict":"CONFIRMED","origin":"pre-existing","message":"The reads an observation makes of the baseline commit are not yet inside its comparison budget. Goes to story git-read-limits-bound-the-work."},
{"file":"docs/design/21-connector-authorized-git-sources.md","line":132,"category":"contract-drift","severity":"warning","verdict":"CONFIRMED","origin":"introduced","message":"Two sentences stated bounds the create did not hold, for header walks and for the fetch's indexing. Both were made true for the code after the fix."},
{"file":"crates/substrate-host/src/git/store.rs","line":554,"category":"mutant","severity":"note","verdict":"CONFIRMED","origin":"introduced","message":"No case reached the no-replace rename, the no-clobber persist, the link checks on the directory of the stores or the bound on the pack stream. Four cases now hold the rename, the persist and both link checks."},
{"file":"crates/substrate-host/src/git.rs","line":1774,"category":"judgement","severity":"note","verdict":"CONFIRMED","origin":"pre-existing","message":"Three timing bounds of 2 to 3 seconds can fail cases under load, one of them in a case the branch changed. Left as it is."},
{"file":"crates/substrate-host/src/git/worktree.rs","line":66,"category":"judgement","severity":"note","verdict":"CONFIRMED","origin":"introduced","message":"The removal of the store build left a wider visibility, unreachable match arms, a fixture name, a copying helper and a case for the removed turn. Removed or made private in the fix."}
]
```
