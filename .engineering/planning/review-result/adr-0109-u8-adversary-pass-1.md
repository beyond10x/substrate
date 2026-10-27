---
format: aep.planning-md/3
id: review-result:adr-0109-u8-adversary-pass-1
kind: review-result
status: active
title: 'Adversary pass 1: macos S1 and S2'
relations:
- reviews: story:macos-adapter-on-seatbelt
- reviews: story:macos-seatbelt-policy-runs-cargo-test
revision: 1
---
# Adversary pass 1: macos S1 and S2

Reviews `story:macos-adapter-on-seatbelt` and `story:macos-seatbelt-policy-runs-cargo-test` at
integration `d5ebb08`. Two attempts stopped before writing a test case. This pass therefore
pins no case. It reports one finding and one check, both read from the code and not run.

## Findings

| # | file:line | severity | verdict | finding |
|---|---|---|---|---|
| A1 | `crates/substrate-host/src/confinement/macos.rs:690` | note | CONFIRMED | `render` refusals write no audit record. The oci adapter audits its render refusals (`oci.rs:1506`); local, linux and systemd do not. |

Checked, holds: `spec/confinement-suite.json` declares 8 `-on-macos` scenarios (4 `macos-s1-*`,
3 `macos-s2-*`, 1 `absent-linux-*`), and the workflow's count pins match the test files.

## Decisions taken by the coordinator

- A1: no change in this story. A refused render starts nothing, and only oci audits one. Making
  render audits uniform is a contract change for every adapter; it is out of the macOS stories' scope.
- No third adversary attempt. The security review's seven conformance cases cover the guarantees
  the adversary brief listed: symlinked exec subpath, hard link to a sealed file, sealed path case,
  TMPDIR removal, cross-run signal, Unix socket connect and dyld variables.
