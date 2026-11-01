---
format: aep.planning-md/3
id: approval-record:d136-build-terms-for-wave-d354
kind: approval-record
status: approved
title: 'D136: build terms for the rest of wave D354'
relations:
- informed_by: approval-record:d354-substrate-wave-2026-11-01
revision: 2
transitions:
- {from: "draft", to: "approved", at: "2026-11-01T17:07:14Z", actor: "human:timo", revision: 2}
---
# Build terms for the rest of wave D354 (host-monitor D136)

The orchestrator relayed host-monitor decision D136 (2026-11-01 17:0xZ) for the rest of wave D354, verbatim except that a local checkout path is replaced by the project name:

> - Cargo of the telos project (including telos-rustgen) does not count as "other cargo" while load1 < 16 and io psi avg60 < 5.
> - With at most 1 other non-telos cargo, run at CARGO_BUILD_JOBS=8. With 2 non-telos others, start anyway at jobs=4.
> - Floors are unchanged: / >= 80G before each command, stop under 70G.

## Decision

Every cargo command in the wave's trees runs after the wave's pre-check, which applies these terms
and prints the job count to use. Under 80G free, a target directory of 15G or more, or more than two
other cargo processes, the command does not start and the unit reports blocked.

## Scope

Wave D354 only (approval-record:d354-substrate-wave-2026-11-01). It changes no repository file.
