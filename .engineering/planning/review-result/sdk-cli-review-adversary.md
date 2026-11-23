---
format: aep.planning-md/3
id: review-result:sdk-cli-review-adversary
kind: review-result
status: active
title: Adversary pass on the CLI and SDK fixes of the 0.24.0 review
relations:
- reviews: story:cli-destroy-retry-replays-the-stored-outcome
revision: 1
---
# Adversary pass: the SDK and CLI review fixes (pull request 269, `554209df0`)

Verdict: NEEDS-CHANGE. Of the four claims of branch `fix/sdk-cli-review-0-24-0`, the destroy by
id did not hold. The recovery of a lost Git create reply, the last source selection and the
working directory on both builders held against the shipped daemon. Counts as reported: cases
executed 131 to 142, 3 red; all 131 earlier cases passed. Four findings: one blocker, one
warning, two notes. Origin: 2 introduced, 2 pre-existing.

- F1, blocker, introduced. The destroy by id put the caller's word into the request path through
  the encoder for file paths, which keeps the slash, so the word could name another route.
  Fixed on the branch before the release, in the commit "sdk: an identifier in a request path
  is one path segment". The rule: an identifier is one path segment, and a word outside the
  rule is refused before a request is sent.
- F2, warning, pre-existing, not changed in this unit. A retry under an operation id answers
  the stored outcome without a prior read for `workspace destroy` only. `workspace renew`,
  `exec run` and `session signal` still read their resource first. The changelog entry was
  scoped to say so.
- F3, note, introduced, fixed with F1. Three texts promised `exec.workdir-unserved` for a
  session start without the `exec.workdir` fact; the unit's own case asserts
  `session.confinement-unavailable` there.
- F4, note, pre-existing. The CLI's JSON form of `unavailable` carries only the reason, so an
  unknown operation prints no operation id unless the caller passed one. The fix commit names
  no change for it.

Attacked and held: a lost destroy reply recovered with one request and one ledger read; a
replay that is byte-identical; one operation id with another workspace is a conflict; an
adopted workspace keeps its files on the host; a stored refusal surfaces field for field; the
source authority is absent from the ledger read and from the error's text; 16 refused working
directories with zero requests sent; no changed public signature. Not tried: the delegated
lane; recovery over the remote transport; a Git create against a real source; another
subject's workspace; mutants of the implementation.

```findings
[
{"file":"crates/b10x-substrate-sdk/src/lib.rs","line":269,"category":"boundary","severity":"blocker","verdict":"NEEDS-CHANGE","origin":"introduced","message":"The destroy by id put the caller's word into the request path through the encoder for file paths, which keeps the slash, so the word could name another route. Rule: an identifier in a request path is one path segment; a word outside the rule is refused before a request is sent."},
{"file":"crates/b10x-substrate-cli/src/lib.rs","line":541,"category":"acceptance","severity":"warning","verdict":"CONFIRMED","origin":"pre-existing","message":"A retry under an operation id answers the stored outcome without a prior read for workspace destroy only; workspace renew, exec run and session signal still read their resource first. The changelog entry says so."},
{"file":"crates/b10x-substrate-sdk/src/lib.rs","line":1413,"category":"contract-drift","severity":"note","verdict":"CONFIRMED","origin":"introduced","message":"The session builder's doc, the SDK guide and the changelog promised exec.workdir-unserved for a session start without the fact, while the unit's own case asserts session.confinement-unavailable. The texts were corrected."},
{"file":"crates/b10x-substrate-cli/src/lib.rs","line":757,"category":"judgement","severity":"note","verdict":"CONFIRMED","origin":"pre-existing","message":"The JSON form of unavailable carries only the reason, so an unknown operation prints no operation id unless the caller passed one."}
]
```
