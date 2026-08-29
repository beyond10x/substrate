---
format: aep.planning-md/1
id: story:agents-md-matches-the-scripts
kind: story
status: implemented
title: AGENTS.md says what the gate and the bot script actually do
summary: 'Two verified drifts: AGENTS.md:112 says gate.sh verifies 0.1.0 only (gate.sh:20-23 runs all four checkers); AGENTS.md:172 says bot-token.sh:8 defaults to a foreign org (it defaults to beyond10x).'
owner: substrate
tags:
- docs
relations:
- decomposes: epic:release-hardening
revision: 6
---
# Story: AGENTS.md says what the gate and the bot script actually do

## Outcome

A contributor who reads `AGENTS.md` § *The gate* and § *Bot identity* gets facts that match the
scripts they describe. A false working agreement teaches the next agent to distrust the whole file.

## Context

Two claims are verified false on 2026-08-29:

| `AGENTS.md` says | the script says |
|---|---|
| `AGENTS.md:112-114` — "`scripts/gate.sh` verifies the 0.1.0 bundle only"; the 0.2.0/0.3.0/0.4.0 checkers "are **not** run by the gate" | `scripts/gate.sh:20-23` runs `check-contract-bundle.py`, `-0.2.0.py`, `-0.3.0.py`, `-0.4.0.py` |
| `AGENTS.md:172-173` — the bot-org default at `scripts/bot-token.sh:8` "is not the org this repository lives in" | `scripts/bot-token.sh:8` — `org="${B10X_BOT_ORG:-beyond10x}"`, this repository's org |

## Acceptance

`AGENTS.md` § *The gate* and § *Bot identity* contain no statement that contradicts
`scripts/gate.sh` or `scripts/bot-token.sh`, and `README.md` § *Build, test, run* lists the gate's
steps in the gate's order.

Evidence that satisfies it:

- the "verifies the 0.1.0 bundle only" paragraph is replaced by one that names all four checkers
  and keeps the rule that a new successor bundle's checker joins `scripts/gate.sh`;
- the `bot-token.sh` paragraph states the default is `beyond10x`;
- every `file:line` reference remaining in those sections resolves;
- `bash scripts/gate.sh` exits 0 (`check-links.py` covers the edited links).

## Out of Scope

The `rustup update` paragraph (`AGENTS.md:116-119`) is true today and is retired by
`story:pinned-rust-toolchain`, not here. No behaviour, bundle byte or route changes.

## Implemented — 2026-08-29

- `AGENTS.md` § *The gate*: step list matches `scripts/gate.sh:15-26`; the "0.1.0 bundle only"
  paragraph is gone; the `rustup update` paragraph is replaced by the `rust-toolchain.toml` pin,
  the one-commit bump rule and `scripts/check-toolchain.py`; the `tail`-exit-status sentence kept.
- `AGENTS.md` § *Bot identity*: the default at `scripts/bot-token.sh:8` is stated as this
  repository's org, quoting `org="${B10X_BOT_ORG:-beyond10x}"`.
- `README.md` § *Build, test, run*: table in gate order, four bundle rows split out, the dead
  `check-brand.sh` row removed (the script does not exist), `bundle packager` and `toolchain`
  rows added.
- `grep -n 'verifies the 0.1.0 bundle only\|is not the org this repository lives in\|rustup
  update' AGENTS.md` → no hits. `python3 scripts/check-links.py` → exit 0.
- Full gate: see the evidence record on this story.
