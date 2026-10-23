---
format: aep.planning-md/3
id: review-result:wave-2026-10-23-u3-security-pass-1
kind: review-result
status: active
title: Security review pass 1, unit u3 (bundle 0.23.0 read-only roots)
relations:
- reviews: story:bundle-publishes-read-only-roots
revision: 1
---
# Security review pass 1, unit u3 (bundle 0.23.0, read-only roots)

Agent type `aep:security-reviewer`. Branch `unit/2026-10-23-u3-ro-roots-bundle`, working tree on `344f9c56`. Case in `crates/substrate-daemon/tests/read_only_roots_fact_security.rs`. Verdict: NEEDS-CHANGE (warning, no blocker).

```findings
- file: crates/substrate-daemon/src/app/operations.rs
  line: 459
  category: contract-drift
  severity: warning
  verdict: NEEDS-CHANGE
  origin: introduced
  message: 0.23.0 publishes read_only_roots as requiring exec.read-only-roots but the daemon dispatches a root-naming start to a driver whose snapshot omits the fact (202, driver reached), so no-root-unmounted holds only by the HostDriver probe and admission agreeing by construction, with no published unserved refusal.
```

Checked and not faulted: the fact is absent on the portable lane and gated on the exec floor and openat2; schema `maxItems` and fact `const` equal `MAX_READ_ONLY_ROOTS`; applied roots are built after the spawn barrier; `read_only_roots.rs` unchanged (openat2 `RESOLVE_NO_SYMLINKS|NO_MAGICLINKS`, `--ro-bind-fd`); no `/home/<name>/` path or new JWT in the bundle; no frozen bundle byte changed.

Coordinator decision: the daemon checks the fact and refuses with the existing 501 `exec.sandbox-unavailable`; no new refusal code (a dedicated `-unserved` code needs a design note beyond ADR 0010).
