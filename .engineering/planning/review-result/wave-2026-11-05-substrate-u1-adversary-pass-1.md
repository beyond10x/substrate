---
format: aep.planning-md/3
id: review-result:wave-2026-11-05-substrate-u1-adversary-pass-1
kind: review-result
status: active
title: Wave 2026-11-05 substrate U1 adversary pass 1
relations:
- reviews: story:substrate-host-features-let-confinement-consumers-skip-git-and-kubernetes
revision: 1
---
# Adversary pass 1 — wave 2026-11-05 U1 (story:substrate-host-features-let-confinement-consumers-skip-git-and-kubernetes)

Diff `43c49c42...3f519773`, merged into the integration branch at `3dd1f266`. Adversary commit
`77a3748c` on `adv/2026-11-05-u1`, merged at `6bedbd3b`. Agent `aep:adversary`. Header and
findings verbatim; prose trimmed to the measured facts.

```
verdict: CONFIRMED
cases: executed 49→51, red 0
origin: introduced 4, pre-existing 0, undecided 0
```

Two cases in `crates/substrate-host/tests/host_features.rs`:

- `forgetting_a_workspace_removes_the_baseline_an_earlier_git_build_wrote` is red on a mutant whose
  no-git `remove_baseline` returns `Ok(())` without removing anything. Without the case the
  no-default lane passed that mutant 49/49.
- `the_no_default_tree_names_no_git_stack_openssl_or_kubernetes_client` is red on a mutant that
  gives `substrate-guarded` a curl edge. Without the case the lane and `check-packages` passed it.

Attacked and held:

- Clippy is clean for every feature combination.
- The default tree's packages and features are identical to the base.
- The public items are unchanged under every feature set.
- The CLI resolves the host with neither feature and reaches no Git or pod path.
- The no-default lane is real: restoring the CLI's default features fails it.

Resolution: the `packages.rs:239` warning and the `lib.rs:1411` note go to the U1 fix unit with the
security pass's two findings. The `git_unserved.rs:64` row is out of scope: no consumer opens a
state root a Git build wrote with a build without Git. The CHANGELOG remark was already handled by
the release-prepare commit.

```findings
[{"file":"xtask/src/packages.rs","line":239,"category":"acceptance","severity":"warning","verdict":"CONFIRMED","origin":"introduced","message":"scope in; check-packages fences only the host's features table, so a transitive normal dependency put curl, curl-sys and openssl-sys in the no-default tree with check-packages and the no-default lane green (M2); now held by the_no_default_tree_names_no_git_stack_openssl_or_kubernetes_client"},
 {"file":"crates/substrate-host/src/git_unserved.rs","line":79,"category":"mutant","severity":"note","verdict":"CONFIRMED","origin":"introduced","message":"scope in; a no-op remove_baseline in the build without git passed the no-default lane 49/49 (M1); now caught by forgetting_a_workspace_removes_the_baseline_an_earlier_git_build_wrote"},
 {"file":"crates/substrate-host/src/git_unserved.rs","line":64,"category":"edge-case","severity":"note","verdict":"INFEASIBLE","origin":"introduced","message":"scope out; the no-git reconcile leaves a baseline and .substrate-git-* staging directories that git.rs:213 would remove, reachable only when a build without git opens a state root a git build wrote; no such consumer found"},
 {"file":"crates/substrate-host/src/lib.rs","line":1411,"category":"contract-drift","severity":"note","verdict":"CONFIRMED","origin":"introduced","message":"scope in; a build without git refuses a configured Git source with 'The requested Git source is not configured.' instead of the message naming the missing feature"}]
```
