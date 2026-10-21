---
format: aep.planning-md/3
id: story:macos-host-compiles
kind: story
status: active
title: substrate-host compiles on macOS; Linux-only host behaviour is gated and refuses by name elsewhere
summary: b10x-substrate-host builds for aarch64-apple-darwin so loom and takt can ship macOS again; HostDriver and HostProcessSupervisor refuse at open off Linux.
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-10-21T08:57:23Z", actor: "human:timo", revision: 3}
- {from: "proposed", to: "active", at: "2026-10-21T08:57:24Z", actor: "human:timo", revision: 4}
---
## Outcome
`cargo check -p b10x-substrate-host --target aarch64-apple-darwin` succeeds. takt's release job "Build aarch64-apple-darwin" failed on substrate-host 0.7.17 with 83 errors (https://github.com/beyond10x/takt/actions/runs/39569846703), which held takt 0.7.0 to Linux.

On an operating system other than Linux the crate compiles its contract types and two refusing stand-ins: `HostDriver::open` returns `host.platform-unserved` (class `unserved`, address `host.platform`) and `HostProcessSupervisor::open` returns `process.host-unserved`, both before touching the host. Linux behaviour is unchanged: the Linux modules gain `#[cfg(target_os = "linux")]` and two type groups (`EgressAperture`; `ExecObservation`, `PipeFrame`, `PipeStream`) move to portable modules with the same public paths.

## Acceptance
- A macOS CI job runs `cargo clippy -p b10x-substrate-host --all-targets -- -D warnings` and the crate's library tests on a macOS runner, including the two refusal tests.
- The Linux workspace gate (`scripts/gate.sh`) passes unchanged.

## Scope
`crates/substrate-host/src/{lib.rs,aperture.rs,observation.rs,unsupported.rs,egress.rs,process.rs}`, `crates/substrate-host/src/host_process/{mod.rs,ports.rs,unsupported.rs}`, the integration tests under `crates/substrate-host/tests/` (Linux-gated), `.github/workflows/macos.yml`, `CHANGELOG.md`.
