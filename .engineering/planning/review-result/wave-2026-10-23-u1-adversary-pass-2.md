---
format: aep.planning-md/3
id: review-result:wave-2026-10-23-u1-adversary-pass-2
kind: review-result
status: active
title: Adversary pass 2, unit u1 (check-secrets count)
relations:
- reviews: story:check-secrets-counts-delete-only-commits
revision: 1
---
# Adversary pass 2, unit u1 (check-secrets count)

Agent type `aep:adversary`. Branch `unit/2026-10-23-u1-secrets`, working tree on `344f9c56` after correction round 1. Verdict: NEEDS-CHANGE; fixed in `d047fb20`.

```findings
- file: xtask/src/secrets.rs
  line: 212
  category: acceptance
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: a committed diff=<driver> attribute plus a textconv driver in the checking machine's git config empties the printed patch for both Gitleaks and the expectation, so a credential commit passes; adding --no-textconv to both git log invocations closes it
- file: xtask/src/secrets.rs
  line: 67
  category: boundary
  severity: note
  verdict: INFEASIBLE
  origin: undecided
  message: a local refs/replace ref shows substitute content under the original commit hash to both sides and a credential passes; CI checkout does not fetch refs/replace
- file: xtask/src/secrets.rs
  line: 230
  category: acceptance
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: a commit adding only a UTF-16 or NUL-prefixed file now passes with a credential in it where the base refused it, as acceptance line 3 requires
- file: xtask/src/secrets.rs
  line: 171
  category: boundary
  severity: note
  verdict: INFEASIBLE
  origin: introduced
  message: Git marks a diff binary when either side has a NUL, so a merge keeping text over a parent's binary version is refused as hidden text although that text was scanned on the other parent's line
```

Coordinator decisions: 1 fixed (`--no-textconv` on Gitleaks' log options and the check's own `git log`); 2 fixed (`GIT_NO_REPLACE_OBJECTS=1` on every process the check starts); 3 accepted, as acceptance line 3 asks, and pinned by `a_nul_carrying_file_is_uncounted_even_when_it_is_utf16_text`; 4 accepted as fail-closed, refusal message no longer names an attribute. No third pass. Open, local-only: `.git/info/grafts` is not neutralised by `GIT_NO_REPLACE_OBJECTS`.
