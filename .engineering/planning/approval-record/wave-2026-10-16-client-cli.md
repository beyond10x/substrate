---
format: aep.planning-md/3
id: approval-record:wave-2026-10-16-client-cli
kind: approval-record
status: draft
title: 'Wave 2026-10-16: a client CLI'
relations:
- decides: story:a-client-cli-drives-the-daemon
revision: 1
---
# Wave 2026-10-16: a client CLI

Integration branch `wave/2026-10-16-client-cli`, base `276d08c3`; one pull request, then a release.

Width: 1 story in 2 sequential units, because every other draft story needs an ADR, a successor wire
bundle or a provisioned runner before code, and the host-process stories wait on ADR 0036.

| unit | step | tree branch | lands on |
|---|---|---|---|
| u0 | ESS pin to 0.60.0; CLI presentation binding | `unit/2026-10-16-u0-cli-spec` | `spec/`, `.github/workflows/gate.yml` (ESS version) |
| u1 | the `b10x-substrate` binary against the binding | `unit/2026-10-16-u1-cli` | `crates/b10x-substrate-cli/`, `Cargo.toml`, `Cargo.lock`, docs |

Reviews beside the build: an adversary on u1 (refusal and exit-code mapping, absent capability facts).
