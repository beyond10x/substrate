# Workspaces: root names and adoption

This ESS projection gives two nouns their typed home and models the one operation the daemon lacks
to serve a directory it did not create: `workspace.adopt` (ADR 0035, superseding ADR 0034). It is
separate from the accepted-operation ledger and the capture projections, so it changes neither
suite's obligations.

`cargo xtask check-ess` validates it, regenerates `../workspaces-suite.json` (18 synthesized and 5
authored scenarios) and compares bytes, runs that suite against ESS's `interpreted` reference, and
regenerates `RootName` and `Origin` into `crates/substrate-wire/src/generated/workspace.rs`, which
`substrate-wire` exports as `RootName` and `WorkspaceOrigin`. The interpreted run checks the model,
not the daemon; the daemon's native tests (`crates/substrate-daemon/src/app/tests.rs`, the
`adopt_*` cases) and the store's (`crates/substrate-store/src/tests.rs`) carry the runtime claims.

## Types (`identifier.yaml`)

- `substrate.workspaces.RootName` is the single-component root-name set `validate_root_name`
  admits (`crates/substrate-guarded/src/fs.rs`): ASCII alphanumerics, `_` and `-`, 1 to 255
  characters (Linux `NAME_MAX`), and no leading `-`. `.`, `..`, `/`, NUL and hidden names are
  outside the alphabet. It was `WorkspaceId` in the ADR 0034 cut. It is renamed because the wire
  workspace id stays `ws_…`, so this set names a directory, not a workspace. Its generated Rust
  type is `substrate_wire::RootName`, and its length bound is the wire constant
  `MAX_ROOT_NAME_BYTES` (`MAX_WORKSPACE_ID_BYTES` in that cut).
- `substrate.workspaces.WorkspaceId` is the unchanged wire `workspace-id`, `^ws_[A-Za-z0-9]+$`.
  The model's alphabet has to hold the prefix's `_`, so it also admits a `_` after the prefix,
  which the wire pattern refuses.

Under ESS 0.80.0 the generated `RootName` decoder checks the alphabet and the length bound; the
leading-`-` rule and the other invariants stay runtime obligations. The adopt input decodes its
root name without that check, so the daemon's admission refuses it and records the operation.
The daemon's admission is the evidence for these rules, not this model.

## Adoption (`adoption.yaml`)

`Workspace` is the store row (`workspaces` table: subject, id, root_name; unique root name per
deployment) with an `origin` column of `created` or `adopted`. `RootEntry` is what sits at
`<workspace_root>/<root_name>`. No command creates a RootEntry: the operator places it, a scenario
establishes it with a `setup:` row, and a conformance target makes it with `mkdir`.

- `Adopt` takes `root_name` and `labels`. It takes no lease and no storage limit, because the
  directory belongs to the operator. It refuses in this order:
  - a root name another subject holds: `resource.not-found`, the same answer as an absent
    directory, so adopt is no cross-subject existence oracle;
  - a root name the caller already holds: `workspace.already-exists`;
  - a missing entry: `resource.not-found`;
  - an entry that is not a directory, or one the daemon's uid does not own:
    `workspace.path-escape`, the code `observe_workspace` gives create
    (`crates/substrate-guarded/src/fs.rs:147`);
  - a full subject partition: `workspace.capacity`.

  All of these are declared `external:` (see the limits below). Otherwise the outcome writes a
  Ready row with a minted `ws_…` id, the root name, the labels, origin `adopted` and the caller as
  owner. In 0.19.0 the wire workspace resource publishes `root_name` and `origin`, read-only.
- `Destroy` gives `resource.not-found` for an absent workspace and for another subject's.
  `workspace.not-ready` answers from Destroying, and `workspace.execs-active` when an exec is
  live. Otherwise an adopted row is forgotten (deleted, directory left in place) and a created row
  moves to Destroying, as today. Every other removal path (restart cleanup, lease expiry,
  reconciliation) also only forgets an adopted record. Those paths are outside this projection.
- `Create` is modelled only as far as adoption collides with it: a created row holds a root name.

A root name outside `RootName` is refused by the bundle's input schema (`request.schema-invalid`)
and recorded in the ledger. ESS declares no outcome for an input outside its own type, so that
refusal is the daemon test's to assert.

## Scenarios

Synthesis writes 18 scenarios. Five more are authored under `scenarios/`. All 23 pass against
ESS 0.80.0's `interpreted` target, which checks the model, not the daemon.

| file | claim |
|---|---|
| `adopt-succeeds.yaml` | Ready row, minted id, root name, origin adopted, caller as owner |
| `adopt-absent-directory.yaml` | `resource.not-found`, no `WorkspaceAdopted` |
| `adopt-twice-same-subject.yaml` | `workspace.already-exists`, first row unchanged |
| `adopt-twice-other-subject.yaml` | `resource.not-found` for another subject, first row unchanged |
| `destroy-adopted-leaves-directory.yaml` | row forgotten, no `DestroyStarted`, RootEntry still present |

## Known limits of this projection

- `Destroy/execs-active` is declared `before_row: true`. The store reads the exec table for the
  addressed workspace, but ESS answers a branch the stored row selects before an unguarded
  external one, and refuses `external:` beside `when_subject:`. The observable order is the same:
  the exec read is subject-scoped, so an absent workspace or another subject's has no exec to
  find. `exec.start` also refuses a workspace that is not Ready.
- ESS refuses `caller` inside a row-set selector (`ESS-COMMAND-003`, `unobservable_fact`). So the
  owner-split collision checks cannot be `when_related:` guards over the Workspace rows, and they
  are external. Synthesis therefore does not witness root-name uniqueness; the two authored
  double-adopt scenarios carry it.
- An earlier revision guarded the collision with a row-set selector. ESS's swapped-caller
  synthesis re-ran it with the callers swapped on the same root name, which fails for a key that
  is unique across subjects (beyond10x/ess#580). Revisit the external declarations when an ESS
  release fixes it.
- The out-of-set name has no authored scenario: every Adopt refusal is external, so an act that
  names no outcome may claim nothing (`ESS-AUTHOR-037`). The daemon test asserts it, and so does
  the clean-room runner against the shipped binary
  (`crates/substrate-daemon/tests/runtime_vectors.rs`, `check_adopted_workspace`).
- Exec is outside this projection. That an exec runs in an adopted workspace, and that the
  confinement floor around it is the created workspace's, are runtime claims: the clean-room
  runner's delegated lane runs an exec and its isolation cases against an adopted workspace.
- `RootName` stays unscenarioed (`ESS-SYNTH-013`): a view row does not assert
  `.count` in this suite format.
- Lease freezing, Expired and Unknown states, and destroy completion are not modelled. They stay in
  the native store and runtime suites.
