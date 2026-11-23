---
format: aep.planning-md/3
id: review-result:file-change-ledger-security-pass-2
kind: review-result
status: active
title: Security review of the ledger bound on a file change, pass 2
relations:
- reviews: story:file-change-result-fits-the-ledger-before-the-file-changes
revision: 1
---
# Security review, pass 2: a file change fits the ledger (pull request 270, `c5021adea`)

Review of the second form of `story:file-change-result-fits-the-ledger-before-the-file-changes`,
one commit on `main` at `b247fd45d`, limited to what the redesign introduced. Verdict:
CONFIRMED, with one warning for behaviour that is on `main` as well. Nothing was found in the
redesigned bound itself.

Counts as reported: cases executed 24 to 25 in the daemon's `http` target, 1 red, the one case
the reviewer added. The wire and guarded suites exited 0. Three findings: one warning, two
notes. Origin: 1 introduced, 2 pre-existing.

- F1, warning, pre-existing. A replacement whose write fails after its parent directories were
  created does not yet remove them, and one changelog sentence was wider than the code. The
  changelog of release 0.25.0 names the refusals that create no directory: text bounds, digest,
  content, size and precondition. The rest is the draft
  `story:failed-file-replacement-removes-the-directories-it-created`.
- F2, note, infeasible, pre-existing. The daemon does not yet hold a driver's answer to the
  workspace and path of its request. Not acted on: no shipped driver or executor was found to
  answer otherwise.
- F3, note, introduced. `AGENTS.md` stated 59 bound constants of the wire crate where the file
  has 60. Corrected in the release-prepare commit.

Checked and not faulted: the widest result as an upper bound of every fixed part, refused
before admission and recorded as refused; the cut, which lands on character boundaries and
leaves the result within the bound whenever its fixed part is; the largest answer a driver can
return, under the 16 MiB header limit and no larger than on `main`; the order in `replace_cas`;
the content bound as exactly the published `oneOf`; the executor's source, the namespace
driver's file calls and the `Driver` trait identical to `origin/main`.

Not checked by the reviewer: a quota or full-disk run; a driver that does not echo its request;
the namespace and delegated lanes; the store's accounting beyond reading.

```findings
[
{"file":"crates/substrate-guarded/src/fs.rs","line":443,"category":"contract-drift","severity":"warning","verdict":"CONFIRMED","origin":"pre-existing","message":"A replacement whose write fails after its parent directories were created does not yet remove them, and the changelog sentence was wider than the code. The entry of 0.25.0 names the refusals that create no directory; draft story failed-file-replacement-removes-the-directories-it-created."},
{"file":"crates/substrate-daemon/src/app/workspaces.rs","line":1145,"category":"integrity","severity":"note","verdict":"INFEASIBLE","origin":"pre-existing","message":"The daemon does not yet hold a driver's answer to the workspace and path of its request; no shipped driver or executor was found to answer otherwise."},
{"file":"AGENTS.md","line":416,"category":"contract-drift","severity":"note","verdict":"CONFIRMED","origin":"introduced","message":"The stated count of the wire crate's bound constants was 59 and the file has 60. Corrected in the release-prepare commit."}
]
```
