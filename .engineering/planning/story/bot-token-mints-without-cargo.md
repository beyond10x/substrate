---
format: aep.planning-md/3
id: story:bot-token-mints-without-cargo
kind: story
status: draft
title: The bot token is minted without starting cargo
tags:
- defect
revision: 1
---
## Outcome

`scripts/bot-token.sh` mints a b10x-bot token without starting cargo. The check of the two
credential files it runs first (`cargo xtask check-bot-files --config <file> --key <file>`,
`xtask/src/bot_files.rs`: each file is there, is a regular file and no symbolic link, belongs
to the effective user and is readable; the configuration is not writable by group or other,
mask `022`, and the key grants nothing to group or other, mask `077`) is done by the script
itself in POSIX sh, with the same five refusals and the same messages.

## Why

The workstation's build pool sends `cargo run` and its aliases, `cargo xtask` among them, to a
build node (slot 0.33.0, 2026-11-24). The two credential files exist on the workstation only,
so the check fails there with "b10x-bot credential input is unavailable" and no token is
minted: `scripts/as-bot.sh` and `scripts/bot-gh.sh` cannot be used. A script that mints a
credential must not depend on where a build runs, and must not need a compile.

Until this story is done, bot writes in this repository go through `b10x-gates bot --`,
`b10x-gates gh --` and `b10x-gates api` (`AGENTS.md` § Bot identity).

## Acceptance

- `scripts/bot-token.sh` calls no `cargo`. With `cargo` absent from `PATH` it reaches the
  same answer.
- Each refusal of `check-bot-files` has a case that runs the script against a throwaway
  directory and a stand-in for the forge: a missing file, a directory, a symbolic link, a file
  of another owner where the test can make one, a configuration writable by group or other,
  a key with any group or other bit, an unreadable file, and the admitted modes. Each refusal
  ends non-zero before any request and before the key is read.
- The cases are Rust, in the `xtask` package beside `xtask/tests/bot_scripts.rs`; no Python.
- `check-bot-files` stays as a verb or is removed with its tests; the change says which and
  why.
- `AGENTS.md` § Bot identity names the scripts as usable again.

## Other repositories with the same call

`llmgw` and `zwirn` carry the same `cargo xtask check-bot-files` call in their
`scripts/bot-token.sh` (read on 2026-11-24). Each needs the same change in its own repository;
this story covers substrate only.

## Where the requirement comes from

The operator's session, 2026-11-24, after this repository's session reported that the mint
fails under the build pool's routing and that `scripts/bot-gh.sh` then ran `gh` without a bot
token. The second half is fixed apart from this story: both scripts now end non-zero, and run
no program, where the mint fails or prints no token (`xtask/tests/bot_scripts.rs`).
Stakeholder: the operator's session.

## Scope

`scripts/bot-token.sh`, `xtask/src` (the `check-bot-files` verb), `xtask/tests/`, `AGENTS.md`.
