---
format: aep.planning-md/3
id: review-result:wave-2026-10-23-b3-adversary-pass-1
kind: review-result
status: active
title: Adversary pass 1, unit b3 (session start publishes exec fields)
relations:
- reviews: story:session-start-publishes-exec-fields
revision: 1
---
# Adversary pass 1, unit b3 (session start publishes exec fields)

Agent type `aep:adversary`. Branch `unit/2026-10-23-b3-session-exec`, commit `48c3302`. Cases in `xtask/tests/session_exec_fields_adversary_pass1.rs`. Verdict: NEEDS-CHANGE, 5 red of 6.

```findings
- file: xtask/src/bundle/session_exec_fields.rs
  line: 47
  category: mutant
  severity: warning
  verdict: NEEDS-CHANGE
  origin: introduced
  message: check_members compares only properties, so a session exec whose required list or additionalProperties drifts from exec start still passes check-bundle 0.23.0 (three mutants).
- file: crates/substrate-wire/src/lib.rs
  line: 837
  category: contract-drift
  severity: note
  verdict: CONFIRMED
  origin: pre-existing
  message: the daemon admits scratch null, scratch max_bytes not a multiple of 1024 and duplicate measurements, which the published session and exec start schemas refuse.
- file: crates/substrate-daemon/tests/runtime_vectors.rs
  category: acceptance
  severity: warning
  verdict: NEEDS-CHANGE
  origin: pre-existing
  message: the session vector expects exec.scratch-quota-unserved for scratch, but no file of bundle 0.23.0 publishes that code, so acceptance 1 holds for scratch by neither branch.
- file: xtask/src/bundle.rs
  line: 896
  category: judgement
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: design 27 claims a future exec-start field left off the session schema fails check-bundle, but session_exec_fields runs only for 0.23.0.
```

Red output: three mutants `contract bundle 0.23.0 verified: 361 files`; `exec.scratch = null: schema refuses, daemon admits`, `max_bytes 1048577 … daemon admits`, `duplicate measurements … daemon admits`; `no file of contracts/substrate-wire/0.23.0 names exec.scratch-quota-unserved`.

Held: both routes run start_inner, validate_exec_input and driver_failure; each delegated vector fails if the daemon drops its field; 16 members on each side match ExecStartInput, wait is const false; frozen bundles 0.1.0 to 0.22.0 unchanged; SHA pin equals sha256 of 0.23.0 bundle.json; the SDK session builder carries all three fields.

Coordinator decision: fix all four. The class check compares required and additionalProperties and runs for every bundle from 0.23.0 on; 0.23.0 publishes the refusal codes the three fields produce, on both routes (design 27 amended first); the daemon refuses the three values the schema refuses, unless an in-collection client is shown to send them.
