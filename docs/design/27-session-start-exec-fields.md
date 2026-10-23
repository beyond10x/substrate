# Session start publishes the exec fields exec start does

Status: accepted for implementation, 2026-10-23. Owned by
`story:session-start-publishes-exec-fields`; carried by the unreleased successor bundle `0.23.0`.

Invariant 8 asks for a design before a contract change. No ADR is needed: ADRs 0020, 0021 and 0023
define `scratch`, `measurements` and `workspace_access` for a start, and none of them excludes a
session. This note records what the daemon already does with them on the session route and what the
bundle therefore has to say.

## Evidence: what the daemon does today

A session embeds the exec start type itself: `PipeSessionStartInput.exec` is an `ExecStartInput`
(`crates/substrate-wire/src/lib.rs`, `pub exec: ExecStartInput`), and that type carries
`workspace_access`, `scratch` and `measurements` as optional members. A session request naming any
of them is therefore decoded, not refused as an unknown field.

| field | daemon admission on `POST /v1/sessions` | driver on dispatch |
|---|---|---|
| `measurements` | `validate_pipe_session_input` ends in `validate_exec_input` (`crates/substrate-daemon/src/app/operations.rs`), which refuses `exec.metrics-unserved` (501) when the snapshot has no `exec.resource-usage`. `pipe_session_start` seeds the provisional record's `usage` from it (`crates/substrate-daemon/src/app/sessions.rs`). | `start_pipe_with_git_roots` calls the shared `start_inner` with `&input.exec` (`crates/substrate-host/src/process.rs`), whose `admit` refuses the same code and whose record carries `usage`. |
| `scratch` | the same `validate_exec_input`: shape within the storage bounds, `exec.scratch-quota-unserved` (501) without `exec.scratch-quota`, `exec.scratch-quota-limit` (507) over or misaligned with the fact. | `start_inner` runs `prepare_scratch(id, input.scratch)` and mounts `/scratch`. |
| `workspace_access` | not checked by the daemon on either route. | `admit` runs `admit_workspace_access` and `admit_writable_subtrees`; `start_inner` runs `prepare_workspace_access` and binds the workspace as granted; the applied record states the grant. Refusals: `exec.workspace-access-invalid` (422), `exec.workspace-scoped-write-unserved` (501), `exec.workspace-subtree-absent`, `-symlink`, `-not-directory` (422). |

So each field is accepted and honoured on the session route, by the same code and with the same
refusals as on exec start. The session route adds only its own gates in front (capture, mode, window,
`session.confinement-unavailable`, the session bounds), which is why a portable host answers
`session.confinement-unavailable` before any of these fields is read.

## Decision

All three fields take the first branch of the story's contract: **accepted and honoured**. Bundle
`0.23.0` adds `scratch`, `measurements` and `workspace_access` to the `exec` object of
`schemas/inputs/pipe-session-start.json`, each byte for byte the member `exec-start.json` carries. No
refusal is new. A session start that names none of the three serializes and hashes exactly as
before. Amendments 1 to 3 below widen the checker, publish the refusals, and align the daemon with
the schema.

The checker states the class rather than the instance: the session-start `exec` object must declare
exactly the property set of the exec-start object, and each member it shares with exec start must be
the same schema in the rendered bundle, except `wait`, which a session narrows on purpose to
`const: false` (a session's exec is also always leased, which its `required` list states, not the
member's schema). A future exec-start field that is left off
the session schema then fails `cargo xtask check-bundle 0.23.0`, instead of waiting for the next
consumer to find it.

## Runtime evidence

`crates/substrate-daemon/tests/runtime_vectors.rs` gains one delegated case per field, each started
with `POST /v1/sessions`:

- `measurements: ["resource-usage"]`: the session's exec record carries `usage`, and after the
  attachment receives the `exit` frame the record's usage is `observed`.
- `workspace_access: {"mode": "read-only"}`: the applied record states the grant, the child's write
  to `/workspace` fails inside the sandbox, and the host workspace stays unchanged.
- `scratch`: the clean-room daemon declares no project-quota identities, so its snapshot has no
  `exec.scratch-quota`, and the start is refused `exec.scratch-quota-unserved` (501) before dispatch
  and recorded as refused. A served scratch quota needs the project-quota fixture, which only
  `cargo xtask quota-lane` provides; it is not claimed here.

The portable lane gains no case: on that lane every session start is refused
`session.confinement-unavailable` before the exec fields are read, so a portable case could not tell
a served field from an ignored one.

## Amendment 1, 2026-10-23: the object, not only its members

Comparing member schemas is not enough to make the session `exec` object exec start's contract. Two
more parts of the object decide which requests it admits, so the checker compares them too:

- `additionalProperties` must be the same on both objects. Today both are `false`, matching the wire
  crate's `deny_unknown_fields` `ExecStartInput`.
- The session `exec` object's `required` list must equal exec start's plus `lease_ttl_ms`, and
  nothing else. Today exec start requires `workspace`, `argv`, `env`, `sandbox`, `limits` and `wait`.
  The session object requires those and `lease_ttl_ms`. That addition is legitimate:
  `validate_pipe_session_input` refuses a session whose exec has no `lease_ttl_ms`
  (`crates/substrate-daemon/src/app/operations.rs`, `input.exec.lease_ttl_ms.is_none()`), and
  `pipe_session_start` relies on it (`"validated pipe sessions always have leases"`).

The check runs for `0.23.0` and for every later bundle, so this decision holds for successors and
does not depend on whoever cuts the next bundle remembering it.

## Amendment 2, 2026-10-23: the refusals these fields raise are published

The story's acceptance needs the refused branch to name a published code. Bundle `0.23.0` is
unreleased, so it publishes the refusals the three fields raise. They are published on exec start and
session start alike, as an `x-b10x-refusals` annotation on the member, as the bundle already does for
`read_only_roots`, `apertures` and `writable_roots`. The set and statuses come from the code that
raises them:

| member | code | status, class | raised by |
|---|---|---|---|
| `measurements` | `exec.metrics-unserved` | 501 unserved | `validate_exec_input` (daemon), `admit` (host) |
| `scratch` | `exec.scratch-quota-unserved` | 501 unserved | `validate_exec_input`, `admit`, `prepare_scratch` |
| `scratch` | `exec.scratch-quota-limit` | 507 exhausted | `validate_exec_input`, which runs before the host's identical check |
| `workspace_access` | `exec.workspace-access-invalid` | 422 refused | `admit_workspace_access`, `prepare_workspace_access` |
| `workspace_access` | `exec.workspace-scoped-write-unserved` | 501 unserved | `admit_workspace_access` |
| `workspace_access` | `exec.workspace-subtree-absent` | 422 refused | `admit_writable_subtrees` |
| `workspace_access` | `exec.workspace-subtree-symlink` | 422 refused | `admit_writable_subtrees` |
| `workspace_access` | `exec.workspace-subtree-not-directory` | 422 refused | `admit_writable_subtrees` |
| `workspace_access` | `exec.workspace-subtree-changed` | 422 refused | `prepare_workspace_access` |

The codes become `substrate_wire` constants. The daemon and the host raise them through those
constants, and the bundle check holds each member's annotation to the wire crate's table. Failures
(`exec.scratch-create-failed`, `exec.workspace-pin-failed`, 500) are host faults, not refusals of a
request, and are not part of this set. Frozen bundles `0.1.0` to `0.22.0` do not change. They keep
the gap.

## Amendment 3, 2026-10-23: the daemon refuses what the schema refuses

The wire crate admitted three values the published schema refuses on both starts: `scratch: null`
(decoded as absent), a `scratch.max_bytes` that is not a multiple of 1024 (the schema's `multipleOf`),
and a duplicate in `measurements` (decoded into a set). Each is now refused as
`request.schema-invalid` (422) on both routes:

- `scratch` and `measurements` are decoded by functions that refuse `null` and a repeated
  measurement.
- `StorageLimit::within_contract_bounds` adds the 1024-byte unit. That function also bounds a
  workspace's `storage`, which has the same schema definition, so a misaligned workspace quota is now
  `request.schema-invalid` at admission instead of `storage.quota-limit` from the host.

No client in the collection sends any of these values. The Rust SDK and `harness-substrate` build
starts from the wire type, which serializes an absent `scratch` by omission and `measurements` from a
set.
