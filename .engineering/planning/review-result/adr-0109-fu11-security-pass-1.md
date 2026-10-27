---
format: aep.planning-md/3
id: review-result:adr-0109-fu11-security-pass-1
kind: review-result
status: active
title: 'Security review pass 1: confined cargo build links without the xcrun shim'
relations:
- reviews: story:confined-cargo-build-links-without-the-xcrun-shim
revision: 1
---
# Security review pass 1: confined cargo build links without the xcrun shim

Verdict: needs change. Read at unit head `01776c09` (merged as `9d776eb9`); cases on branch
`review/adr-0109-fu-macos-security` at `173b3938`, red on macos-15 run 40473407531.

## Findings

- F1 `crates/substrate-host/src/confinement/macos.rs` (blocker): xcrun runs at open with the
  embedding process's environment and working directory, so `DEVELOPER_DIR` chooses which xcrun
  runs outside the sandbox and which SDK and linker the run is given. Red case:
  `security_review_macos::open_runs_no_xcrun_the_embedding_environment_names`.
- F2 `macos.rs` (warning): developer tools selected after open (xcode-select to another Xcode)
  get no SDKROOT, no linker and no refusal. Red cases:
  `security_review_macos::an_xcrun_named_by_the_environment_cannot_name_the_linker`,
  `security_review_macos_developer_selection` (one case, its own binary, changes and restores the
  host selection).

## Outcomes

- F1 fixed in `9f0ee5dc`: xcrun runs with a cleared environment, `PATH=/usr/bin:/bin`, working
  directory `/` and stdin from `/dev/null`.
- F2 fixed in `9f0ee5dc`: at start the developer root reported now is compared with the one stored
  at open; a difference, or a stored SDK that no longer resolves beneath it, is refused
  `exec.sandbox-developer-tools-unresolved` before dispatch, for starts whose exec subpaths reach
  the developer tools.
- All three cases pass on macos-15 run 40476923925 (`security_review_macos ok. 9 passed`,
  `security_review_macos_developer_selection ok. 1 passed`). Merged on the follow-up branch as
  `403f3d79`.
