---
format: aep.planning-md/3
id: approval-record:wave-2026-11-04-substrate-merge-and-release
kind: approval-record
status: approved
title: The 2026-11-04 wave's pull request merges on green and releases 0.19.2
revision: 2
transitions:
- {from: "draft", to: "approved", at: "2026-11-05T01:34:22Z", actor: "human:timo", revision: 2}
---
# The 2026-11-04 wave's pull request merges on green and releases 0.19.2

Decided by the orchestrator under its grant on 2026-11-05, for pull request 212
(`wave/2026-11-04-substrate`, head `041c1a3e`) of `approval-record:wave-2026-11-04-substrate`.

## Decision

- When every required check on pull request 212 is green, the wave lead merges it through the bot
  with no further go. That includes the systemd netns floor test, which CI decides under
  `approval-record:wave-2026-11-04-substrate-gate-host-gap`.
- After the merge come the release steps:
  1. the four release lanes at the merge commit, started only with at least 60G free on `/`;
  2. the annotated tag `0.19.2` with its four lane trailers, pushed through the bot;
  3. the release verification;
  4. the announcement.
- If a check is red, the lead stops and reports the failing job and its log lines. The lead does
  not rerun it.
