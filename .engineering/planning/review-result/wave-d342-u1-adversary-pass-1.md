---
format: aep.planning-md/3
id: review-result:wave-d342-u1-adversary-pass-1
kind: review-result
status: active
title: Adversary pass 1 of wave D342, unit U1
relations:
- reviews: story:refused-executor-handshake-names-the-api-answer
revision: 1
---
# Adversary pass 1 of wave D342, unit U1

Agent type `aep:adversary`, against the story's Acceptance only (the second bullet as amended to
401 or 403). Tree `substrate-d339-int` at `b0172f3b`, unit commit `d8541b60`. Two cases written,
both red on their first run: `client_tests::adversary_d342::a_refusal_whose_chunk_size_overflows_is_still_the_apis_403`
(client 50 passed, 1 failed) and
`namespace_lane_run::tests::a_log_last_written_before_the_case_began_is_not_kept_with_it` (xtask
462 passed, 1 failed, 9 ignored).

```findings
- file: crates/substrate-kubernetes-client/src/exec.rs
  line: 331
  category: boundary
  severity: note
  verdict: INFEASIBLE
  origin: introduced
  message: dechunk computes start + size unchecked, so a 16-hex-digit chunk size panics the exec in debug builds; no API server is shown to send one and release wraps harmlessly
- file: xtask/src/namespace_lane_run.rs
  line: 2083
  category: judgement
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: the one-second mtime slack keeps the previous back-to-back verb case's daemon log and names it in the failed case's message
```

Held: a chunk-size line split across reads, a chunk not yet whole, chunk extensions, trailers and
an unparsable size line; a `Content-Length` larger than what arrives and a server that never closes
(the 2 s wait, inside `API_TIMEOUT`); the 64 KiB cap; tungstenite's leftover bytes prepended once;
a 403 after an earlier 400 still ends the probe at once; the retry test fails if 400, 404, 408, 429
or 503 were taken as refused; the at-once test fails if the handshake retried to `CREATE_BOUND`;
the quoted phrase survives the warning and the 300-char bound; a fast-failing case's own log is
kept. Mutant the suite missed: `refused_for_good` matching 403 alone, since no test drove a 401.

Out of scope, one line each: `FakeCluster::refuse_exec` labels every status `Forbidden`, 400 and
503 included; RBAC propagation lag after `set_role` turning a 403 into a false at-once failure in
the next verb case was not checked.

Coordinator decision (D342, 2026-11-01):

- `exec.rs:331`: fixed in the wave (`cac4ef5b`): `dechunk` slices the rest after the size line and takes the
  chunk with `get(..size)`, so no sum can overflow.
- `namespace_lane_run.rs:2083`: fixed in the wave: the slack is `MTIME_SLACK`, 20 ms, the
  kernel's coarse file clock (one tick at 100 Hz is 10 ms), well under the gap between back-to-back
  verb cases.
- The 401 mutant: fixed in the wave, inside Acceptance 2:
  `an_executor_handshake_refused_401_names_the_apis_answer_at_once`.
- `refuse_exec`'s `Forbidden` label: no change; the driver reads the status code, not the reason.
- RBAC propagation lag: no change in this wave; the lane's next run at the merge commit observes
  it, and a false at-once failure would name the API's 403 and the kept daemon log.
