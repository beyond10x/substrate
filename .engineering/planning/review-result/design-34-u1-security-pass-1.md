---
format: aep.planning-md/3
id: review-result:design-34-u1-security-pass-1
kind: review-result
status: active
title: Security review of design 34 U1, the Kubernetes client, pass 1 (2 warnings, fixed)
relations:
- reviews: story:kubernetes-namespace-workspace-and-exec-slice
revision: 1
---
# Security review of design 34 U1 (Kubernetes client extraction), pass 1

Reviewed `unit/k8s-api-extraction` at `c77cd337`, 2026-10-29. Verdict: two warnings, both
introduced by the unit, each with a red case committed as `2fab66ee`; fixed in `54ebd97e`.

## Findings

| # | file:line | severity | finding | fix |
|---|---|---|---|---|
| 1 | `crates/substrate-kubernetes-client/src/api.rs:230`, `:908`, `exec.rs:146` | warning | `Status.message` was cut to 300 characters before the bearer token was redacted; a token split by the cut left its prefix in the error (case `an_echoed_credential_split_by_the_message_bound_leaves_no_part_of_it`) | redact, then cut, in one helper |
| 2 | `xtask/src/packages.rs:88` | warning | the `fake` fence missed a renamed client inherited through `[workspace.dependencies]` with `workspace = true`; `cargo tree` showed `fake` on host's runtime edge in a scratch copy (`xtask/tests/kubernetes_client_fake_fence_adversary.rs`, 2 red, 1 control) | resolve the alias through `[workspace.dependencies]` |
| 3 | `api.rs:729` | note | `Api::watch` had no deadline until response headers arrived | header wait bounded by `Api::timeout`, returns `Unanswered`; a test with headers delayed 5 s against 300 ms |

## Checked and cleared

`Debug` and kubeconfig refusals carry no credential; TLS uses the kubeconfig CA only, https only,
no verification bypass; the WebSocket upgrade uses the same connector and server name and follows
no redirect; body, page, watch, exec-frame and per-stream bounds hold; continue tokens stop at
`max_pages`; `delete_preconditioned` always sends the uid; `dryRun=All` is always appended;
`pod.rs` behaviour, golden manifest and selectors are unchanged.

## After the fix (`54ebd97e`), each run once

Client lib 11/0; fence adversary 3/0; xtask `packages::` 6/0; host pod unit 33/0; `adversary_pod`
21/0; `ess_confinement_pod` 14/0; `security_review_pod` 7/0. Not run: the kind pod lane.

## Open for later slices

The fake has no 504 or delay-past-timeout fault plan beyond the one-shot `Delay` added here;
design 34 § 10.1 lists them, for the driver slices (U6, U8, U9).
