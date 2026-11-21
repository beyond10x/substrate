---
format: aep.planning-md/3
id: story:release-notes-start-with-the-short-form
kind: story
status: draft
title: A release's GitHub notes start with the short form
summary: release.yml puts a bounded short form above the digests and lane records it writes
tags:
- release
revision: 1
---
# Story: a release's GitHub notes start with the short form

## Outcome

The body of a GitHub release starts with a short form a person reads in one screen, and the
artifact digests, verification commands and lane records `release.yml` writes today follow it.

## Where the requirement comes from

The operator session ordered release 0.23.0 on 2026-11-21 with notes in a short form and decided,
the same day, that the body stays as `release.yml` writes it for that release, that the short
form goes into the annotated tag's message, and that this draft is filed. Stakeholder: the
operator session `host-operator`.

## The short form

- A lead of one sentence, at most 25 words.
- `**Breaking**`, with `- None.` when there is none.
- `**Changed**`, at most 5 bullets of one clause each.
- A last line linking the changelog entry.
- At most 900 characters without the link line.

## Why

- `release.yml`'s step *Compose release notes* writes `release-notes.md` from a fixed template
  and passes it to `gh release create --notes-file`. It takes no free text.
- A `gh release edit` after publication is not in `AGENTS.md` § *Releases*, and the body would
  then differ from what the workflow verified and wrote.

## What changes

1. The workflow reads the short form from one stated source at the tagged commit. Candidates, not
   decided: the annotated tag's message above its lane trailers, or a block of the version's
   `CHANGELOG.md` section.
2. It refuses a short form over the bounds above, before any artifact is published, and writes
   it as the first block of the body.
3. A recovery dispatch of a tag without a short form publishes the body as today and says so.
4. `xtask/tests/release_workflow.rs` executes the refusals offline, as it does for the lane
   records.

## Acceptance

A release cut from a tag carrying a short form has that text as the first block of its GitHub
release body, unchanged, above the digests and lane records; a short form of 901 characters or 6
bullets is refused with nothing published.
