---
status: accepted
date: 2026-10-17
---

# ADR 0035: an operator's directory is served after an explicit adopt

Supersedes [ADR 0034](0034-a-workspace-is-addressed-by-its-root-name.md).

> **Client-visible change.** The workspace resource (`schemas/resource.json`) is a closed schema
> (`additionalProperties: false`). `0.19.0` adds two required, read-only members to it, `origin`
> and `root_name`. A client that validates workspace resources, events or listings against
> `0.18.0` rejects every workspace the `0.19.0` daemon reports, minted ones included, until it
> moves to `0.19.0`. Request shapes and `workspace-id` (`ws_…`) are unchanged.

## Context

ADR 0034 widened the wire `workspace-id` to the single-component root-name set, so that a
directory the operator places under the workspace root could be addressed by its name. It rested
on the premise that such a directory "can be read and written" over the socket and that only
`exec.start` refuses it. The code does not hold that premise.

Every workspace route, `exec.start` and session start pass through the store's `admit_workspace`
(`crates/substrate-store/src/workspaces.rs`). It looks up a row in the `workspaces` table keyed by
deployment, subject and id. Only `workspace.create` writes that row (`reserve_workspace_create`).
A directory the daemon did not create has no row, so it answers `resource.not-found` on every
route, whatever the wire pattern admits. A native test shows it:
`PUT /v1/workspaces/adopted-repo/files/adopted.txt` answers `404 resource.not-found`. The host's
`validate_root_name` is what admits a name without the `ws_` prefix, and the daemon never reaches
it for a directory it did not create.

Widening `workspace-id` is therefore necessary for ADR 0034's route but not sufficient. Something
must give the directory a store row, and the row decides four things:

- **Owner.** Rows are subject-scoped (`PRIMARY KEY (deployment, subject, id)`), and the root name
  is unique per deployment across subjects (`UNIQUE (deployment, root_name)`,
  `crates/substrate-store/src/schema.rs`).
- **Deletion.** `workspace.destroy` removes the directory tree (`destroy_workspace`,
  `crates/substrate-host/src/lib.rs`). For a directory the operator owns, that is data loss.
- **Lease and freeze.** Admission freezes a workspace whose lease is due, and refuses a workspace
  that is not Ready.
- **Reads.** `workspace.get`, events and reconciliation all read the row.

ADR 0034 also left its name refusals out of the operation ledger. Every other pre-dispatch refusal
is recorded as a `refused` operation that a retry with the same operation id replays
(`record_bound_refusal`, `crates/substrate-daemon/src/app/operations.rs`).

## Decision

1. **Adoption is an explicit operation, `workspace.adopt`, at `POST /v1/workspaces/adopt`.** It
   takes two inputs:
   - `root_name`: a member of the root-name set. That is ASCII alphanumerics, `_` and `-`, 1 to
     255 bytes (Linux `NAME_MAX`), no leading `-`, exactly `validate_root_name`'s set. The set
     contains no `.`, so a root name never names a hidden entry or a `.substrate-*` service
     directory. ADR 0034 enforced it through one predicate in `substrate-wire`, which the host's
     `workspace_path` also used; that predicate now guards `root_name`.
   - `labels`: as `workspace.create` takes them.

   Adopt takes no `lease_ttl_ms` and no storage limit, because the directory belongs to the
   operator. The daemon neither expires an adopted workspace by lease nor applies a project quota
   to its directory.
2. **The wire `workspace-id` stays `^ws_[A-Za-z0-9]+$`.** An adopted workspace gets a minted
   `ws_…` id, exactly like a created one, and every route keeps addressing workspaces by that id.
   The root name is the row's `root_name`, which the store already keeps apart from `id`. ADR
   0034's widening of `workspace-id`, its inline `^workspace:` change in
   `results/reconciliation-snapshot-get.json` and the admission changes to `exec.start` and
   session start are withdrawn.
3. **A successful adopt writes one row.** The row is Ready and owned by the calling subject, with a
   minted `ws_…` id, the given `root_name` and labels, and `origin: adopted`. Rows that
   `workspace.create` writes are `origin: created`, and existing rows migrate as `created`. The
   result is the workspace resource.
4. **The wire workspace resource publishes `origin` and `root_name`, read-only.** They are filled
   from the row on every answer that carries a workspace resource, for both origins. No input sets
   either one. A client can tell from `origin` whether a destroy will remove the directory.
5. **Refusals, in this order, all with existing codes:**

   | condition | code | status | decided by |
   |---|---|---|---|
   | `root_name` outside the set | `request.schema-invalid` (address `root_name`) | 422 | bundle input schema |
   | another subject's row holds `root_name` | `resource.not-found` | 404 | store, `UNIQUE (deployment, root_name)` |
   | the caller's own row holds `root_name` | `workspace.already-exists` | 409 | store |
   | no entry at `<workspace_root>/<root_name>` | `resource.not-found` | 404 | host observation, `ENOENT` |
   | the entry is not a directory, or the daemon's effective uid does not own it | `workspace.path-escape` (address `path`) | 422 | host observation |
   | the subject's partition is full | `workspace.capacity` | 507 | store (`snapshot_max_workspaces`) |
   | the operation ledger is full | `operation.ledger-capacity` | 507 | store |

   - **Another subject's row** answers exactly as an absent directory does, so adopt is no
     cross-subject existence oracle. Because the check spans subjects, adopting the directory of
     another subject's minted `ws_…` workspace is also refused, as not found: adoption cannot take
     over a workspace.
   - **The ownership check** is the one `workspace.create` already makes. `create_workspace` calls
     `observe_workspace` (`crates/substrate-host/src/fs.rs:133`). That refuses
     `st_uid != effective_uid()` or a non-directory at `crates/substrate-host/src/fs.rs:139`, with
     `path_escape()` (`crates/substrate-host/src/fs.rs:1381-1387`): code `workspace.path-escape`,
     address `path`.
   - No code is renamed and none is added.
6. **Every adopt refusal is recorded in the operation ledger** through `record_bound_refusal`,
   like every other pre-dispatch refusal, and a retry with the same operation id replays it. That
   includes the out-of-set name. This amends ADR 0034's decision 4, which kept name refusals out
   of the ledger.
7. **Destroying an adopted workspace forgets the record, on every removal path, and never deletes
   operator-owned files.**
   - **Created rows** are unchanged: the row goes to Destroying, the driver removes the tree, and
     the row is deleted.
   - **Adopted rows** are forgotten: the row, its cleanup entry and any lease are deleted, and the
     directory and its contents stay in place. The daemon still removes what it made for that
     workspace outside the directory, such as a git baseline under
     `.substrate-git-baselines/<root_name>`.
   - **Every path that ends a workspace** applies the same rule: synchronous `workspace.destroy`,
     restart cleanup of Destroying rows (`due_destroying_workspaces`), lease expiry and
     reconciliation. None of them removes an adopted directory or anything in it.
   - **The precondition refusals** are unchanged and apply to both origins: `resource.not-found`
     for an absent workspace or another subject's, `workspace.not-ready`, then
     `workspace.execs-active`. After destroy, the same directory can be adopted again.
8. **Executable specification.** The ESS projection `spec/workspaces/` models:
   - the Workspace row: id, root name, labels, origin, owner;
   - `Adopt` and `Destroy`, with these preconditions, effects and refusals;
   - five authored scenarios.

   The root-name set it previously called `WorkspaceId` is renamed `RootName`. `WorkspaceId` now
   names the unchanged `ws_…` id.

## Wire change: bundle `0.19.0`

`0.18.0` and every earlier directory stay byte for byte (invariant 6). The successor `0.19.0`
adds:

- **One route.** `workspace.adopt`, `POST /v1/workspaces/adopt`, api major 1, idempotency `keyed`,
  risk `write`, required scope `workspaces`, input binding `body.input`, like
  `workspace.create`. The route counts in the compatibility block grow by one from `0.18.0`'s,
  and the checker pins them. The static segment `adopt` cannot shadow a workspace, because it is
  not a `ws_…` id.
- **An input schema**, `schemas/inputs/workspace-adopt.json`, closed:

  ```json
  { "type": "object", "additionalProperties": false, "required": ["root_name", "labels"],
    "properties": {
      "root_name": { "type": "string", "minLength": 1, "maxLength": 255,
                     "pattern": "^[A-Za-z0-9_][A-Za-z0-9_-]*$" },
      "labels": { "$ref": "../common.json#/$defs/labels" } } }
  ```

  `labels` is required here because `workspace.create` requires it (`inputs/workspace-create.json`).
  `maxLength` 255 is published as a wire constant `MAX_ROOT_NAME_BYTES`. The ADR 0034 cut called
  it `MAX_WORKSPACE_ID_BYTES`; it is renamed with the ESS type. The constant is bound from
  `xtask/src/bundle.rs`, so `xtask/src/render.rs` does not change and no new `$wire` binding is
  needed.
- **Two new members on the workspace resource** (`schemas/resource.json`, closed with
  `additionalProperties: false`), both required in `0.19.0`:
  - `origin`: `"created"` or `"adopted"`;
  - `root_name`: the root-name set above.

  Every result and event schema that references the workspace resource carries them.
- **A result schema**, `schemas/results/workspace-adopt.json`: the workspace resource. Its `id` is
  the unchanged `workspace-id`.
- **Refusals.** The table in decision 5 lists the route's refusals in the bundle's refusal
  registry. All are existing codes.
- **Vectors.** These cases run through the shipped daemon:
  - an adopt that succeeds, with `origin` and `root_name` in the result;
  - an absent directory, an entry that is not a directory, and a directory another uid owns;
  - an out-of-set name, with its ledger record and replay;
  - a double adopt by the same subject (`workspace.already-exists`);
  - an adopt of a name another subject holds (`resource.not-found`, byte-identical to the
    absent-directory answer apart from request and operation ids);
  - an adopt of another subject's `ws_…` directory (`resource.not-found`);
  - a destroy of an adopted workspace that leaves the directory and its contents;
  - an exec started in an adopted workspace.
- **`workspace-id` is unchanged.**

The daemon and the Rust SDK advertise `substrate-wire/0.19.0` and its bundle digest. The gate's
`check-bundles` and `check-json` runs extend to `0.19.0`. A client that validates against `0.18.0`
must move to `0.19.0` to accept the two new resource members.

## Consequences

- **The confinement floor is unchanged.** An adopted workspace is mounted, bounded and torn down
  by the same code path as a created one: admission, exec and file I/O all start from the row's
  `root_name`. The delegated lane runs its isolation cases against an adopted workspace to prove
  it.
- **Every removal path reads the origin first.** Each of the four paths in decision 7 gets a
  negative test: an adopted directory with files survives it.
- **An adopted workspace has no lease and no quota.** Lease expiry therefore never applies to it,
  and its storage is whatever the operator's filesystem allows. `storage` observations stay
  absent for it rather than reporting a limit that was never applied (invariant 3).
- **The store gains a migration**: an `origin` column, with existing rows set to `created`.
- **Ownership comes from the caller.** The owner is the subject that adopts, and no request names
  another one. Another allowed subject cannot reach the workspace (rows are subject-scoped), and
  cannot learn through adopt that the name is held.
- **The withdrawn 0034 cut is re-done.** The shared root-name predicate and the ESS type carry
  over. The `0.19.0` bundle is re-cut as the additive route and resource members above, rather
  than as a widened identifier.
- **Known limits, unchanged by 0.19.0:**
  - **Aperture root.** The driver's default aperture root is
    `<workspace_root>/.substrate-apertures` (`crates/substrate-host/src/lib.rs:233`). When
    apertures are declared, it is created beside every adopted directory, in the directory the
    operator owns. `HostConfig::aperture_root` already moves it elsewhere. `0.19.0` neither changes
    the default nor exposes it on the wire. A root name cannot collide with it, because the set
    has no `.`.
  - **Git inside the exec.** A managed worktree's `.git` is a file pointing at a common git
    directory outside the workspace, which the confined mount does not contain. Git's writes
    there would not be writes to the workspace. `0.19.0` does not change this. A caller can bind
    that directory as a read-only root for read-only git commands. Writable git inside an exec is
    a separate decision.
- **Not stable.** `0.19.0` is a development bundle like its predecessors (atlas ADR 0019).
