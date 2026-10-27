---
format: aep.planning-md/3
id: review-result:adr-0109-fu11-adversary-pass-1
kind: review-result
status: active
title: 'Adversary pass 1: confined cargo build links without the xcrun shim'
relations:
- reviews: story:confined-cargo-build-links-without-the-xcrun-shim
revision: 1
---
# Adversary pass 1: confined cargo build links without the xcrun shim

Verdict: needs change. Read at unit head `01776c09` (merged as `9d776eb9`); cases on branch
`review/adr-0109-fu-macos-adversary` at `039091e3`, red on macos-15 run 40473479613.

## Findings

- A1 (warning): the adapter sets no `CC` or `AR`, so a build script compiling C the way the `cc`
  crate does reaches the `/usr/bin/cc` shim and fails under the policy. Case in
  `crates/substrate-host/tests/seatbelt_developer_tools_adversary.rs`.
- A2 (warning): the selected developer directory as exec subpath holds the resolved SDK and clang
  but gets neither SDKROOT nor the linker. Cases in the same file and the `macos.rs` test module.
- A3 (warning): the linker variable is set only for the daemon's build architecture, not for the
  other Apple triple a `--target` build reads. Cases in the same file and the `macos.rs` test
  module.
- A4 (note): the canonical, versioned SDK path kept from open stops existing after an SDK update
  moves the `MacOSX.sdk` link. Unit case in `macos.rs`.

## Outcomes

- A1 fixed in `9f0ee5dc`: `ar` is resolved at open beneath the developer root; a start gets `CC`
  and `AR`.
- A2 fixed in `9f0ee5dc`: each value is given when its path lies beneath an exec subpath; an exec
  subpath holding the developer directory without a needed value is refused by name.
- A3 fixed in `9f0ee5dc`: the linker is set for `aarch64-apple-darwin` and `x86_64-apple-darwin`.
- A4 fixed in `9f0ee5dc`: the SDK path is kept as xcrun names it (the unversioned link); a path
  that stops resolving is refused by the F2 check.
- The A2 unit case's exact-equality assertion became a presence check for its pairs, because A1
  and A3 add `CC`, `AR` and the second triple's linker to the same environment; the story id and
  reason are in that hunk.
- Red before the fix: the three unit cases, `0 passed; 3 failed`. Green on macos-15 run
  40476923925: `seatbelt_developer_tools_adversary ok. 3 passed`, `seatbelt_adversary ok. 12
  passed`, lib `ok. 51 passed`. macOS job 3m49s against 3m37s before. Merged on the follow-up
  branch as `403f3d79`.
