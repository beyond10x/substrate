---
status: accepted
date: 2026-11-24
---

# ADR 0053: a mutation is refused before dispatch when its success result cannot be recorded

Decider: the operator's session, 2026-11-24 (D20, option B: the most correct form, also where
it takes longer; D21, option B: decision 8). Decides
`story:every-success-result-fits-the-ledger-before-its-side-effect`. The design comes before code
(invariant 8): this record adds a refusal code, an error code, a capability fact and a successor
bundle, and no code yet. Three points were decided with D20 B: a new code names the cause, the
bound is published in bundle `0.28.0`, and this text merges before any code. D21 B decided what
a completion over the reserve does. The other decisions follow from these and from the code
they cite; *Questions answered with this record* lists the four that had more than one answer.

## Context

An operation is durable before its driver is dispatched
([ADR 0005](0005-operations-are-durable-before-driver-dispatch.md)). The ledger charges every
accepted row a fixed number of bytes for its terminal form,
`StoreConfig::operation_terminal_headroom_bytes` in `crates/substrate-store/src/lib.rs`, and a
completion that needs more than the row was charged is not written
(`finalize_operation_accounting` in `crates/substrate-store/src/operations.rs`,
`StoreError::OperationTerminalHeadroom`). `StoreConfig::success_result_json_bytes` states what
that leaves for a success result: the headroom less the 71 bytes of the terminal columns, in
bytes of compact JSON. Its documentation gives the duty: "A caller holds a result to this before
it completes, so the completion cannot fail for size."

The three v2 file changes hold their result to it. `workspace_file_mutation_v2` in
`crates/substrate-daemon/src/app/workspaces.rs` compares
`FileMutationResult::widest_json_bytes(workspace, path)` with the reserve before the workspace
is admitted and refuses `workspace.write-limit` when no result can fit. At completion
`fit_json_bytes` cuts the one member that has a published flag for it, the diff text with
`truncated`.

This record states that rule for every kind whose success result the ledger stores, and says
what it means for a result type that has no member to cut.

## Decision

1. **The rule.** Every mutation whose success result the operation ledger stores is decided
   against the bytes the ledger reserves for a success result before the mutation's side
   effect, and the result is held to the reserve at completion. A completion never fails for
   size, and a row that was accepted always ends terminal.

2. **One code names the refusal: `operation.result-limit`.** Class `exhausted`, HTTP 429,
   `retriable: false`, `address: "result"`. The message is one fixed sentence without a figure:
   "The result of this operation exceeds what the operation ledger can record." It is recorded
   as refused before dispatch (`refuse_before_dispatch_response` in
   `crates/substrate-daemon/src/app/operations.rs`): no row is accepted, no resource row and no
   lease is written, no driver is called, and the same operation id replays the refusal.
   - *One code for every kind.* The cause is the ledger's reserve, not a property of an exec, a
     process or a file. A client handles it in one branch, and a kind added later needs no new
     code.
   - *The name.* It stands beside `operation.ledger-capacity` (507, the ledger is full) and
     `request.body-limit` (429, the request is too large). `-limit` is the suffix of the
     bundle's byte bounds. A lexical code of one member (`exec.workdir-invalid`,
     `process.argv-invalid`, `request.schema-invalid`) is rejected by D20 B: each member of a
     refused request passes its own published rule.
   - *The class and status.* `exhausted` and 429 are what `request.body-limit` and
     `workspace.write-limit` answer, and what `driver_detail` maps `Exhausted` to.
   - *`workspace.write-limit` keeps its meaning.* The v2 file routes keep answering it for
     this cause, address `limit`. Two codes for one cause is the price of not moving a released
     answer.
   - `process.start-input-bounds` is a coverage requirement of the bundle, not a code. The
     host-process codes for a start's bounds are `process.argv-invalid` and
     `process.env-invalid` (422), and they keep their meaning.

3. **Where the decision stands.** It is the last check that needs no state: after every lexical
   and capability check of the kind (`validate_exec_input`, the session start's own checks,
   `plan_start`, the path checks), and before workspace admission, the lease, the reservation
   and the dispatch. A request another published code refuses keeps that code.

4. **The bound is a wire constant and a capability fact.**
   - `substrate_wire::OPERATION_RESULT_MAX_JSON_BYTES: u64 = 524_217`, beside the four
     `OPERATION_LEDGER_*` constants in `crates/substrate-wire/src/lib.rs`. A store test holds
     `StoreConfig::default().success_result_json_bytes()` to it.
   - The fact `operation.result-max-json-bytes`, required, `const` 524217, beside the four
     `operation.ledger-*` facts in `schemas/capability.json`, filled by
     `CapabilityFacts::default` and the host driver's probe as they are.
   - *An operator cannot set the reserve.* The daemon opens its state with
     `Store::open_with_event_retention` (`crates/substrate-daemon/src/runtime.rs`), which takes
     the default headroom. No daemon flag, `HostConfig` member or chart value names it; only
     tests call `Store::open_with_config`. A state database also refuses to open under another
     accounting fingerprint (`validate_store_config` in `crates/substrate-store/src/schema.rs`).
   - *Both, because there are two kinds of client.* A Rust client takes the constant with the
     wire crate. A client that reads only the bundle takes the fact. The `facts` object is
     closed (`additionalProperties: false`), so the fact needs a successor bundle.
   - The daemon decides against its store's own figure, `Store::success_result_json_bytes`,
     which is what makes the completion safe. A daemon as shipped holds that figure equal to
     the constant by a test.

5. **What a client can compute before it sends.** With the wire crate: the width function of
   decision 6 over its own input, compared with the constant. Without it: the widest result
   built from the published result schema. Members the request sets are written as sent, with
   JSON escapes. Every other string is counted at 6 bytes for each character of its
   `maxLength`, every integer at its longest decimal form, and every optional member as
   present. A request under the bound by that count is never refused `operation.result-limit`.

6. **How the width is counted.** The width is the number of bytes of the compact JSON form of
   the widest result the request can lead to, counted as the row's serializer writes it:
   - a string the request sets costs `json_char_bytes` for each character, as sent;
   - a member the request does not set is counted at its widest admitted value: present, a
     string at 6 bytes for each character its bound admits (1 byte where only ASCII is admitted: an
     identifier, the name of a digest, a label key, a root name), an integer at the longest decimal
     form of its type (20 digits for a 64-bit one), a boolean as `false`, a timestamp at 35
     characters, an identifier the daemon mints at 32 characters, an enumeration at its longest
     name, a list at its bound;
   - the figure covers every later stored form of the same resource, so a mutation of that
     resource never needs a decision of its own. For an exec: every state, `exit`, `usage` in
     its widest variant, `lease`, `refusal` and `unrecorded_output`. For a host process: every
     state, `exit`, `ended_by`, `expires_at`, `deadline` and the lease with its `keep_reason`. A
     stop sets no text.

   Each result type gets an associated function in the wire crate, named after the precedent:

   | Result type | Function | Takes |
   |---|---|---|
   | `Exec` | `Exec::widest_json_bytes` | the `ExecStartInput` |
   | `PipeSession` | `PipeSession::widest_json_bytes` | nothing; its `workspace` and `capability_snapshot` are counted as a minted identifier and the name of a digest, which they are once the start is validated and its workspace admitted |
   | `ProcessRecord` | `ProcessRecord::widest_json_bytes` | the start's `argv`, `cwd`, environment names, `owner` and `reason`; `cpu`, `memory` and `listening` are left out, because a read observes them and no stored record holds them |
   | `Workspace` | `Workspace::widest_json_bytes` | nothing; counted at the label and root-name bounds |
   | `FileObservation` | `FileObservation::widest_json_bytes` | the workspace id and the path |
   | `FileAbsence` | `FileAbsence::widest_json_bytes` | the workspace id and the path |
   | `ExecAbsence`, `SessionAbsence`, `WorkspaceAbsence` | `widest_json_bytes` on each | nothing |
   | `FileMutationResult` | `FileMutationResult::widest_json_bytes` | unchanged |

   `ProcessRecord` is generated from `spec/` and is not edited by hand; its function is an
   inherent `impl` in a hand-written module of the wire crate.

7. **Every member a driver, the configuration or the identity sets has a wire constant.**
   Bundle `0.27.0` already publishes a `maxLength` for each of them in `schemas/resource.json`
   and `schemas/common.json`, except `refusal.code`: it publishes two codes as a `const` each
   (`exec.aperture-byte-limit` and `exec.workdir-missing`) and no length, and bundle `0.28.0`
   adds one (decision 11). The wire crate gains the constant, the width function counts the
   member at it, and `cargo xtask check-bundle 0.28.0` holds the published literal to the
   constant.

   | Member | Published bound | Set by | Held where |
   |---|---|---|---|
   | `applied.cgroup` | 512 characters | driver, from the cgroup root | at the driver's open |
   | applied aperture `destination`, `listen`, `name` | 64, 21, 64 | configuration | where an aperture is declared |
   | applied writable root `host_path` | `host-absolute-path`, 4,000 | configuration or the Git checkout | where a root is declared or materialized |
   | `usage` unavailable `code`, `message` | 128, 2,048 | driver | at the daemon's driver port |
   | `refusal.message` | 512 | driver | at the daemon's driver port |
   | `refusal.code` | none in `0.27.0`; 128 from `0.28.0` | driver | at the daemon's driver port |
   | process `exit.signal` | 32 | supervisor | at the daemon's driver port |
   | `lease.actor`, `lease.principal` | 256 each | identity | at admission |

   Each member is held to its bound at the place its row names, and the code story proves each
   row with a test. A configuration that can produce a member over its bound refuses the
   daemon's start by name. A member is never shortened.

8. **At completion the stored bytes are measured, and nothing is cut.** Every completion
   function (`finish_success`, `finish_exec_leased`, the host-process completions in
   `crates/substrate-daemon/src/app/processes.rs`, and the store's own completions of a lease
   renewal and a retire) compares the bytes it is about to store with the reserve. By decisions
   3, 6 and 7 no result of a start decided under this rule exceeds it. The measure covers what
   those decisions do not: a member over its published bound, and a record whose start was not
   decided under this rule. The row then ends terminal with an error outcome, never `unknown`:
   - code `operation.result-unrecorded`, class `failed`, HTTP 500, `retriable: false`,
     `address: "result"`, message "The operation took effect, but its result exceeds what the
     operation ledger can record. Read the resource.";
   - the row names the resource (`complete_error` takes the resource id), and the same
     operation id replays the same error;
   - the resource's own record is stored whole, and the run is neither ended nor changed
     because of it;
   - the widest such outcome is 394 bytes, so it always fits.

   This is the honesty rule (invariant 3): the client is told by name that the effect happened
   and that the ledger does not hold the result. No member is shortened, no result is
   replaced by a smaller one, and success is not reported for bytes that were not stored. The
   v2 file changes keep `fit_json_bytes`, because their one cuttable member has a published
   flag.

9. **A mutation of an existing resource is never refused for this cause.** A signal, a stop, a
   lease renewal, a retire and a destroy answer `operation.result-limit` in no case: a stop
   that is refused leaves a run nobody can end. They fit because the start decided every later
   form (decision 6), or they fall under decision 8.

10. **Every kind.** "Decided" means refused `operation.result-limit` before the side effect
    when the width function's figure exceeds the reserve. "Fits" means the widest result of any
    admitted request is under the published bound of 524,217 bytes. Each figure is the JSON
    width at the published bounds, counted by the rules of decision 6, and each named test is
    owed by the code story.

    | Kind | Result | Rule | Figure and the test that holds it |
    |---|---|---|---|
    | `exec.start` | `Exec` | decided, `Exec::widest_json_bytes` | `an_exec_start_no_result_can_fit_is_refused_before_any_driver_is_dispatched` |
    | `session.start` | `PipeSession` | fits, and decided by the exec input it embeds, `Exec::widest_json_bytes` | 3,926 bytes, `the_widest_session_result_is_under_the_published_bound`; `a_session_start_no_exec_result_can_fit_is_refused_before_any_driver_is_dispatched` |
    | `exec.signal` | `Exec` | fits by the start's decision | `a_start_at_the_bound_completes_and_every_later_mutation_of_it_completes` |
    | `exec.lease.renew` | `Exec` | fits by the start's decision | the same test |
    | `exec.retire` | `ExecAbsence` | fits | 122 bytes, `the_widest_absence_is_under_the_published_bound` |
    | `session.signal` | `PipeSession` | fits | 3,926 bytes, `the_widest_session_result_is_under_the_published_bound` |
    | `session.lease.renew` | `PipeSession` | fits | 3,926 bytes, the same test |
    | `session.retire` | `SessionAbsence` | fits | 125 bytes, `the_widest_absence_is_under_the_published_bound` |
    | `process.start` | `ProcessRecord` | decided, `ProcessRecord::widest_json_bytes` | `a_process_start_no_result_can_fit_is_refused_before_anything_is_started` |
    | `process.stop`, and each stop of stop-by-owner | `ProcessRecord` | fits by the start's decision, which counts the lease's `keep_reason` at `MAX_PROCESS_REASON_BYTES` | `a_process_started_at_the_bound_is_stopped_and_renewed_and_every_row_completes` |
    | `process.lease.renew` | `ProcessRecord` | fits by the start's decision | the same test |
    | the daemon's own completions of a host process | `ProcessRecord` | fits by the start's decision | `a_process_started_at_the_bound_that_exits_or_expires_completes_its_open_rows` |
    | `workspace.create` | `Workspace` | fits | 110,926 bytes, `the_widest_workspace_result_is_under_the_published_bound` |
    | `workspace.adopt` | `Workspace` | fits | 110,926 bytes, the same test |
    | `workspace.lease.renew` | `Workspace` | fits | 110,926 bytes, the same test |
    | `workspace.destroy` | `WorkspaceAbsence` | fits | 127 bytes, `the_widest_absence_is_under_the_published_bound` |
    | v1 `workspace.file.write` | `FileObservation` | decided, `FileObservation::widest_json_bytes` | `a_v1_file_write_no_result_can_fit_is_refused_before_any_driver_is_dispatched` |
    | v1 `workspace.file.delete` | `FileAbsence` | decided, `FileAbsence::widest_json_bytes` | `a_v1_file_delete_no_result_can_fit_is_refused_before_any_driver_is_dispatched` |
    | v2 replace, edit, patch | `FileMutationResult` | decided, `workspace.write-limit`, unchanged | `a_change_no_result_can_fit_is_refused_before_any_driver_is_dispatched` and three more in `crates/substrate-daemon/tests/http/driver_answer_bound.rs` |

    The `Workspace` figure counts 64 labels at the bounds of `labels_within_bounds`, a root name
    at `MAX_ROOT_NAME_BYTES` and a lease whose actor and principal are 256 characters each. The
    three absence figures are those of `WorkspaceAbsence`, `SessionAbsence` and `ExecAbsence`.
    The figures of the wire crate's tests are held by those tests
    (`crates/substrate-wire/src/result_width_tests.rs`), each as an equality. A v1 file path inside the published `relative-path` bound of 4,096 characters costs at
    most 24,576 bytes, so such a request is never refused for this cause under the published
    bound.

11. **Bundle `0.28.0`.** It is additive, its predecessor is `0.27.0`, and its compatibility
    block states that it adds no route and preserves the 45 routes of `0.27.0`. It adds:
    - the fact `operation.result-max-json-bytes` (decision 4);
    - the refusal `operation.result-limit`, published by one vector per decided kind (an exec
      start, a session start, a host-process start, a v1 file write and a v1 file delete), each
      against a setup machine that states a lower value of the fact, as `write-limit.json`
      lowers `workspace.max-file-bytes`;
    - the error `operation.result-unrecorded`, published by one vector of a completed row;
    - on the five input schemas of the decided kinds, the refusal in the annotation the bundle
      already uses for a member's refusals (`x-b10x-refusals`), and the counting rule of
      decision 5 in the fact's description;
    - a coverage requirement for the bound, beside `security.bounds.input`;
    - a `maxLength` where a result member has none: the five resource identifiers of
      `schemas/common.json` (32), `refusal.code` of an exec (128), and `argv` items, `cwd` and
      `env_names` items of the process record, each at its wire bound. A member the request sets is counted as sent, whatever
      its own published rule bounds.

    `xtask/src/bundle/result_bound.rs`, a module beside `run_bounds.rs`, holds it under
    `cargo xtask check-bundle 0.28.0`: the fact's `const` is `OPERATION_RESULT_MAX_JSON_BYTES`,
    each vector's code, class, status and address are the wire constants, each `maxLength` of
    decision 7 and of this list is its wire constant, and the route counts are those of
    `0.27.0`. The renderer gains no `{"$wire": …}` binding: the sources state literals.
    `scripts/gate.sh` gains `0.28.0` at the end of its `cargo xtask check-bundles` line, and
    `cargo xtask check-json` then reads twenty-eight bundles. No frozen byte moves.

12. **The refusal, the fact and the bundle ship in one release.** The fact cannot be published
    under `0.27.0`, whose `facts` object is closed, and D20 B publishes the bound with the
    code. A daemon that answered the code while it advertised `0.27.0` would answer something
    no bundle it names publishes. So one release moves the contract pair
    (`ADVERTISED_CONTRACT_BUNDLE` and its digest) to `0.28.0` and turns the refusal on. Work
    that changes no answer lands before it: the constants, the width functions and their tests
    in the wire crate.

## Compatibility

Clients pin the SDK or the wire crate by Git revision ([ADR 0030](0030-rust-crates-are-source-distributed-and-non-publishable.md)),
so no client changes until it moves its pin. For a client of a daemon that advertises `0.28.0`:

- A start of an exec, a session or a host process, and a v1 file write or delete, whose widest
  result by decision 6 exceeds the published bound is answered 429 `operation.result-limit`,
  although each of its members passes its own published rule. Nothing was started and no row was
  accepted. The request is not retried unchanged.
- Every other request is answered as before. A request inside the bound by the count of
  decision 5 is never refused for this cause.
- A terminal operation may carry the error `operation.result-unrecorded`. The client reads the
  resource the row names. It does not repeat the mutation under a new operation id.
- A client that matches refusal codes by a closed list adds two codes. A client that validates
  a capability snapshot against the bundle's schema needs the schema of `0.28.0`.

## Alternatives

- **A lexical code of one member**, or `request.schema-invalid`. Rejected by D20 B: the request
  passes the schema the bundle publishes for it.
- **`workspace.write-limit` for every kind.** It is a workspace code and its vector states a
  file limit; an exec start is not a write.
- **One code per resource family** (`exec.result-limit`, `process.result-limit`,
  `workspace.result-limit`). Three names and three client branches for one cause, and a fourth
  with the next family.
- **422.** The request is well formed, and the bound belongs to the machine. 429 with
  `retriable: false` is what the two existing byte bounds answer.
- **A wire constant without a fact.** A client without the wire crate could not read the bound
  from the machine it talks to.
- **A larger reserve instead of a refusal.** The reserve is charged to every accepted row, and
  the headroom cannot exceed the row bound of 1 MiB. A larger figure moves the bound; the
  decision before dispatch is needed with any figure.
- **Cut a member at completion.** Rejected by D21 B: a cut `cgroup` or path names something
  that does not exist, and a result would be reported for bytes that were not stored.

## Consequences

- Two codes name one cause on two route families: `workspace.write-limit` on the v2 file
  routes, `operation.result-limit` everywhere else.
- A member counted at its bound makes the figure larger than the result that is stored, so a
  request can be refused whose real result would have fitted. The count has to be the same
  for the daemon and for a client, and a client cannot know what a driver will set.
- The width functions are held to the serializer by tests, as `file_text_bound_tests.rs` holds
  `FileMutationResult`. A member added to a result type without its count fails those tests.
- `AGENTS.md` invariant 6, the gate section and the lane counts of the runtime vectors move
  with the cut, as with every successor.

## What the implementation owes

- `crates/substrate-wire`: `OPERATION_RESULT_MAX_JSON_BYTES`, the two codes and the member
  bounds of decision 7 as constants, the fact on `CapabilityFacts`, the width functions of
  decision 6, and a test module beside `file_text_bound_tests.rs`.
- `crates/substrate-store`: the test that holds the default reserve to the constant.
- `crates/substrate-daemon`: the decision of decision 3 on the five decided kinds, the measure
  of decision 8 in every completion function, and the checks of decision 7 at the driver port
  and at admission.
- `crates/substrate-host`: the fact in the probe, and the bounds of decision 7 where the
  configuration is read.
- `xtask`: `xtask/bundle-source/0.28.0/`, the rendered `contracts/substrate-wire/0.28.0/`,
  `xtask/src/bundle/result_bound.rs`, and the gate line.
- `crates/b10x-substrate-sdk` and the contract pair, with the cut.
- `CHANGELOG.md`, `website/docs/reference/contract.md` and `AGENTS.md`.

Acceptance cases. All are in process; none starts a daemon, a confined run or a host process.
Each is expected to end within 1 s. None was run or timed for this record, and the code story
times each before it adds it. The bounds are 60 s a test, 120 s the test step and 10 minutes a
job in the build pool.

| Case | Where | Holds |
|---|---|---|
| `the_default_reserve_is_the_published_result_bound` | store tests | decision 4 |
| `the_widest_exec_result_bounds_every_stored_form_of_its_exec` | wire crate | decision 6, every state and variant |
| `the_widest_process_record_bounds_every_stored_form_of_its_process` | wire crate | decision 6 |
| `the_widest_v1_file_results_bound_every_result_for_their_path` | wire crate | decision 6 |
| `the_widest_workspace_result_is_under_the_published_bound` | wire crate | 110,926 bytes |
| `the_widest_session_result_is_under_the_published_bound` | wire crate | 3,926 bytes |
| `the_widest_absence_is_under_the_published_bound` | wire crate | 127, 125 and 122 bytes |
| `every_member_a_driver_sets_is_counted_at_its_wire_bound` | wire crate | decision 7 |
| the five `…_no_result_can_fit_is_refused_before_…` cases of decision 10 | daemon, a module of `tests/http.rs` after `driver_answer_bound.rs`, a store with a small headroom and a driver stub that counts its calls; `Harness::faked()` in `tests/processes.rs` for the process case | 429, the code, no driver call, no resource row, no lease, the same id replays the refusal |
| `a_start_at_the_bound_completes_and_every_later_mutation_of_it_completes` | daemon, the same module | signal and lease renewal end terminal and replay |
| `a_process_started_at_the_bound_is_stopped_and_renewed_and_every_row_completes` | `tests/processes.rs` | stop, stop-by-owner, renewal |
| `a_process_started_at_the_bound_that_exits_or_expires_completes_its_open_rows` | `tests/processes.rs` | the daemon's own completions |
| `a_request_another_code_refuses_keeps_that_code` | daemon | decision 3 |
| `a_driver_answer_over_a_member_bound_ends_the_row_terminal_and_cuts_nothing` | daemon | decision 8: the error, the resource read back whole, the replay |
| `a_stop_is_never_refused_for_the_result_bound` | `tests/processes.rs` | decision 9 |
| `check-bundle 0.28.0` refuses a moved literal, code or route count | `xtask`, cases of `result_bound.rs` | decision 11 |

The four cases of `driver_answer_bound.rs` stay as they are. The runtime vectors gain the cases
that execute the new vectors; which lane can run each was not read.

## Questions answered with this record

1. **What a completion over the reserve does** (decision 8). Answered by the operator's session,
   2026-11-24 (D21, option B): the row ends terminal with the error
   `operation.result-unrecorded`, nothing is cut and the run goes on. The two other options:
   - cut each string over its bound and name the cut members in a new member of the result: a
     new member on four result types, and a cut `cgroup` or path names something that does not
     exist;
   - as decided, and the daemon ends the run of a start it could not record: a driver's defect
     then ends work that was admitted.
2. **The form of the fact** (decision 4): `const` 524217 and required, as the four
   `operation.ledger-*` facts. Nothing sets the reserve: the daemon opens its store with the
   default (`crates/substrate-daemon/src/runtime.rs`), and no setting names it. An integer
   filled from the store's own figure, as `events.retention-events` is filled from a setting,
   belongs to a decision that makes the reserve an operator setting.
3. **A session start takes the exec decision** (decision 10). Its own result fits. The exec
   routes refuse only the retire of a session's exec (`exec.session-owned`, in
   `crates/substrate-store/src/execs.rs`); a signal and a lease renewal on the exec routes
   address it as any exec and store its `Exec` record as their result. So a session start is
   decided by `Exec::widest_json_bytes`, like every start that carries an exec input.
4. **The v1 file routes' code** is `operation.result-limit` (decision 10). On v1,
   `workspace.write-limit` names a file limit only; its meaning does not widen without a new
   name.
