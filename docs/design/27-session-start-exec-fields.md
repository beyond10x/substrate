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
refusal is new and no daemon or driver code changes. A session start that names none of the three
serializes and hashes exactly as before.

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

## Not decided here

Exec start does not publish its own refusal codes for these fields (`exec.metrics-unserved`,
`exec.scratch-quota-unserved`, `exec.scratch-quota-limit` and the five `exec.workspace-*` codes)
anywhere in the bundle. Mirroring exec start exactly keeps that gap on both starts; closing it is a
separate story.
