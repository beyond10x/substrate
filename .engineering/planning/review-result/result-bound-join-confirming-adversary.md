---
format: aep.planning-md/3
id: review-result:result-bound-join-confirming-adversary
kind: review-result
status: active
title: Confirming adversary pass on the join of the result bound
relations:
- reviews: story:every-success-result-fits-the-ledger-before-its-side-effect
revision: 1
---
# Confirming adversary pass on the join: the vectors' machine and the published snapshot

2026-11-24, on the commit `163af08f5` of the branch `feat/result-bound` (ADR 0053 with its
amendment of 2026-11-24). The adversary added two cases, test code only, in
`crates/substrate-daemon/tests/http/result_bound.rs` and
`crates/substrate-daemon/tests/contract_vectors.rs`, and did not edit the implementation, a
bundle or a document. Verdict: approve. The daemon's targets (`driver_port`, `http`,
`processes`, `contract_vectors`, `runtime_vectors`) executed 198 to 200, red 1, skipped 1.
S1: 0. S2: 0. S3: 4.

Nothing of this branch is released: bundle `0.28.0` is not published.

## Held

- The host probe and the namespace driver state `OPERATION_RESULT_MAX_JSON_BYTES`, and `serve`
  opens its ledger with the default headroom; no daemon flag or environment variable sets the
  headroom. Every listener of `serve` starts after the check of the two figures (read).
- A start compares the digest it names with one reading of the driver's machine, and no route
  rewrites a fact (read).
- The runner: with the vector's facts kept from the driver, four cases fail at the read-back
  (mutant, run). The request is sent as written. The host-process case keeps every assertion it
  had in `tests/processes/result_bound.rs`.
- The margins over 383 are 23,946, 23,946, 25,240, 260 and 144 bytes by the daemon's own rule
  (`result_bound_margins_are_the_published_ones_by_the_daemons_rule`, run). xtask and the daemon
  use the wire crate's width functions.
- 383 is the least reserve: a store configuration with a terminal headroom under 454 is refused
  (read).
- An id `SystemAuthority` mints is at most 31 bytes (read).
- Only `0.28.0/bundle.json`, the five refusal vectors and their five sources changed; the
  SHA-256 of the inner `bundle.json` is the advertised one; no released directory and no
  delegated-context file changed (run).
- The amendment, the `AGENTS.md` sentence of invariant 6, the changelog's range and the digest in
  `STATUS.md` agree with the code (read).

Not established by the pass: the xtask package, `check-secrets` and the lanes were not run by
it. Mutants not run: those of `check_machines`.

## Findings and what was done

1. S3. Rule asked: an app that serves publishes the figure its ledger decides against, on every
   way an app is built. Case
   `no_app_publishes_a_figure_its_ledger_does_not_decide_against`. `serve` holds the
   rule; no binary or crate of this repository builds an app another way.
2. S3. Rule asked: a snapshot that states another figure than the ledger reserves is refused in
   both directions. The committed case tested one.
3. S3. Rule asked: a case cited for a rule fails when the rule is broken. Three cases stay green
   when the published figure is taken from the ledger again, because their fixtures state one
   figure on both sides.
4. S3. Rule asked: a comment says what the code does. `crates/substrate-host/src/probe.rs`
   described the daemon's publication as it was before the commit.

Done in the branch's next commit, whose message states each.

Run of the pass, release profile, in the build pool: the daemon's five targets 200 run, 199
passed, 1 failed (the case of finding 1), 1 skipped.
