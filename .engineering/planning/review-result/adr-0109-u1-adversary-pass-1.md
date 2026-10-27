---
format: aep.planning-md/3
id: review-result:adr-0109-u1-adversary-pass-1
kind: review-result
status: active
title: Adversary pass 1 on story:ess-pin-0-70-0
relations:
- reviews: story:ess-pin-0-70-0
revision: 1
---
# Adversary pass 1: story:ess-pin-0-70-0

Verdict: no defect introduced by the unit; one pre-existing documentation note.

## Checked and not broken

- `grep -rn '0\.69\.2' spec xtask .github` exits 1; every `requires: ess` under `spec/` names 0.70.0.
- `scripts/gate.sh` reaches ESS only through `cargo xtask check-ess`; `macos.yml`, `release.yml` and
  `pages.yml` install no ESS.
- `ESS_SHA256` at `.github/workflows/gate.yml:128` equals the release asset digest of
  `ess-0.70.0-x86_64-unknown-linux-gnu.tar.gz` and the `sha256sum` of the downloaded archive; the
  asset name built at `gate.yml:130` is the release asset's name; the archive's single top-level
  directory fits `--strip-components=1`.
- Regenerating with 0.70.0 reproduces the committed bytes of the operations, host-processes and
  workspaces suites, the three generated type files and the CLI package.
- Re-run facts: host processes 91 scenarios and 28 refusals (23 ESS-SYNTH-010, 4 ESS-SYNTH-001,
  1 ESS-SYNTH-022), 110 passed interpreted; workspaces 23 scenarios (5 authored), 1 refusal
  (ESS-SYNTH-013 on `RootName`), 23 passed; operations 14 scenarios, 0 refusals.
- `contracts/` unchanged; the only `crates/substrate-wire/src/lib.rs` change is the comment at line
  2446.

## Finding

- `spec/workspaces/README.md:100` (note, pre-existing): the line said `RootName` and `WorkspaceId`
  stay unscenarioed under ESS-SYNTH-013. Both 0.70.0 and the base suite synthesize
  `substrate.workspaces.WorkspaceId/invariant/at/substrate.workspaces.Workspaces/workspace`; only
  `RootName` is refused.
