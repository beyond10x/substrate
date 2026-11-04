---
format: aep.planning-md/3
id: review-result:wave-2026-11-04-substrate-u2-adversary-pass-1
kind: review-result
status: active
title: Wave 2026-11-04 substrate U2 adversary pass 1
relations:
- reviews: story:abandoned-forwarder-reaped-through-eintr
revision: 1
---
# Adversary pass 1 — wave 2026-11-04 U2 (story:abandoned-forwarder-reaped-through-eintr)

Diff `a692acb7...e4c42955` (unit commit `de299a56`). The adversary wrote no case, so it made no commit. The wave lead's guard case `a0ab2518` answers note 1. Agent `aep:adversary`. Header and findings verbatim; prose trimmed to the measured facts.

```
verdict: nothing that fails on e4c42955
cases: executed 43→43, red 0
origin: introduced 2 (notes), pre-existing 0, undecided 0
```

- The case catches the regression: with the retry removed, the case alone is red: `0 passed; 1 failed`.
- Mutant B is missed: dropping the `errno != EINTR` check, so that any error retries, leaves `egress::` green at 43 passed.
- Checked, no defect:
  - the loop cannot spin;
  - it cannot reap the wrong pid;
  - an already-reaped pid ends on ECHILD;
  - `KILL_ON_ABANDON` is `cfg(test)` only.
- The mutants ran on a `git archive` copy, not in the unit tree.

Resolution:

- Note 1 is fixed in the wave. The wave lead decided that a case could reach it directly, without a daemon reaper. `a_reap_that_fails_other_than_eintr_ends_at_once` (`a0ab2518`) reaps a child, then calls `abandon_forwarder` on its pid. The call must end within 5 s. Mutant B is red against it, and the unmutated code is green.
- Note 2 is accepted as the module's convention. Every `DownNamespace` case in `egress.rs` skips when the user namespace cannot be opened. Whether the CI runner gets that far was not measured.

```findings
[{"file":"crates/substrate-host/src/egress.rs","line":941,"category":"mutant","severity":"note","verdict":"INFEASIBLE","origin":"introduced","message":"scope in; Dropping the EINTR check (retry on any error, a busy spin on ECHILD) leaves egress:: green, 43 passed; ECHILD needs SIGCHLD ignored or a waitpid(-1) reaper in the daemon, and grep finds neither, so nothing reaches it."},
 {"file":"crates/substrate-host/src/egress.rs","line":4771,"category":"judgement","severity":"note","verdict":"INFEASIBLE","origin":"introduced","message":"scope in; The new case passes without running when DownNamespace::open fails (no unshare or no unprivileged user namespace); whether the ubuntu-latest gate runner gets that far was not measured, so CI may never exercise the acceptance."}]
```
