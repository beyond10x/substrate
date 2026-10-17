# Scope and evidence of the first ESS projection

## Implemented boundary

`domains/operations.yaml` models accepted-operation rows in one deployment/subject partition,
under the fresh-identity and available-capacity prerequisites. The `AcceptedOperation` name selects
rows admitted through `Store::reserve`; it is a projection of `OperationRecord`, not a new runtime
record. The source enum is `OperationState` in `crates/substrate-wire/src/lib.rs`; stored lowercase
states map to ESS `Accepted`, `Unknown`, and `Terminal`.

| Command | Production source | Observation |
|---|---|---|
| Reserve | `crates/substrate-store/src/operations.rs`, `Store::reserve` | accepted row and persisted operation.accepted event |
| MarkDispatchUnknown | same file, `Store::mark_dispatch_unknown` | unknown row and persisted operation.unknown event |
| CompleteSuccess | same file, `Store::complete_success` | terminal row and operation.terminal event for the generic operation kind |
| CompleteError | same file, `Store::complete_error` | terminal row and operation.failed event |
| Operations | same file, `Store::operation` | scoped identity read; no new list endpoint |

The suite checks 14 synthesized scenarios, including unknown identities, wrong states, all declared
transitions, persisted event payloads and unchanged complete projected rows after refusal.
`crates/substrate-store/tests/ess_operations.rs` is the production adapter and closed step runner.
No runtime entity, wire shape, authorization rule or existing contract byte changes.

## Client CLI presentation projection

`client/client.yaml` (domain `substrate.client`) and its `ess-cli/2` binding `client/cli.yaml`
describe the `b10x-substrate` command line, a client of the socket API through
`b10x-substrate-sdk`. It adds no route, wire field or daemon behaviour. What the green check
establishes:

- Every command path, its arguments, its callable, the callable's input, result and error types,
  and the process globals (`--socket`, `--output human|json`) are typed in the model, and
  `ess specify cli` resolves them. A `service_forward` callable's input carries exactly its
  command's input or its view's parameters, and a view callable's result is that view's row.
- `spec/generated/cli` is the exact `ess generate cli` projection of the binding;
  `cargo xtask check-ess` fails on drift. Its `binding.json` is the leaf list: twelve command paths.

| CLI command | Callable target | Route (`crates/substrate-daemon/src/app/routes.rs`) | Wire type (`crates/substrate-wire/src/lib.rs`) |
|---|---|---|---|
| `machine` | view `Machine` | `GET /v1/machine` | `CapabilitySnapshot` |
| `workspace create` | command `CreateWorkspace` | `POST /v1/workspaces` | `WorkspaceCreateInput` → `Workspace` |
| `workspace show` | view `Workspaces` | `GET /v1/workspaces/{workspace_id}` | `Workspace` |
| `workspace renew` | command `RenewWorkspaceLease` | `POST /v1/workspaces/{workspace_id}/lease/renew` | `LeaseRenewInput` → `Workspace` |
| `workspace destroy` | command `DestroyWorkspace` | `DELETE /v1/workspaces/{workspace_id}` | `WorkspaceAbsence` |
| `exec run` | command `RunExec` | `POST /v1/execs` (`wait: true`), `GET /v1/execs/{exec_id}/output` | `ExecStartInput` → `Exec`, `OutputSlice` |
| `exec show` | view `Execs` | `GET /v1/execs/{exec_id}` | `Exec` |
| `session show` | view `Sessions` | `GET /v1/sessions/{session_id}` | `PipeSession` |
| `session signal` | command `SignalSession` | `POST /v1/sessions/{session_id}/signal` | `ExecSignalInput` → `PipeSession` |
| `operation show` | view `Operations` | `GET /v1/ops/{operation_id}` | `OperationRecord` |
| `metrics` | view `Metrics` | `GET /v1/metrics` | `MetricsQuery` → `MetricsObservation` |
| `events` | local action `events` | `GET /v1/events` | `EventQuery` → `EventPage` |

What it does not establish:

- **No wire shape.** Each daemon observation is one `observation: Json` member, carried verbatim;
  the released contract bundle remains its only authority. The CLI's runtime check accepts any JSON
  there.
- **No lifecycle.** Every `substrate.client` entity holds one state, `Observed`. The daemon's
  workspace, exec, session and operation states are members of the observation; no transition
  between them is claimed.
- **No conformance.** No suite is committed or run for this domain. `ess verify conform
  synthesize --path spec/client` writes 12 scenarios and one refusal (`ESS-SYNTH-004`:
  `SignalSession/signalled` needs a session, and no command the CLI presents creates one; session
  start is not a CLI command). The CLI's acceptance scenarios run against the shipped daemon in
  the CLI crate's own tests.
- **No refusal inventory.** Every command and view answers the daemon's `ErrorDetail` as the CLI
  error `refused`; which named code answers which request stays in the daemon and the bundle's
  vectors. ESS CLI error codes admit only `[A-Za-z0-9_-]`, so the daemon's dotted code
  (`resource.not-found`) is in the error's `data.code`, never the CLI's own code.
- **Empty workspaces only.** A Git source needs a source authorization the binding does not take.
  No file access, session start or attachment, reconciliation snapshot, exec signal, retire or
  lease renewal, metrics stream, event stream, or exec start without waiting is presented.

What the binding cannot state, and the handler owns (`spec/generated/cli/manifest.json` lists the
twelve handler obligations):

- The socket default. ess-cli has no environment fallback for a global; `--socket` absent means
  the handler reads `SUBSTRATE_SOCKET`, the daemon's own variable. The daemon has no built-in path.
- `exec run` exit status and raw output. The generated runtime exits 0 on success and renders the
  typed result (compact JSON in human mode); it cannot exit with the command's own code or stream
  its bytes. The result's `stdout`/`stderr` are `String`, so non-UTF-8 output has no faithful value
  (an `UNMAPPED:` marker in `client.yaml`).
- Human tables. Human mode is the runtime's compact JSON; a table or one-line rendering is not
  expressible in the binding.

## Workspace adoption projection

`workspaces/` (domain `substrate.workspaces`, ADR 0035) models the store row a workspace is served
from (id, root name, labels, origin, owner), the entry under the workspace root, and the commands
`Create` (only as far as adoption collides with it), `Adopt` and `Destroy`. What the green check
establishes:

- The model validates under ESS 0.60.0, and `workspaces-suite.json` is its exact synthesized suite
  (16 scenarios) plus the five authored scenarios under `workspaces/scenarios/`. Synthesis refuses
  two obligations (`ESS-SYNTH-013`: no view row asserts `.count` of `RootName` or `WorkspaceId`).
- All 21 scenarios pass against ESS's `interpreted` reference. That executes the model, not the
  daemon.
- `RootName` and `Origin` are generated into `crates/substrate-wire/src/generated/workspace.rs`
  and used as the wire types of `WorkspaceAdoptInput::root_name`, `Workspace::root_name` and
  `Workspace::origin`. The generated types are structural; the alphabet, length and leading-`-`
  rule are enforced by `substrate_wire::is_root_name`, which the daemon's adopt admission and the
  host's `validate_root_name` both call.

What it does not establish, and where that evidence lives:

| claim | evidence |
|---|---|
| refusal order, ledger record and replay of every adopt refusal, the out-of-set name included | daemon tests `adopt_*` in `crates/substrate-daemon/src/app/tests.rs` |
| an exec runs in an adopted directory and its output and exit are observed | same, `adopt_then_exec_runs_in_the_operator_directory` |
| no removal path deletes an adopted directory (sync destroy, restart cleanup, lease expiry, reconciliation) | daemon and store tests named `*adopted*` |
| the `origin` column migration (existing rows are `created`) | `crates/substrate-store/src/tests.rs` |
| wire bundle shapes for the adopt route and the two new resource members | the successor bundle `0.19.0` and its runtime vectors, cut separately |

No production adapter executes `workspaces-suite.json` against the daemon yet.

## Explicitly outside this projection

- Refused-before-admission operation rows, replay/conflict behavior, ledger capacity, crash recovery,
  optional attribution, timestamps and response payloads. The existing native store tests still cover
  these. They are not claims made by this ESS suite.
- Workspace, exec, session, lease, snapshot and capability lifecycles; filesystem, cgroup, quota,
  aperture, secret-descriptor and transport enforcement. The existing portable/delegated runtime
  suites and immutable bundle checks remain required. ESS adoption here is incremental, not complete
  formal coverage of Substrate.
- Product policy, agent loops and fleet scheduling, which are outside Substrate itself.

## Compiler finding

An attempted full Operation lifecycle with an independent `Refused` creation state was refused by
ESS 0.50.0: `[unreachable_state] entity substrate.operations.Operation.states.Refused: Refused cannot
be reached from Accepted`, despite `creates: ...` with `into: Refused`. There is no Accepted→Refused
transition in the implementation; none was invented to satisfy the compiler. The accepted-row
projection avoids making a claim about those independently inserted rows. Extending it requires ESS
to admit disconnected creation states or an explicitly reviewed projection for refused rows.

## Negative control

During adoption, the production SQL in `Store::mark_dispatch_unknown` was temporarily changed to
keep `state = 'accepted'`. The same suite failed at `AcceptedOperation/state/Unknown/refuses/MarkDispatchUnknown`: the corrupted row admitted a second dispatch-unknown transition instead of refusing it. The SQL
was restored, and the suite rerun. This proves the adapter reads production state rather than simply
repeating the expected outcome. The full adoption report records the command and result.
