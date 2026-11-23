---
format: aep.planning-md/3
id: review-result:file-change-ledger-security-pass-1
kind: review-result
status: active
title: Security review of the ledger bound on a file change, pass 1
relations:
- reviews: story:file-change-result-fits-the-ledger-before-the-file-changes
revision: 1
---
# Security review, pass 1: a file change fits the ledger (pull request 267, `7eb2a5d0b`)

Review of the first form of `story:file-change-result-fits-the-ledger-before-the-file-changes`,
one commit on base `e11fcc988`. Verdict: NEEDS-CHANGE. The claimed rule held on both drivers:
the reviewer found no result the guarded crate admits and the store refuses. Counts as
reported: cases executed 294 to 312, 2 red; 18 cases added, 16 of them green. Seven findings:
four warnings, three notes. Origin: 6 introduced, 1 pre-existing. Both red cases are in
pre-existing code.

Pull request 267 was closed and replaced by pull request 270, a second form of the change in
one commit, reviewed in pass 2. What became of each finding there:

- The first form changed the executor's request, so a keeper of an earlier release would have
  refused the three v2 changes after an upgrade, which the design amendment denied. The second
  form leaves the executor's protocol as it is on `main`.
- A replacement refused for its precondition or its size left the parent directories it had
  created. The second form creates them after both checks.
- The changelog's first sentence stated more than the code held. Pass 2 read the new entry.
- The commit message said what was wrong before the fix was on `main`. The second form is
  another commit.
- The server held only the longer of the two published lengths of a replacement's content. The
  second form holds exactly the published rule.
- Three guards were measured by no case of the commit. The reviewer's green cases hold each.
- The daemon did not hold a driver's answer to the bound it handed out; no shipped driver was
  found to exceed it. The second form holds every answer to the bound before it completes the
  row.

Checked and found correct: the arithmetic of the terminal row against the headroom; that edit
and patch write nothing before their last refusal; the cut; that a text bound is refused before
admission, recorded as refused and leaves the file unchanged. Not checked by the reviewer: the
gate; the delegated and namespace lanes; an executor of an earlier release run against the new
daemon; anything run at the base.

```findings
[
{"file":"docs/design/34-kubernetes-namespace-driver.md","line":906,"category":"contract-drift","severity":"warning","verdict":"NEEDS-CHANGE","origin":"introduced","message":"The first form changed the executor's request, so a keeper of an earlier release would have refused the three v2 changes after an upgrade, which the amendment denied. The second form leaves the executor's protocol unchanged."},
{"file":"crates/substrate-guarded/src/fs.rs","line":440,"category":"concurrency","severity":"warning","verdict":"CONFIRMED","origin":"pre-existing","message":"A replacement created its parent directories before its precondition and size refusals. Rule: a replacement refused for its precondition or its size creates no directory."},
{"file":"CHANGELOG.md","line":23,"category":"contract-drift","severity":"warning","verdict":"NEEDS-CHANGE","origin":"introduced","message":"The entry's first sentence said every admitted v2 change ends with a successful terminal record, which the code does not hold. Rule for the text: a v2 change is written only when its result fits what the ledger reserved."},
{"file":"crates/substrate-daemon/tests/http.rs","line":639,"category":"judgement","severity":"warning","verdict":"CONFIRMED","origin":"introduced","message":"The commit message and two helper comments said what was wrong before the fix was on main. Rule: commit message, pull request and test names state the rule only."},
{"file":"crates/substrate-wire/src/lib.rs","line":1239,"category":"boundary","severity":"note","verdict":"CONFIRMED","origin":"introduced","message":"The server held only the longer of the two published lengths of a replacement's content. Rule: a content outside the published lengths is request.schema-invalid before dispatch."},
{"file":"crates/substrate-daemon/src/app/workspaces.rs","line":1028,"category":"mutant","severity":"note","verdict":"CONFIRMED","origin":"introduced","message":"The commit's suite did not measure the daemon's text bound on a replacement, the executor's pass-through of the bound or the namespace driver sending the daemon's bound. Three added green cases do."},
{"file":"crates/substrate-daemon/src/app/workspaces.rs","line":1122,"category":"integrity","severity":"note","verdict":"INFEASIBLE","origin":"introduced","message":"The daemon completed the result a driver returned without comparing it with the bound it handed out; no shipped driver was found to exceed it. The second form holds every answer to the bound before it completes the row."}
]
```
