---
format: aep.planning-md/3
id: review-result:wave-2026-10-23-b13-adversary-pass-1
kind: review-result
status: active
title: Adversary pass 1, unit b13 (check-secrets graft file)
relations:
- reviews: story:check-secrets-ignores-a-local-graft-file
revision: 1
---
# Adversary pass 1, unit b13 (check-secrets graft file)

Agent type `aep:adversary`. Branch `unit/2026-10-23-b8b13-docs-secrets`, commit `5d4f530`, file `xtask/src/secrets.rs`. Verdict: NEEDS-CHANGE, 4 red (2 default, 2 ignored end-to-end).

```findings
- file: xtask/src/secrets.rs
  line: 73
  category: acceptance
  severity: warning
  verdict: NEEDS-CHANGE
  origin: pre-existing
  message: "-m defers to the caller's log.diffMerges, so off or dense-combined drops a merge's added content from both Gitleaks' log and the expectation, and check-secrets passes a merge carrying a credential."
```

Cases added (module `tests`, prefix `adversary3_`): two expectation cases under `log.diffMerges=off` and `dense-combined` (red), two end-to-end cases for the same (red, ignored), a default-config control (green) and a `color.diff=always` case (green, refuses).

Held: `diff.external`, `GIT_EXTERNAL_DIFF`, attribute diff drivers (ignored by `git log -p`), `GIT_NAMESPACE`, `log.diffMerges=first-parent` and `remerge`, graft file and shallow clone. Not tested: a forged commit-graph, `GIT_DIR`/`GIT_OBJECT_DIRECTORY` redirection, `core.bigFileThreshold`.

Coordinator decision: fixed in this unit with `--diff-merges=separate` in both places. Caller config is not dropped wholesale (`GIT_CONFIG_GLOBAL=/dev/null`), because CI's checkout records `safe.directory` there.
