---
format: aep.planning-md/3
id: review-result:design-34-u7-security-pass-1
kind: review-result
status: active
title: Security review of design 34 U7, substrate-wire 0.25.0, pass 1 (2 warnings)
relations:
- reviews: story:kubernetes-namespace-workspace-and-exec-slice
revision: 1
---
# Security review of design 34 U7, substrate-wire 0.25.0, pass 1

Reviewed `wave/k8s-namespace-1` at `b62ec96f` (U7 unit commit `3144c2c4`). Red cases: `1810731c` on
`sec/u7-bundle` (`xtask/tests/namespace_driver_bundle_security_review.rs`,
`crates/substrate-host/tests/namespace_driver_wire_security_review.rs`; test files only). Targeted
cases 47 before, 52 after, 2 red.

Verdict: NEEDS-CHANGE. No blocker, 2 warnings. The shipped vectors are correct; the checker would
accept a later vector that contradicts its own refusal table.

## Findings

```findings
- {file: xtask/src/bundle/namespace_driver.rs, line: 200, category: contract-drift, severity: warning, verdict: NEEDS-CHANGE, origin: introduced, message: "check-bundle accepts an exec.signal-unserved vector whose machine publishes the refused signal, so the row's 'outside the published exec.signals' is unenforced"}
- {file: xtask/src/bundle/namespace_driver.rs, line: 131, category: contract-drift, severity: warning, verdict: NEEDS-CHANGE, origin: introduced, message: "check-bundle accepts a driver: host machine answering a driver-unserved refusal, so 'the host publishes none of them' is unenforced"}
```

## Cleared

- The merge adds only the 370 files under `contracts/substrate-wire/0.25.0/`; `render.rs` and
  `render_v2.rs` untouched; `gate.sh` lists 0.25.0; wire crate, runtime-vector runner and TLS test
  name `0.25.0` with `853d7365…fc00`.
- Each of the three refusals has a constant and exactly one vector; nothing in `crates/` returns
  them yet.
- `driver` was already on the wire as a fixed `host`; widening it follows ADR 0047 N7 A; no code
  branches on it.
- `confinement.no-egress` is set only at `probe.rs:132`, to absent.
- 11 delegated-context vectors (8 holding a JWT) per tree, byte-identical to 0.24.0's;
  `.gitleaks.toml` unchanged; `AGENTS.md:184` names 0.17.0 through 0.25.0.
- Exactly three new refusal-metric codes; 582 samples pinned in `health.rs` and `tls_listener.rs`.

## Not verified

JWT signatures (bytes compared only), the delegated lane, metric series beyond what the
`health.rs` tests pin.

## Disposition

Both go to fix unit J: every machine in a refusal vector is `kubernetes`, the signal row's request
signal is absent from that machine's `exec.signals`, and the capsule row's machine lacks
`exec.inline-capsule`.
