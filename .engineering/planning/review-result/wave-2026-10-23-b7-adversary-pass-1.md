---
format: aep.planning-md/3
id: review-result:wave-2026-10-23-b7-adversary-pass-1
kind: review-result
status: active
title: Adversary pass 1, unit b7 (hosted admission Identity connection pool)
relations:
- reviews: story:hosted-admission-reuses-identity-authority
revision: 1
---
# Adversary pass 1, unit b7 (hosted admission Identity connection pool)

Agent type `aep:adversary`. Branch `unit/2026-10-23-b7-identity-pool`, commit `f601062`. Cases in `crates/substrate-daemon/tests/hosted_pool_adversary_pass1.rs`, run from the built test binary. Verdict: NEEDS-CHANGE, 2 red of 8.

```findings
- file: crates/substrate-daemon/src/hosted.rs
  line: 116
  category: contract-drift
  severity: warning
  verdict: NEEDS-CHANGE
  origin: introduced
  message: The 30 s idle timeout is enforced only at checkout and checkin, so idle Identity connections stay open indefinitely while ADR 0040 says an older idle connection is closed.
- file: crates/substrate-daemon/src/hosted.rs
  line: 252
  category: contract-drift
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: The pooled-connection retry also fires after Identity received the request, so one hosted request reaches Identity as two resolutions, contradicting ADR 0040's statement that resolution load equals the hosted request rate.
```

Red output: idle case `left: 1` / `right: 0` after 31.5 s idle; retry case `left: 3` / `right: 2`. Whole binary: `FAILED. 6 passed; 2 failed`.

Held: at most 8 idle connections; no connection shared by concurrent requests (12 requests, 12 connections); a connection Identity closed while idle is replaced without 503; leftover response bytes never answer a later resolution; HTTP/1.0 keep-alive not reused, chunked reused; oversized or short bodies not reused; the 5 s deadline holds across the retry.

Coordinator decision: fix both in the implementation and keep ADR 0040's wording; close idle connections after 30 s, and retry only a request hyper hands back unsent.
