---
format: aep.planning-md/1
id: story:tooling-moves-to-cargo-xtask
kind: story
status: active
title: 'Anything that runs is Rust: the gate''s Python moves to cargo xtask'
summary: 'atlas AGENTS.md section Language: gates, checkers, renderers and packagers are Rust. 23,916 lines of Python beside 30,768 of Rust; the four frozen bundle renderers stay as reproducibility proofs, everything else moves, and 0.5.0 is rendered from substrate-wire types.'
owner: substrate
tags:
- build
- rust
relations:
- decomposes: epic:release-hardening
revision: 7
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

**Resolved 2026-08-30: integration test.** No existing daemon test spawns the binary — `http.rs:29-34`,
`websocket.rs:36-38`, `pipe_session.rs` and `contract_vectors.rs` all build `App::new(...)` in
process — so there was no harness to reuse, but the black-box property survives:
`env!("CARGO_BIN_EXE_substrate-daemon")` lets a test in that package spawn the shipped binary over
a Unix socket. Nothing outside the gate invoked the script.

## Progress — 2026-08-30, steps 1–2

- `xtask/` (bin-only, clap derive; `anyhow`, `clap`, dev `tempfile` — no new crate, `Cargo.lock`
  +9 lines, zero version bumps); `.cargo/config.toml` alias `cargo xtask`.
- `check-toolchain --root`, `check-links`, `check-adrs` ported; the three Python files deleted in
  the same change. Differential runs against the Python before deletion: check-links 19 cases +
  whole repo byte-identical stdout/stderr; check-adrs 11 mutations of `adr/` 11/11 identical;
  check-toolchain 12 mutations 12/12 identical. Two deliberate differences: the predecessor
  monorepo escape hatch in check-links is gone (no ancestor has `scripts/check-monorepo.sh`), and
  `[x](   )` is skipped where the Python raised `IndexError` (`check-links.py:33`).
- Repo root at runtime is the nearest ancestor whose `Cargo.toml` has `[workspace]`
  (`xtask/src/repo.rs`), not the binary's compile-time path.
- Tests written failing-first (`2 passed; 26 failed` on `unimplemented!()` stubs) →
  `cargo test -p xtask --locked`: 28 passed. `bash scripts/gate.sh`: passed, 13 steps.
- `gate.sh:18,19,27`, `AGENTS.md` § The gate, `README.md` § Build, `STATUS.md` (three links to the
  deleted files) updated. Remaining: steps 3–6 (packager, runtime vectors, `render-bundle 0.5.0`).

## Progress — 2026-08-30, step 3

- `cargo xtask package-bundle <version> --out <dir> [--force] [--source-date-epoch] [--contracts-root]`
  (`xtask/src/package.rs`): hand-written ustar headers matching `tarfile.USTAR_FORMAT` byte for
  byte, `serde_json` pretty output post-escaped to `json.dumps(ensure_ascii=True)`, epoch from
  `git log -1 --format=%at -- .` or the flag. Differential against the Python on 0.4.0 before
  deletion: identical stdout (`sha256:3758e80b…`, archive `sha256:91fb5524…`, 880,640 bytes),
  `diff -r` empty, modes identical, eleven refusal cases with identical exit code and text.
- 21 packager tests ported (49 in the crate); failing-first on a timestamp-embedding stub
  (`45 passed; 4 failed`) then green. Both Python files deleted; `gate.sh` drops the packager
  test line (it runs under `cargo test --workspace`); gate 12 steps, passed.
- Remaining: steps 4–6.


## Progress — 2026-08-30, step 4

- `crates/substrate-daemon/tests/runtime_vectors.rs` (1,730 lines, one `#[tokio::test]` mirroring
  the Python's `main()` in order) replaces `scripts/check-runtime-vectors.py`, which is deleted.
  Hand-written HTTP/1.1 over `tokio::net::UnixStream` and raw WebSocket frames; the lane switch is
  `SUBSTRATE_VECTORS_CGROUP_ROOT` where the script took `--cgroup-root`, and an unset value keeps
  the portable branch asserting `exec.sandbox-unavailable` 501 with the delegated cases absent,
  never counted (invariant 3).
- Zero new crates; `Cargo.lock` byte-identical. `nix` gains its test-only `signal` feature
  (`crates/substrate-daemon/Cargo.toml:48-50`).
- Differential against the Python before deletion: portable lane both `27 HTTP cases, startup
  refusal, dual-daemon refusal`; delegated lane both `38 HTTP cases` under
  `systemd-run --user -p Delegate=yes --scope`; two negative cases (`SUBSTRATE_ALLOW_UID` set,
  non-delegated cgroup root) fail identically on both. Failing-first:
  `not implemented: cases 2..27 are not ported yet` → `0 passed; 1 failed`.
- Two deliberate improvements: the temp dir is forced to 0700 (Rust's `TempDir` is 0777 & ~umask
  and the daemon refuses it), and the case counts are now asserted (`PORTABLE_CASES=27`,
  `DELEGATED_CASES=38`) rather than printed.
- The gate keeps no separate step: `cargo test --workspace --locked` runs it. Gate 11 steps,
  passed; `cargo test --workspace`: 17 suites, 205 passed, 0 failed.
- **The gate now runs no Python except the frozen bundles' own renderers and checkers.** Remaining
  for this story: step 5 (`render-bundle 0.5.0` from `substrate-wire` types) and step 6.
