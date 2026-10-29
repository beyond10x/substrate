---
format: aep.planning-md/3
id: review-result:design-34-u2-adversary-pass-1
kind: review-result
status: active
title: Adversary pass on design 34 U2, the guarded extraction, pass 1 (3 notes)
relations:
- reviews: story:kubernetes-namespace-workspace-and-exec-slice
revision: 1
---
# Adversary pass on design 34 U2 (guarded extraction), pass 1

Reviewed `unit/guarded-extraction` at `6d7ae315`, 2026-10-29. Verdict: nothing found; no red
case. Three note-level findings, all introduced by the unit.

## Findings

| # | file:line | severity | finding |
|---|---|---|---|
| 1 | `crates/substrate-host/tests/confinement_public_items.rs:29` | note | the doc comment says `Listed`'s shape is held; `Listed` is absent from the committed list because nothing in confinement reaches it |
| 2 | `spec/workspaces/README.md:18,48`, `STATUS.md:158`, `docs/design/29…:26`, YAML comments in `spec/workspaces/`, `README.md:582` | note | inline paths still name `crates/substrate-host/src/fs.rs`; the README crate table omits `substrate-guarded` |
| 3 | `crates/substrate-guarded/src/fs.rs:1427` | note | `pub mod fs` exports more than design 34 § 2 names, including `pin_workspace_directory`, which opens the parent by path and follows links; host uses every new public item today, so no guard is bypassed now |

## Attacks that did not break it

- Moved code is byte-identical to the base once `pub(crate)`/`pub(super)` is normalised to `pub`.
- `cfg(target_os)` gating, crate attributes and dependency features are equivalent.
- `baseline_value` has one caller (`process.rs:2802`) and one definition.
- Four scanner mutants (a new `pub fn` on `DriverError`, a new `From` impl, a field added to a
  type `DriverError` names, a new `pub use` of a guarded type) were each caught by
  `confinement_public_items`.

## Suites run once each

guarded 31 passed; `confinement_public_items` 18 passed; host lib 387 run; `check-packages` 7
packages, 11 manifests.
