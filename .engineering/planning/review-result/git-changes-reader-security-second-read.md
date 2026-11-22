---
format: aep.planning-md/3
id: review-result:git-changes-reader-security-second-read
kind: review-result
status: active
title: 'Security review, second read: the Git changes reader'
relations:
- reviews: story:git-changes-reader-reads-only-the-admitted-workspace
revision: 1
---
# Security review, second read: the Git changes reader (pull request 255, `8b6613866`)

Verdict: NEEDS-CHANGE, small. Round 1's four findings are closed in the code. Read only; nothing
built or run.

- "Unchanged" is decided only by the baseline tree's mode and id against a hash of the worktree
  content; every baseline size is the length of a checked read; `HEAD`, packed refs, grafts and
  the commit-graph do not reach the answer.
- Every cut sets `truncated`, and the byte-order prefix property holds.
- Remaining: one omission an index entry decided, and four sentences that claimed more than
  held. All answered in round 3 (`1de70cd8a`), which was not reviewed again.
- One finding's text is recorded here as its rule only. Its detail is with the operator until
  `story:daemon-holds-the-baseline-objects-of-a-git-workspace` is on `main`
  (`approval-record:operator-decides-how-security-review-fixes-travel-2026-11-22`).

```findings
[
{"file":"crates/substrate-host/src/git.rs","line":455,"category":"acceptance","severity":"warning","verdict":"NEEDS-CHANGE","origin":"introduced","message":"An index entry of mode 160000 removes a new file, or everything new beneath a directory, from the answer without truncated, against the text that the index only labels."},
{"file":"docs/design/21-connector-authorized-git-sources.md","line":93,"category":"contract-drift","severity":"warning","verdict":"NEEDS-CHANGE","origin":"introduced","message":"The 1 GiB hashing bound does not cover current sides, which git.rs:838-842 reads and hashes outside the budget."},
{"file":"docs/design/21-connector-authorized-git-sources.md","line":102,"category":"contract-drift","severity":"warning","verdict":"NEEDS-CHANGE","origin":"introduced","message":"The storage limit does not bound whole-file reads of the index and stored objects: a sparse file's apparent size is read and no block quota counts it."},
{"file":"docs/design/21-connector-authorized-git-sources.md","line":85,"category":"contract-drift","severity":"warning","verdict":"CONFIRMED","origin":"undecided","message":"With a program running in the workspace the stated limit says less than holds; the text is replaced by the rule that an observation then gives no guarantee of an answer."},
{"file":"crates/substrate-host/src/git/worktree.rs","line":348,"category":"boundary","severity":"note","verdict":"CONFIRMED","origin":"introduced","message":"The store's entry bound is applied after each directory is listed whole into memory, so one directory's listing is not bounded by it."}
]
```
