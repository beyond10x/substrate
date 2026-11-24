---
format: aep.planning-md/3
id: review-result:bot-token-check-security
kind: review-result
status: active
title: 'Security review: bot-token.sh checks its credential files itself'
relations:
- reviews: story:bot-token-mints-without-cargo
revision: 1
---
# Security review: `bot-token.sh` checks its credential files itself

2026-11-24, on the commit `30e895583` of the branch `feat/bot-token-mints-without-cargo`. The
reviewer added 21 cases in `xtask/tests/bot_scripts.rs` and did not edit the script. Verdict:
one sentence pair of `AGENTS.md` to correct; three cases red on inputs nothing found reaches.
`bot_scripts` cases executed 23 to 44, red 3. S1: 0. S2: 0. S3: 5.

## Held

- The five refusals and their messages are those of the former `cargo xtask check-bot-files`:
  a missing file, a directory, a FIFO, a socket, a link, a dangling link, another owner, each
  group and other bit of the key (0620, 0610, 0604, 0602, 040 with two lines), the
  configuration's bits, an unreadable file. A path with a leading `-`, a directory named
  `-print`, a newline, a space or a glob character, a trailing `/`, a linked directory
  component and one file named as both inputs give the answer the former check gave.
- Every refusal ends the script before `jq`, `openssl` or `curl` starts. A FIFO, a socket and
  a device are refused before the open; the open reads no byte.
- A refusal prints fixed text: no path and no byte of either file.
- `id` absent, printing nothing or printing no number refuses; `find` absent refuses.
- `scripts/bot-gh.sh` and `scripts/as-bot.sh` run no program where the mint refuses.
- Nothing in the tree calls `check-bot-files`.
- Every case runs with a cleared environment in its own temporary directory, with stand-ins
  for `jq`, `openssl` and `curl`, and fails against a script without the check.

Not established: bash 3.2 and BSD `find` on macOS (not run); a file another user owns (needs
privilege); the build node's versions of `find`, `id` and `bash`.

## Findings and what was done

1. S3. Rule asked: a refusal prints the refusals alone, also where `id` is not on `PATH`.
   Done: the effective uid is read once, with the tool's own error text discarded, and no
   number refuses.
2. S3. Rule asked: a `find` test holds only where `find` printed the file and ended with
   status 0. Done at both tests.
3. S3. Rule asked: the check, `jq` and `openssl` take the same string, and each takes it as a
   path. Done: a relative path gets its `./` prefix once, before the check, and that string is
   the one handed on.
4. S3. `AGENTS.md` § Bot identity said one line per failed file and a `PATH` without `cargo`.
   Done: one line per failed check; the cases' `cargo` is a stand-in that records a call.
5. S3, accepted. The check and the reads by `jq` and `openssl` resolve the path separately,
   and no parent directory is examined. A second process of the same user can already read
   the key; another user needs write access to a directory of the path, which the default
   location does not grant, and could make the mint fail or hang, not disclose the key. The
   former check had the same window.

Runs after the changes, release profile, in the build pool: `bot_scripts` 44 of 44.
