---
format: aep.planning-md/3
id: approval-record:operator-decides-how-security-review-fixes-travel-2026-11-22
kind: approval-record
status: draft
title: Operator decides how the fixes of a private security review travel (2026-11-22)
revision: 1
---
# Operator decides how the fixes of a private security review travel (2026-11-22)

## Who decided

The operator, on 2026-11-22 at 03:00 CET. His pick reached this repository's session as a relay by
session `host-operator`; it was not typed in this session. What he typed in this session, before
the question, was the order to handle the review's findings.

## What was asked

The operator handed this session a private security review of `main` at `1af046902`, which is
also the content of release 0.23.0. The repository is public, and `SECURITY.md` says an unresolved
vulnerability is not disclosed in a public issue, discussion or pull request. A story in the
planning store and a pull request are public when pushed. Put to session `host-operator` as a
decision without a default, because it is a publication:

- A: stories and pull requests stay in the public repository, each worded as the behaviour that
  must hold, with no steps that show how to use a defect. The detail stays in the private file
  until the fix is on `main`. The changelog's Security entry names each defect at the release.
- B: one GitHub private security advisory per high-priority finding, fixed in its temporary
  private fork, where no workflow runs before the merge.
- C: full detail in public stories at once.

## The words, as relayed

> host-operator relays Timo's pick, 2026-11-22 03:00 CET, to D5 "how do the fixes for the 11
> findings of his private security review travel in the public repository?": "A: public, worded
> as behaviour". Stories and PRs state the rule that must hold, no steps that show how to use a
> defect; the detail stays in the private file until the fix is on main; the changelog's Security
> entry names each at the release. A release after the fixes comes to him as its own question.

## What follows

- Each finding that holds against the source gets a story whose title and body state the rule.
  The story names the review by its date and the finding by its number, and nothing more of it.
- A fix's commit message, pull request and test names follow the same rule until the fix is on
  `main`. After that the story's "As built" section may state what was wrong.
- A finding that does not hold gets a record that says so, by number.
- A release that carries the fixes is a separate question to the operator.
