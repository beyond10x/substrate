---
format: aep.planning-md/3
id: story:every-success-result-fits-the-ledger-before-its-side-effect
kind: story
status: draft
title: Every success result fits the ledger before its side effect
tags:
- security
scope:
- confidence: inferred
  path: contracts/substrate-wire/0.28.0
- confidence: inferred
  path: crates/b10x-substrate-sdk
- confidence: cited
  path: crates/substrate-daemon/src/app/execs.rs
- confidence: inferred
  path: crates/substrate-daemon/src/app/operations.rs
- confidence: cited
  path: crates/substrate-daemon/src/app/processes.rs
- confidence: inferred
  path: crates/substrate-daemon/src/app/responses.rs
- confidence: cited
  path: crates/substrate-daemon/src/app/sessions.rs
- confidence: cited
  path: crates/substrate-daemon/src/app/workspaces.rs
- confidence: inferred
  path: crates/substrate-daemon/tests/http.rs
- confidence: inferred
  path: crates/substrate-daemon/tests/processes.rs
- confidence: inferred
  path: crates/substrate-host/src/probe.rs
- confidence: cited
  path: crates/substrate-store/src/operations.rs
- confidence: inferred
  path: crates/substrate-store/src/tests.rs
- confidence: inferred
  path: crates/substrate-wire/src/lib.rs
- confidence: inferred
  path: scripts/gate.sh
- confidence: inferred
  path: xtask/bundle-source/0.28.0
- confidence: inferred
  path: xtask/src/bundle.rs
- confidence: inferred
  path: xtask/src/bundle/result_bound.rs
revision: 5
---
# Story: every success result fits the ledger before its side effect

## Outcome

Every mutation whose success result the operation ledger stores is decided against the bytes
the ledger reserves for a success result (`StoreConfig::success_result_json_bytes`) before the
mutation's side effect, and the result is held to the reserve at completion. A completion never
fails for size, and an accepted row always ends terminal.

## The rule

ADR 0053 states it. For each operation kind one of two things holds:

- the widest result of any admitted request is under the published bound, with a figure and a
  test that holds it; or
- the daemon refuses, before dispatch, a request for which no result can fit:
  `operation.result-limit`, class `exhausted`, HTTP 429, not retriable, address `result`.

A mutation of an existing resource (a signal, a stop, a lease renewal, a retire, a destroy) is
never refused for this cause: the start decided every later stored form of its resource. At
completion the daemon measures the bytes it stores and cuts nothing. A result over the reserve
ends the row terminal with the error `operation.result-unrecorded` (class `failed`, HTTP 500,
not retriable), the row names the resource, and the resource's record is stored whole. The v2
file changes keep `workspace.write-limit` and `fit_json_bytes`.

## What changes

- The wire crate states the bound, `OPERATION_RESULT_MAX_JSON_BYTES`, the two codes, a constant
  for every result member a driver, the configuration or the identity sets, and one
  `widest_json_bytes` function per result type: `Exec`, `PipeSession`, `ProcessRecord`,
  `Workspace`, `FileObservation`, `FileAbsence` and the three absences.
- The capability snapshot publishes the fact `operation.result-max-json-bytes`.
- Decided before dispatch by the refusal: an exec start, a session start (by the exec input it
  embeds), a host-process start, a v1 file write and a v1 file delete.
- Fits, with its figure held by a test: a workspace create, adopt and lease renewal
  (`Workspace`, 110,879 bytes); a session signal and lease renewal (`PipeSession`, 3,899 bytes);
  an exec retire, a session retire and a workspace destroy (an absence, 126 bytes).
- Fits by the start's decision: an exec signal and lease renewal; a host-process stop, each
  stop of stop-by-owner, a lease renewal and the daemon's own completions.
- Successor bundle `0.28.0` publishes the fact, the refusal with one vector per decided kind,
  the error with one vector, and the missing `maxLength`s. The contract pair moves to it in the
  release that turns the refusal on.

## Design

ADR 0053, "a mutation is refused before dispatch when its success result cannot be recorded"
(invariant 8: accepted before code). It adds one refusal code, one error code, one capability
fact and bundle `0.28.0`. No route is added, and no frozen bundle byte moves. The record
answers its four questions that had more than one answer: the completion rule (the operator's
session, D21 B), the form of the fact, the session start and the code of the v1 file routes.
`approval-record:operator-session-decides-the-result-bound-and-the-completion-rule-2026-11-24`
quotes the two decisions.

## Acceptance

All cases are in process: no daemon, no confined run and no host process is started. Each is
expected to end within 1 s and is timed before it is added. Bounds: 60 s a test, 120 s the test
step, 10 minutes a job in the build pool.

- Store: `the_default_reserve_is_the_published_result_bound`.
- Wire crate, beside `file_text_bound_tests.rs`: the widest `Exec`, `ProcessRecord` and v1 file
  results bound every stored form the serializer writes for their input; the widest
  `Workspace`, `PipeSession` and absence are the figures above and under the bound; every
  member a driver sets is counted at its wire constant.
- Daemon, one case per decided kind (exec start, session start, host-process start, v1 file
  write, v1 file delete): a request for which no result can fit is answered 429
  `operation.result-limit`; no driver was called, no resource row and no lease exist, and the
  same operation id replays the refusal.
- Daemon: a start whose widest result is exactly the reserve completes, and every later
  mutation of that resource ends terminal and replays (an exec: signal, lease renewal; a host
  process: stop, stop-by-owner, lease renewal, exit and expiry).
- Daemon: a request another published code refuses keeps that code.
- Daemon: a driver answer with a member over its bound ends the row terminal with
  `operation.result-unrecorded`; the resource reads back whole and the same id replays the
  error.
- Daemon: a stop is never refused for the result bound.
- The four cases of `tests/http/driver_answer_bound.rs` pass unchanged.
- `cargo xtask check-bundle 0.28.0` refuses a moved literal, code or route count, and
  `scripts/gate.sh` runs it.

## Scope

- **The reserve:** `StoreConfig::success_result_json_bytes` and
  `DEFAULT_OPERATION_TERMINAL_HEADROOM_BYTES` in `crates/substrate-store/src/lib.rs`;
  `Store::success_result_json_bytes`, `complete_success`, `complete_error` and
  `finalize_operation_accounting` in `crates/substrate-store/src/operations.rs`. The daemon
  opens its state with the default (`crates/substrate-daemon/src/runtime.rs`).
- **The precedent:** `workspace_file_mutation_v2` in
  `crates/substrate-daemon/src/app/workspaces.rs`; `FileMutationResult::widest_json_bytes`,
  `fit_json_bytes`, `json_bytes` and `json_char_bytes` in `crates/substrate-wire/src/lib.rs`.
- **Primary surface:** `crates/substrate-daemon/src/app`.
- **Files and symbols, decided kinds:** `app/execs.rs` (the exec start, between
  `validate_exec_input` and the workspace admission); `app/sessions.rs` (`pipe_session_start`);
  `app/processes.rs` (`process_start`, `plan_start`); `app/workspaces.rs`
  (`workspace_file_write`, `workspace_file_delete`); `app/operations.rs`
  (`validate_exec_input`, `refuse_before_dispatch_response`).
- **Files and symbols, completion:** `app/operations.rs` (`finish_success`,
  `finish_exec_leased`); `app/processes.rs` (`complete_stop`, `accept_stop`,
  `conclude_open_start`, `wire_record`); the store's completions of a lease renewal and a
  retire in `crates/substrate-store/src/leases.rs`, `execs.rs`, `sessions.rs` and
  `host_processes.rs`; `app/responses.rs` for the two answers.
- **Wire crate:** `crates/substrate-wire/src/lib.rs` (constants, `CapabilityFacts`, the width
  functions) and a hand-written module for `ProcessRecord`, whose type in
  `generated/host_process.rs` is generated and not edited.
- **Host driver:** `crates/substrate-host/src/probe.rs` (the fact) and the places the
  configuration is read (the bounds of ADR 0053 decision 7).
- **Bundle:** `xtask/bundle-source/0.28.0/`, `contracts/substrate-wire/0.28.0/`, a new
  `xtask/src/bundle/result_bound.rs` dispatched from `xtask/src/bundle.rs`, `scripts/gate.sh`,
  `ADVERTISED_CONTRACT_BUNDLE` and its digest, `crates/b10x-substrate-sdk`.
- **Tests:** a new module of `crates/substrate-daemon/tests/http.rs` after
  `tests/http/driver_answer_bound.rs` (a store with a small headroom, a driver stub that counts
  its calls); `crates/substrate-daemon/tests/processes.rs` with `Harness::faked()`; a module
  beside `crates/substrate-wire/src/file_text_bound_tests.rs`; `crates/substrate-store/src/tests.rs`;
  the runtime vectors and their lane counts.
- **Gate duties:** a change to `crates/substrate-wire/src/lib.rs` runs
  `cargo test --release --locked --lib -p b10x-substrate-wire -p b10x-substrate-daemon
  -p b10x-substrate-sdk` (the new codes are wire constants the daemon's
  `the_refusal_codes_are_the_wire_constants` counts); a new file under
  `crates/substrate-daemon/src/` runs `--test driver_port`; a change under `xtask/src/` runs
  the `xtask` package's targets as `AGENTS.md` § Public website states.
- **Documents:** `CHANGELOG.md`, `website/docs/reference/contract.md`, `AGENTS.md`
  (invariant 6, the gate section, the lane counts, and the list of in-collection consumers of
  the wire crate in § Releases).
- **Would collide with:** any unit that touches exec-start validation (`app/operations.rs`,
  `app/execs.rs`), the host-process handlers (`app/processes.rs`, `tests/processes.rs`), the
  module list of `tests/http.rs`, `crates/substrate-wire/src/lib.rs`, or the next bundle cut.
- **Size:** more than one wave unit. A natural split: the wire crate's constants and width
  functions with their tests (no answer changes); the daemon's decision and completion rule
  with the bundle cut and the pair (one release).
- **Confidence:** medium. The sites are read. Not read: whether each member a driver sets is
  checked against its bound today, and which runtime-vector lane can execute each new vector.
