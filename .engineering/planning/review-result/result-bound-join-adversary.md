---
format: aep.planning-md/3
id: review-result:result-bound-join-adversary
kind: review-result
status: active
title: Adversary pass on the join of the result bound
relations:
- reviews: story:every-success-result-fits-the-ledger-before-its-side-effect
revision: 1
---
# Adversary pass on the join: the result bound is advertised, published and executed

2026-11-24, on the commit `db8bed91d` of the branch `feat/result-bound` (ADR 0053: the join
`5f7834a07` and the merge of the store's resource bound). The adversary added one case, test
code only, in `crates/substrate-daemon/tests/contract_vectors.rs`, and did not edit the
implementation, a bundle or a document. Verdict: one case red. The daemon's targets
(`driver_port`, `http`, `processes`, `contract_vectors`, `runtime_vectors`) executed 197 to 198,
red 1, skipped 1. S1: 0. S2: 1. S3: 2.

Nothing of this branch is released: bundle `0.28.0` is not published.

## Held

- Under `contracts/` and `xtask/bundle-source/` only `0.28.0` paths differ from `main`. The
  advertised digest is the SHA-256 of the bundle's inner `bundle.json`. No code advertises
  `0.27.0` as current.
- The daemon publishes its store's figure. A machine that does not state the fact reads at
  `OPERATION_RESULT_MAX_JSON_BYTES`. The host probe states the constant.
- The published figure and the compared figure are one number: a result as wide as the reserve
  is dispatched, one a byte wider is refused, and the store is exact at the reserve. Host
  processes decide against the same store.
- All six vectors execute with exact responses, one refused row, no dispatch and no change on
  the host. The 411-byte path lies within the published path bounds.
- `check-bundle 0.28.0` holds the cwd bound to `MAX_PROCESS_CWD_BYTES` (378 files verified).
- The gate's `check-bundles` and `check-json` lines include `0.28.0`, and `release.yml` packages
  it. Counts in the texts hold: 28 bundle directories, 45 routes, 61 constants, 378 files.
- Every acceptance names an id the daemon minted or looked up first.

Not established by the pass: the library cases of the wire crate, the daemon, the SDK and the
store, the xtask package and `check-docs` were not run by it; the delegated, namespace and oci
lanes; whether the client's count of decision 5 is at least the daemon's count of decision 6 for
every member.

## Findings and what was done

1. S2. Rule asked: a vector runs on the machine its setup states, and the figure that machine
   publishes is the figure the request is decided against. Case
   `result_bound_vectors_run_on_the_machine_their_setup_states`. The same rule for the
   host-process vector's request: it is sent as written.
2. S3, no case. Rule asked: a page states only what a release did, and a link opens on what it
   names. The sentences `check-docs` holds to the workspace version name the current release
   with bundle `0.28.0`. They are true from the release-prepare commit, with which this branch
   reaches `main`; the branch does not reach `main` without it.
3. S3, no case. Rule asked: every id the daemon accepts an operation with is one the store
   accepts. The `Authority` trait states no length for the ids it mints.
4. Note. ADR 0053 decision 4 says the probe and the default fill the fact; the daemon publishes
   its store's figure, and the snapshot's digest is to cover the facts as published.

Done in the branch's next commit, whose message states each: findings 1, 3 and 4. Finding 2 is
done by the release-prepare commit of the release branch.

Run of the pass, release profile, in the build pool: the daemon's five targets 198 run, 197
passed, 1 failed (the case), 1 skipped; `cargo xtask check-bundle 0.28.0` exit 0.
