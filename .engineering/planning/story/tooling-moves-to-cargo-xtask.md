---
format: aep.planning-md/1
id: story:tooling-moves-to-cargo-xtask
kind: story
status: draft
title: 'Anything that runs is Rust: the gate''s Python moves to cargo xtask'
summary: 'atlas AGENTS.md section Language: gates, checkers, renderers and packagers are Rust. 23,916 lines of Python beside 30,768 of Rust; the four frozen bundle renderers stay as reproducibility proofs, everything else moves, and 0.5.0 is rendered from substrate-wire types.'
owner: substrate
tags:
- build
- rust
relations:
- decomposes: epic:release-hardening
revision: 2
---
# Story: Anything that runs is Rust — the gate's Python moves to `cargo xtask`

## Outcome

`scripts/gate.sh` runs Rust: one `xtask` crate (clap) owns `check-links`, `check-adrs`,
`check-toolchain`, `package-bundle`, `check-bundle` and the runtime-vector runner, and the next
wire bundle (`0.5.0`) is rendered from `substrate-wire`'s types rather than from Python dicts. The
four frozen renderers and their checkers stay exactly as they are, as the reproducibility proof of
`0.1.0`–`0.4.0` (invariant 6), and nothing new is written in Python.

## Context

`atlas/AGENTS.md` § *Language*, 2026-08-29: anything that runs is Rust; a new `.py` in a foundation
repository is an operator decision. Measured the same day: `scripts/*.py` is 23,916 lines beside
30,768 lines of Rust; 18,300 of those are `render-contract-bundle{,-0.2.0,-0.3.0,-0.4.0}.py` and
`check-contract-bundle{,-0.2.0,-0.3.0,-0.4.0}.py`, near-copies per version. The validator that does
the standards work is already Rust (`crates/substrate-contract-check`, `jsonschema = "=0.49.9"`;
`scripts/contract_json_gate.py:195` shells out to it). The wire has two descriptions — the Rust
types in `crates/substrate-wire` and the renderer's dicts — and only the vectors would notice them
drifting. Wave 1 of this store added ~1,200 more Python lines (`check-toolchain.py`,
`package-contract-bundle.py` and its tests) by following the local convention; they are the first
to move.

## Acceptance

`bash scripts/gate.sh` invokes no Python except the four frozen renderers' checkers, every moved
check refuses the same inputs its Python predecessor refused (proven by the predecessor's own
negative cases run against the Rust replacement before the Python is deleted), `0.4.0` still
verifies byte-clean, and `cargo xtask render-bundle 0.5.0` is the only renderer for the successor.

Evidence that satisfies it, in order:

1. `xtask/` crate in the workspace (`cargo xtask <verb>`, clap derive), `--locked` clean.
2. `check-toolchain`, `check-links`, `check-adrs` moved first (smallest, no wire contact), each with
   the Python negative cases ported as tests; the Python files deleted in the same commit.
3. `package-bundle` moved with `test_package_contract_bundle.py`'s 21 cases ported; the 0.4.0
   manifest digest `sha256:3758e80b…` reproduced by the Rust packager before the Python goes.
4. `check-runtime-vectors` moved (the black-box Unix-socket HTTP runner, 1,194 lines) — last of the
   gate steps, because it is the one with the most behaviour.
5. `render-bundle 0.5.0` derives schemas from `substrate-wire` types (`schemars` or hand-rolled
   emitters — decided in the story), with the `0.4.0` compatibility block computed, not typed.
6. `AGENTS.md` § *The gate* and `README.md` § *Build, test, run* list the xtask verbs; the four
   frozen renderers are documented as frozen proofs, not tooling.

## Out of Scope

Re-rendering `0.1.0`–`0.4.0` from Rust. They are frozen; their Python renderers are their proof.

## Open Questions

Whether `check-runtime-vectors` becomes an `xtask` verb or an integration test under
`crates/substrate-daemon/tests/` (it drives a real daemon over a socket, which the daemon's tests
already do). Decides: operator. Default if nobody answers: **integration test** — one fewer
binary, and the gate already runs `cargo test`.
