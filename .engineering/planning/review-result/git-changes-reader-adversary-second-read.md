---
format: aep.planning-md/3
id: review-result:git-changes-reader-adversary-second-read
kind: review-result
status: active
title: 'Adversary pass, second read: the Git changes reader'
relations:
- reviews: story:git-changes-reader-reads-only-the-admitted-workspace
revision: 1
---
# Adversary pass, second read: the Git changes reader (pull request 255, `8b6613866`)

Verdict: no blocker; one warning and four notes. Read only; nothing built or run.

- All thirteen points of the changelog's list of differences match the code.
- No wrong answer found in the label logic, the hashing or the budget, apart from the findings.
- Cost for N tracked files of which M changed: every observation lists the object store, reads
  one commit and every tree object, and reads and hashes all N tracked files; no baseline blob
  is read for an unchanged file.
- Four proposed cases; all joined the suite in round 3 (`1de70cd8a`), which answered every
  finding. Round 3 was not reviewed again.

```findings
[
{"file":"crates/substrate-host/src/git/worktree.rs","line":179,"category":"boundary","severity":"warning","verdict":"CONFIRMED","origin":"introduced","message":"One tracked file larger than the remaining budget, or one false header size at git.rs:806, ends the answer at that path, so the writer chooses where every observation stops and the caller cannot continue."},
{"file":"crates/substrate-host/src/git.rs","line":1454,"category":"mutant","severity":"note","verdict":"CONFIRMED","origin":"introduced","message":"The case's only assertion sits behind an Ok that the store walk now prevents, so the rule on baseline sizes has no case that can fail."},
{"file":"crates/substrate-host/src/git/worktree.rs","line":32,"category":"contract-drift","severity":"note","verdict":"CONFIRMED","origin":"introduced","message":"An object store over 262,144 entries or nested deeper than 8 is refused; design 21 says so and the changelog's list of differences does not."},
{"file":"crates/substrate-host/src/git.rs","line":455,"category":"contract-drift","severity":"note","verdict":"CONFIRMED","origin":"undecided","message":"An index gitlink entry decides that a directory is not entered, against the text that the index only labels a path and that repository metadata chooses no directory."},
{"file":"crates/substrate-host/src/git/worktree.rs","line":377,"category":"mutant","severity":"note","verdict":"CONFIRMED","origin":"introduced","message":"No committed case fails when the line the ignore bound cuts is kept as a rule, and none reaches a baseline object over budget at git.rs:812."}
]
```
