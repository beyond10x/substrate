---
format: aep.planning-md/3
id: decision-blocker:macos-s2-cargo-link-in-seatbelt
kind: decision-blocker
status: open
title: How cargo links inside the macOS S2 Seatbelt policy
relations:
- blocks: story:macos-seatbelt-policy-runs-cargo-test
withholds: test_result
revision: 1
---
## What is stopped

`story:macos-seatbelt-policy-runs-cargo-test` (S2). Two cases in
`crates/substrate-host/tests/seatbelt_adversary.rs` fail on macos-15 twice on one cause, which is
the delivery stop rule:

- `cargo_test_of_a_fixture_with_a_vendored_dependency_and_a_build_script_passes`
- `a_network_connection_from_the_fixtures_test_is_refused`

Runs: https://github.com/beyond10x/substrate/actions/runs/40456837981 and
https://github.com/beyond10x/substrate/actions/runs/40460300228 (branch
`fix/adr-0109-macos-s2`, head `1a3dac19`).

## Cause, as observed in the run log

Cargo links through `/usr/bin/cc`, an `xcrun` shim. Inside the policy the shim runs
`sh -c '<developer>/usr/bin/xcodebuild -sdk <sdk> -find clang'`; `/bin/sh` is in no exec subpath,
so the shell exits 127 and `cc` exits 72. With `SDKROOT` given, the `rustc` warning is gone and the
shim still takes this path.

## Options

| | change | cost |
|---|---|---|
| a | the test sets the linker to `<developer>/Toolchains/XcodeDefault.xctoolchain/usr/bin/clang`, resolved outside the sandbox like `RUSTC` and `SDKROOT` | test-only, no policy change; the case no longer proves the stock `cc` shim links |
| b | the fixture's profile declares `/bin` as an exec subpath | widens what the fixture may exec; likely needs further paths |
| c | admit `xcrun`'s per-user cache | a path under `/var/folders`, not a fixed system path; excluded by the policy rules |

Recommended: a. Not attempted, because the stop rule fired.

## Also on that branch, not merged

Security review F1, F2, F4, F5 fixed and F3 kept (its case passed on macos-15), `DYLD_*`
refusal, two policy read paths (`/private/etc/ssl`, the Xcode licence plist). Integration holds
none of them until the branch is green.
