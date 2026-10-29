---
format: aep.planning-md/3
id: review-result:design-34-u9-security-pass-1
kind: review-result
status: active
title: Security review of design 34 U9, the exec path, pass 1 (1 warning, 2 notes)
relations:
- reviews: story:kubernetes-namespace-workspace-and-exec-slice
revision: 1
---
# Security review of design 34 U9, the exec path, pass 1

Reviewed `wave/k8s-namespace-1` at `a84bb76c` (U9 unit commit `e51d4442`, wave merged in at
`e5f97c7f`). Red case: `02923560` on `sec/u9-exec-path` (`crates/substrate-kubernetes/src/exec_tests.rs`,
+54, test file only). Crate suite 58 before, 59 after, 1 red.

Verdict: NEEDS-CHANGE. No blocker, 1 warning, 2 notes.

## Findings

```findings
- {file: crates/substrate-kubernetes/src/exec.rs, line: 527, category: integrity, severity: warning, verdict: NEEDS-CHANGE, origin: introduced, message: "finish reads pods/log by name only, so after a same-named replacement the exec reports the replacement's frames as its stdout beside our uid-proven exit"}
- {file: crates/substrate-kubernetes/src/exec.rs, line: 483, category: judgement, severity: note, verdict: CONFIRMED, origin: introduced, message: "the gone-Pod replay quotes ERROR events through watch_event without bearer redaction and logs them, unlike the client's own replay"}
- {file: crates/substrate-kubernetes/src/exec.rs, line: 567, category: contract-drift, severity: note, verdict: CONFIRMED, origin: introduced, message: "a gone or unreadable log still commits Exited with the API exit code, so a lost limits-unapplied refusal reads as a real exit 125"}
```

## Red case

- `exec_tests::output_is_never_read_from_a_same_named_replacement`: the exec reports the
  replacement's log as its stdout.

## Cleared

- An unbound reserved row precedes the POST; a lost answer is adopted by annotation; a stale
  binding sends nothing.
- Pod golden: image by digest, no env, `enableServiceLinks: false`, no token, restricted security
  contexts, requests equal limits, deadline, `restartPolicy: Never`, node pin.
- A frame the child prints is output only; a non-frame line or truncated log is a non-retriable
  protocol error.
- Deletes carry uid preconditions; `int` is `exec.signal-unserved`; a force delete gives unknown
  with no exit; a 410 relists and re-reads by name and uid.
- No fact names the driver; `confinement.no-egress` is not claimed.

## Not verified

Anything needing kind (RuntimeDefault and nested user namespaces, O3, O11), mutating admission
webhooks, the executor's drain-deadline path.

## Disposition

All three go to fix unit M: the log is read only between two GETs that show the bound uid, else
the output is unknown; the replay redacts the bearer token as the client's does; a gone or
unreadable log leaves the exec unknown with no exit (design 34 § 7.3).
