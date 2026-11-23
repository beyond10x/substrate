---
format: aep.planning-md/3
id: review-result:baseline-store-design-read
kind: review-result
status: active
title: The design text of the baseline store, read against the code
relations:
- reviews: story:daemon-holds-the-baseline-objects-of-a-git-workspace
prose_only: A read of design text before any code existed; its eleven findings are the numbered list and name no source line
revision: 1
---
# Review: the design text of the baseline store, read against the code

Independent read of commit `db928a20b` (design 21, § *The baseline store*) against the code of
`origin/main`, before any code was written. Read-only; nothing was run.

Eleven findings, all folded into the text in `792963047`:

1. The build for a workspace that predates the store reads objects of the copy to the size
   their header states; the text had dropped the sentence that said so. Restored, with the
   story that bounds it.
2. The copy's byte bound had no stated source at observation time. Stated: the project quota's
   limit, else 2 GiB.
3. "At most twice its byte limit" holds only under a project quota, and not while a build runs.
4. Three places where the commit-id file is not removed today. Resolved against design 23,
   which stays: a driver with no Git source configured removes nothing; the cost is stated.
5. "Workspace root" named two directories. The copy is pinned beneath the admitted workspace's
   own directory.
6. What an entry that is not a regular file does to a build, and with which code, was not said.
7. Two unconditional sentences hold only once the store exists.
8. A root name outside the root-name set must be refused by the driver before it writes or
   removes; the store moved to `.objects/<root name>` so a long name needs no longer file name.
9. A create whose root name already has an entry is refused before either is written.
10. Staging lies inside `.substrate-git-baselines`, outside the marked quota directory.
11. The refusals table, the daemon-private state and the stat-manifest decision.

Checked and found correct, among others: what the pack builder inserts, that every object it
writes is hash-checked, that the directory of all workspaces is one of the daemon's private
paths, and that none of the four refusal codes is in a released bundle.
