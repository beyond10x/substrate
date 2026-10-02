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
