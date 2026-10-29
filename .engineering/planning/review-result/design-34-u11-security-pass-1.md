---
format: aep.planning-md/3
id: review-result:design-34-u11-security-pass-1
kind: review-result
status: active
title: Security review of design 34 U11, restart reconciliation, pass 1 (1 warning)
relations:
- reviews: story:kubernetes-namespace-workspace-and-exec-slice
revision: 1
---
# Security review of design 34 U11, restart reconciliation, pass 1

Reviewed `wave/k8s-namespace-1` at `32f4d3c1` (U11 unit commit `55be7229`, diff
`4f50de0c..55be7229`). Red case: `252c7bda` on `sec/u11-restart`
(`crates/substrate-kubernetes/src/restart_tests.rs`, +34, test file only). Crate suite 74 before, 75
after, 1 red.

Verdict: NEEDS-CHANGE. No blocker, 1 warning.

## Findings

```findings
- {file: crates/substrate-kubernetes/src/exec.rs, line: 514, category: integrity, severity: warning, verdict: NEEDS-CHANGE, origin: introduced, message: "exec_recover seeds the tracked observation from the restart-recorded exec, whose output_complete is true, so a protocol-failure or not-ours log across a restart is stored as exited with empty, complete output"}
```

## Red case

- `restart_tests::a_protocol_failure_across_a_restart_is_not_whole_output`: a recovered exec whose
  log is a protocol failure reports its empty output as whole.

## Cleared

- Every delete is uid-preconditioned; a 409 leaves the replacement Pod alone; a collision is counted,
  never deleted.
- A process not open or not holding the Lease touches nothing.
- A running or terminating Pod is deleted with 1 s grace and reported unknown with no exit, and
  deleted again in later rounds until it reads absent.
- The N9 check reads the injected wall clock.
- The default `recover_exec` is `observe_exec`; host behaviour is unchanged.
- A row settles only when its object reads absent; the daemon takes unknown to exited only through
  `put_exec`, which refuses to overwrite a terminal state.

## Not verified

A joined daemon-and-driver restart on the fake (the two halves are tested apart); a recovery
cancelled by the maintenance timeout; the kind lane.

## Disposition

Fix unit O: the seeded observation clears `output_complete`, `stdout`, `stderr` and their
truncation flags before `finish`, so only a log read under the bound uid can make output whole.
