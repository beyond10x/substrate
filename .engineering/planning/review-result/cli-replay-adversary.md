---
format: aep.planning-md/3
id: review-result:cli-replay-adversary
kind: review-result
status: active
title: Adversary pass on the CLI commands that send their mutation by id
relations:
- reviews: story:commands-that-read-before-a-mutation-replay-their-stored-outcome
revision: 1
---
# Adversary pass: the CLI commands that send their mutation by id

2026-11-23 to 24, on the uncommitted change of
`story:commands-that-read-before-a-mutation-replay-their-stored-outcome`, base `f320866be`. The
adversary added four cases to `crates/b10x-substrate-cli/tests/scenarios.rs` and did not edit
the implementation. Cases executed 13 to 17, one red.

## Held

- Renew and signal refuse an empty id, `.`, `..`, an id with `/ ? # %` and one with a control
  character before anything is sent (`id_segment`).
- An id of another kind and a resource that does not exist are a recorded `resource.not-found`;
  nothing is dispatched.
- One operation id with another workspace, TTL, signal, session or argv answers
  `operation.request-conflict`; the leases stay as they were and the first request still
  replays.
- Without `--operation-id` each invocation takes a fresh id.
- The SDK's handle methods delegate to the new calls and replace their observed state only on
  success; no public item changed.
- A replayed `exec run` keeps exit status 7, both truncation flags and a timeout, in the JSON
  and the human form.
- The lane's two `executed` lines are printed after each case's last assertion.
- With the read put back, the renew and signal cases fail.

## Findings and what was done

1. A retried `exec run` after a daemon restart is refused `operation.request-conflict`: the
   start request names the capability snapshot, and a daemon start publishes another. The same
   holds with the read put back. Done here: a `limit_` case holds the refusal and the way to the
   stored outcome, and the changelog and the SDK's doc comment name the limit. The question of
   the request's identity is
   `story:a-retried-exec-start-replays-across-a-daemon-restart`.
2. The changelog and the doc comment named one limit of the replay. Done: both name two.
3. The change's own `exec run` case asserts nothing on a host without confined exec. Done: the
   adversary's case for a workspace that never existed is kept; it fails with the read put back.
4. The first-run answers of `exec run` for an id that names nothing were not stated, and the new
   SDK call applied no local id rule. Done: the call applies `id_segment`, with a case, and the
   changelog states the recorded refusal.
5. The story said the MCP adapter's tools are listed, and the change held no list. Done:
   `story:mcp-tools-that-read-before-a-mutation-replay-their-stored-outcome`.

## Not established

An exec still running when the retry arrives (by reading: `operation.outcome-unknown`), and ids
with whitespace or of great length at run time.
