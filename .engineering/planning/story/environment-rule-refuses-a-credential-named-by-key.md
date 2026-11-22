---
format: aep.planning-md/3
id: story:environment-rule-refuses-a-credential-named-by-key
kind: story
status: draft
title: The environment rule refuses a credential named by KEY
summary: is_secretish_name has seven fragments and none matches ANTHROPIC_API_KEY or AWS_ACCESS_KEY_ID
tags:
- defect
- loom
revision: 1
---
# Story: the environment rule refuses a credential named by `KEY`

## Outcome

A start whose environment sets a variable named like an API key or an access key is refused
`exec.environment-refused`, as one named `…_TOKEN` or `…_SECRET` is today.

## Where the requirement comes from

Session `loom` reported it on 2026-11-21 while writing its story
`bound-program-declares-its-environment`: Substrate's credential-name fragments miss
`ANTHROPIC_API_KEY`. Loom adds its own check and asked for no change. The defect is Substrate's
own. Stakeholder: session `loom` as the reporter.

## Why

- `is_secretish_name` in `crates/substrate-guarded/src/environment.rs` refuses a name that
  contains one of seven fragments: `authorization`, `bearer`, `credential`, `password`, `proxy`,
  `secret`, `token` (read at `main` `711282f9f`).
- None of them matches `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `AWS_ACCESS_KEY_ID` or
  `SSH_PRIVATE_KEY`. Read from the source; not run for this draft.
- The module says what the rule is for: "a credential-named variable reaches no process". The
  host driver checks it on every exec start, and the adapters outside the driver check the same
  rule.
- `AGENTS.md` § *Safety envelope* lists the cleared and shaped environment in the isolation
  floor.

## What changes

1. A failing test first: a start setting `ANTHROPIC_API_KEY` is admitted today and must be
   refused.
2. The rule gains what names a key. Not decided: the fragment and its false positives. `key`
   alone also refuses `KEYBOARD_LAYOUT` and `MONKEY`; candidates are a name that ends in `_KEY`,
   `_KEYS` or `_KEY_ID`, or contains `APIKEY` or `PRIVATE_KEY`.
3. This narrows what a client may send. The refusal code exists, so no route and no schema
   changes, but a start that ran yesterday is refused tomorrow: the story states the
   compatibility in the changelog as a behaviour change, and a design note or ADR comes before
   code where invariant 8 asks for one. Whether a released bundle states the fragment list was
   not read for this draft.

## Acceptance

A start that sets `ANTHROPIC_API_KEY`, `AWS_ACCESS_KEY_ID` or `SSH_PRIVATE_KEY` is refused
`exec.environment-refused` on the host driver and on the `local` adapter, a start that sets a
name the decided rule admits runs as before, and the changelog names the change.
