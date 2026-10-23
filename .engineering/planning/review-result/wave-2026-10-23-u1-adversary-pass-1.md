---
format: aep.planning-md/3
id: review-result:wave-2026-10-23-u1-adversary-pass-1
kind: review-result
status: active
title: Adversary pass 1, unit u1 (check-secrets count)
relations:
- reviews: story:check-secrets-counts-delete-only-commits
revision: 1
---
# Adversary pass 1, unit u1 (check-secrets count)

Agent type `aep:adversary`. Branch `unit/2026-10-23-u1-secrets`, working tree on `344f9c56`. Cases in the test module of `xtask/src/secrets.rs`. Verdict: NEEDS-CHANGE; fixed in `d047fb20`.

```findings
- file: xtask/src/secrets.rs
  line: 200
  category: acceptance
  severity: blocker
  verdict: NEEDS-CHANGE
  origin: introduced
  message: a work-tree .gitattributes "* -diff" makes every diff binary, Gitleaks scans 0 commits and the expected count drops to 0, so check() passes a history holding an unscanned credential that the base check refused
- file: xtask/src/secrets.rs
  line: 200
  category: contract-drift
  severity: warning
  verdict: NEEDS-CHANGE
  origin: introduced
  message: yields_fragment counts an archive diff, but Gitleaks 8.30.1 at default --max-archive-depth 0 yields no fragment for archives, so an archive-only commit makes check() refuse a legitimate history, and the test at line 432 asserts the wrong model
```

Fixes: 1 a diff Git prints as binary whose new blob has no NUL in its first 8000 bytes is refused by commit and path; 2 archives count as nothing, the suffix list is gone.
