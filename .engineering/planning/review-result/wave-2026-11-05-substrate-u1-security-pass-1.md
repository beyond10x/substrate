---
format: aep.planning-md/3
id: review-result:wave-2026-11-05-substrate-u1-security-pass-1
kind: review-result
status: active
title: Wave 2026-11-05 substrate U1 security pass 1
relations:
- reviews: story:substrate-host-features-let-confinement-consumers-skip-git-and-kubernetes
revision: 1
---
# Security pass 1 — wave 2026-11-05 U1 (story:substrate-host-features-let-confinement-consumers-skip-git-and-kubernetes)

Diff `43c49c42...3f519773`, merged into the integration branch at `3dd1f266`. Reviewer commit
`3e60d5b6` on `sec/2026-11-05-u1`, merged at `10a493b6`. Agent `aep:security-reviewer`. Header
and findings verbatim; prose trimmed to the measured facts.

```
verdict: NEEDS-CHANGE
cases: executed 26→27, red 2
origin: introduced 2, pre-existing 0, undecided 0
```

The new case, `xtask/tests/host_no_default_graph_security_review_u1.rs`, gives
`b10x-substrate-guarded` a `cfg(unix)` git2 edge. The host's no-default `cargo tree` then names
`git2` and `openssl-sys`, and `check-packages` still accepts the manifest: 0 passed, 1 failed.

`kubernetes_client_fake_fence_adversary` fails with `duplicate key` at the host's new `[features]`
table, so the gate's workspace test step is red.

Checked and held:

- With Git off, no `workspace.git` fact or Git-root fact is published, and Git sources and Git
  roots are refused unserved before dispatch.
- `git_unserved::remove_baseline` is byte-identical to the Git-on version, with the same callers
  and the same root-name validation.
- With Kubernetes off, the pod open refuses without reading the request, the environment or the
  kubeconfig, and is audited like any other unopened adapter.
- The default adapters are untouched.
- The CLI admits and refuses the same things.

Resolution: both findings go to the U1 fix before the gate.

```findings
[{"file":"xtask/tests/kubernetes_client_fake_fence_adversary.rs","line":131,"category":"contract-drift","severity":"blocker","verdict":"NEEDS-CHANGE","origin":"introduced","message":"scope in; the unit's new host [features] table makes this existing fake-fence case fail on a duplicate-key TOML parse error, so the gate's workspace test step is red"},
 {"file":"xtask/src/packages.rs","line":953,"category":"acceptance","severity":"warning","verdict":"NEEDS-CHANGE","origin":"introduced","message":"scope in; check-packages fixes only the host's own [features] table and never reads the host's no-default cargo tree, so a git2/openssl-sys edge below the host reaches every confinement-only consumer with the gate green"}]
```
