---
format: aep.planning-md/3
id: story:committed-text-cites-the-repository-owner
kind: story
status: draft
title: Committed text cites the repository owner
summary: 119 lines in 36 files on main at e7faf3b1d6; three questions for the stakeholder before any rewording
tags:
- documentation
revision: 1
---
# Story: committed text cites the repository owner

## Outcome

Text in this public repository attributes a decision to the repository owner or to the
stakeholder of the work. It names no tooling of the owner's workstation and no instruction file
of that machine. A check in the gate holds the rule for new text.

## Where the requirement comes from

Pull request 301 was not merged on 2026-11-25 because two added lines and one commit body
attributed the owner's decision to tooling of his workstation. It was replaced by pull request
302 with the wording "the repository owner's pick (2026-11-25)". The same day the owner's side
asked for the text already on `main` to be reworded in a later pull request. Stakeholder: the
repository owner.

## Measured

On `main` at `e7faf3b1d6`, 119 lines in 36 files hold such a wording, counted with one pattern
of two alternatives:

- `AGENTS.md`: 8 lines, in § *Bot identity* and § *Planning artifacts*. Two paragraphs of
  § *Planning artifacts* state who decides for the owner and hold the wording.
- `adr/0053-…`: 2 lines. `docs/design/37-executable-lists-on-the-wire.md`: 3 lines.
- `website/ux/rounds/R-02/decision.md`: 1 line.
- `.engineering/planning/approval-record/`: 18 records, 86 lines. Five record ids hold the
  wording, and `AGENTS.md` cites one of them by id.
- `.engineering/planning/story/`: 12 stories, 16 lines.
- `.engineering/planning/review-result/`: 2 records, 3 lines.

Commit bodies on `main` hold the wording too. They are not changed: that needs a force-push,
which nobody does.

## Not known

- Whether the rule covers the two paragraphs of `AGENTS.md` § *Planning artifacts* that say who
  decides for the owner. They are rules for whoever works in this repository. The stakeholder
  decides: reword them here, or take them out of this repository.
- Whether a record id that holds the wording is renamed. An id is cited by other artifacts and
  by `AGENTS.md`; `aep` has no rename verb that was read for this story.
- Whether `approval-record` and `review-result` bodies are changed with
  `aep plan artifact redact`, which records each replacement, or left as history.

## What changes

1. The stakeholder answers the three questions above.
2. Prose outside the planning store (`AGENTS.md`, ADR 0053, design 37, the UX round) is reworded
   to "the repository owner" with the date of the decision. No decision, date or quoted word of
   the owner changes.
3. Story bodies are rewritten with `aep plan artifact body`. Records are handled as question 3
   is answered.
4. A check refuses new text of that wording: a `cargo xtask` verb in the gate's static phase, or
   a case of an existing one, with a case for both sides. Its pattern is the only place the
   wording stays.

## Acceptance

The count of step *Measured*, taken again with the same pattern, is 0 outside the files the
stakeholder's answers leave unchanged and outside the check's own pattern, and the check is red
on a fixture line of that wording and green on the reworded line.
