---
format: aep.planning-md/1
id: story:tooling-moves-to-cargo-xtask
kind: story
status: implemented
title: 'Anything that runs is Rust: the gate''s Python moves to cargo xtask'
summary: 'atlas AGENTS.md section Language: gates, checkers, renderers and packagers are Rust. 23,916 lines of Python beside 30,768 of Rust; the four frozen bundle renderers stay as reproducibility proofs, everything else moves, and 0.5.0 is rendered from substrate-wire types.'
owner: substrate
tags:
- build
- rust
relations:
- decomposes: epic:release-hardening
revision: 11
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

## Progress — 2026-08-30, steps 5 and 6

**Step 5 done, and step 5's own premise was wrong.** `cargo xtask render-bundle 0.4.0` reproduces
the frozen bundle byte for byte — `diff -r contracts/substrate-wire/0.4.0 <out>` empty, 200 files,
and packaging the rendered tree yields the manifest digest already checked in,
`sha256:3758e80bc39f1eb03b15c69410608c9ef1d2ba8095c7e707c6988dbb5894ab00`. A test holds that fixed
point (`the_rendered_tree_packages_to_the_released_manifest_digest`), so the renderer cannot drift
from what shipped.

The story said "derives schemas from `substrate-wire` types". It cannot, and the implementation
records that rather than pretending:

- `schemars` is not a workspace dependency (`Cargo.toml:22-59`) and adding derives would mean
  editing `crates/`, so nothing can reflect the types.
- Even with reflection the types are **ahead** of 0.4.0: `ExecStartInput::read_only_roots`
  (`crates/substrate-wire/src/lib.rs:761`) has no 0.4.0 schema at all —
  `grep -rl read_only_roots contracts/substrate-wire/0.4.0/` → 0 hits — and
  `FileEditInput`, `FilePatchInput`, `WorkspaceTree*` and `PathMoveInput` have no 0.4.0 input schema.
- The constraints the schemas state are not on the types. `argv: Vec<String>`
  (`crates/substrate-wire/src/lib.rs:757`) against a schema of `minLength 1 / maxLength 4096 /
  maxItems 256`, whose literals live in `crates/substrate-daemon/src/app/operations.rs:245-248` and
  `crates/substrate-host/src/process.rs:824-826`.

What the wire crate does own is taken from it: canonical hashing for all 11 fixture cases,
cross-checked against `canonical_request_hash_v2`, and 22 bounds constants bound through a `$wire`
splice. Of 200 emitted files (708,672 bytes), 30 are computed whole, 14 are authored with computed
splices, and 156 are authored verbatim.

One derivation is **lost**, and is documented as lost at `xtask/src/render.rs:19-25`: the three
unions in `schemas/vector.json` encode Python dict insertion order (`exec.output-limit-bytes` before
`exec.max-current`), which a sorted-key bundle records nowhere.

Authored source is `xtask/bundle-source/<version>/` — one file per emitted path under `documents/`,
plus `routes.json`, `coverage.json`, `hash-cases.json`, `vector-order.json` and
`executable-vectors.json`. Not `contracts/source/`: every directory under `contracts/` is a released
bundle (invariant 6) and `scripts/contract_json_gate.py:301` classifies JSON beneath one and fails
closed (invariant 7). Rendering into `contracts/` is a named refusal, exit 1.

Failing-first: `render`/`hash_case` stubbed to `unimplemented!()` gave `55 passed; 9 failed`.
Then `cargo test -p xtask --locked` → `64 passed; 0 failed` (49 before). `bash scripts/gate.sh`
passed, 11 steps. `git status --short contracts/` empty.

**Step 6 done.** `AGENTS.md` § *The gate* and `README.md` § *Build, test, run* both carry the verb
table — `check-toolchain`, `check-links`, `check-adrs`, `package-bundle`, `render-bundle` — and both
say why the four frozen `render-contract-bundle*.py` / `check-contract-bundle*.py` pairs stay: they
are the released bundles' reproducibility proof, and 0.4.0's `generator.name` points at one of them,
so deleting it would break the bundle it generated.

**Decision taken by default:** `render-bundle` gets no `gate.sh` step of its own. A test already
asserts the 0.4.0 digest, and `cargo test --workspace --locked` is the gate's first step — the same
reasoning that keeps `package-bundle` and the runtime-vector runner out of it.

**0.5.0 is cut, so that clause is closed.** `story:sealed-secret-slots` rendered
`contracts/substrate-wire/0.5.0/` (206 files) with `cargo xtask render-bundle`, and no Python
renderer was involved or written. It also replaced the ADR's proposed
`scripts/check-contract-bundle-0.5.0.py` with a new verb, `cargo xtask check-bundle <version>`,
which re-renders from the authored source and compares bytes — strictly stronger than a
hand-written checker.

## What is actually left — 2026-08-30

**576 lines, and they are not the frozen renderers.** The acceptance says the gate invokes no Python
except the four frozen renderers' checkers. It still invokes one more:

    scripts/gate.sh:28  run python3 scripts/test_contract_json_gate.py

`scripts/contract_json_gate.py` (377 lines) and its self-test (199) are shared live machinery, not a
per-version frozen artefact — `:301` classifies JSON under a bundle directory and fails closed, and
`:195` already shells out to `crates/substrate-contract-check`, which is Rust. Porting it is the
last step of this story, and it is the one place a new bundle's JSON is admitted or rejected, so it
wants the same differential proof every other move got: the predecessor's negative cases run against
the Rust replacement before the Python is deleted.

`scripts/check-bot-files.py` (39 lines) is not in `scripts/gate.sh` and is out of this story's
acceptance; name it separately if it should move.

Of the 21,298 Python lines left, 20,683 are the four frozen renderer/checker pairs, which stay.

## Progress — 2026-08-30, the last Python leaves the gate

`scripts/contract_json_gate.py` (377 lines) and `scripts/test_contract_json_gate.py` (199) are
deleted; `cargo xtask check-json` replaces them (`xtask/src/json.rs`, 2,292 lines with tests).

Acceptance, checked line by line:

- **No Python in the gate except the four frozen checkers.** `grep -n python scripts/gate.sh` →
  lines 20-23 only, the four `check-contract-bundle*.py`.
- **`bash scripts/gate.sh` → `gate: passed`, exit 0**, 47.4s, re-run by the orchestrator rather
  than taken on report. Its last three steps: `check-bundle 0.6.0` → 213 files;
  `check-json` → `contract JSON classified: 0.1.0 114, 0.2.0 165, 0.3.0 194, 0.4.0 200, 0.5.0 206,
  0.6.0 213 (1092 documents in 6 bundles)`; `check-toolchain` → `Rust toolchain pinned at 1.97`.
- **`0.4.0` still verifies byte-clean** — `substrate-wire 0.4.0: 199 files, 26 closed operations,
  21 executable vectors, 71 design vectors, 112 requirements, and 11 exact hash fixtures verified`.
  `git status --short contracts/` is empty.
- Failing-first: 7 `unimplemented!()` stubs failed (`69 passed; 7 failed`), then **87 passed**.

**Differential before deletion, and again afterwards from an archived copy: 33 of 36 cases
byte-identical** in stdout, stderr and exit code — six untouched bundles plus 30 mutations. The
three that differ, each decided rather than absorbed:

| case | Python | Rust | why |
|---|---|---|---|
| malformed document | `Expecting property name enclosed in double quotes: line 3 column 1 (char 12)` | `EOF while parsing a value at line 3 column 0` | different parsers; same path, prefix, position and exit code |
| `additionalProperties` order | `'b'` then `'a'` | `'a'` then `'b'` | reachable only for a document that already failed the deterministic-source-form check; one that passes it has file order == sorted order |
| cyclic `$ref` | `RecursionError` and a 1,000-frame traceback | named refusal, `MAX_REFERENCE_DEPTH = 64` | **a behaviour change, on purpose**: Python guarded cycles in `dereference_schema` but not on the `validate` path, and a crash is not a refusal (invariant 3) |

**Coverage widened, not narrowed.** The Python classified four bundles — `check_json_authority` was
only ever called by the four checkers, each closing over its own `BUNDLE`, and `0.5.0`/`0.6.0` had
no Python at all. `check-json` covers all six.

**The four frozen checkers changed**, minimally and only where they imported the deleted module:
7 lines each — the `from contract_json_gate import …` line, the call, and the
`N classified JSON documents` field of the summary string. `check_renderer_reproducibility` and
every bundle assertion are untouched, so their role as the frozen bundles' reproducibility proof
(invariant 6) is intact; verified by diff and by the gate.

**Two new direct dependencies, zero new packages** (`Cargo.lock` +2 lines), both already in the
lock under `jsonschema`: `fancy-regex`, because three bundle patterns use negative lookahead that
`regex` cannot compile and one is reached on a released tree; `serde`, because rejecting duplicate
object keys needs a `Deserialize` visitor. `fancy-regex`'s `delegate_size_limit` is raised to
256 MiB so `common.json`'s `[A-Za-z0-9+/]{1398102}==` compiles.

**Dropped on purpose:** the subprocess-protocol refusals at the old `contract_json_gate.py:206-218`
— a 16 MiB pipe ceiling and label/URI uniqueness. The standards pass runs in process now; the pipe
they guarded does not exist and uniqueness holds by construction.

`git grep contract_json_gate` finds one hit, `docs/reviews/archived/2026-08-13-…md:80`, which is
immutable review input and correctly left alone.
