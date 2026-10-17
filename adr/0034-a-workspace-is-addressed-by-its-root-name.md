---
status: superseded
date: 2026-10-15
---

# ADR 0034: a workspace is addressed by its single-component root name

Superseded by [ADR 0035](0035-an-operators-directory-is-served-after-an-explicit-adopt.md).

## Context

A workspace root may be a directory the operator already owns. The host driver resolves a
workspace id to `<workspace_root>/<id>` and admits any single non-escaping component
(`validate_root_name`, `crates/substrate-host/src/fs.rs`: ASCII alphanumerics, `_` and `-`, not
empty, not `.` or `..`, no leading `-`; a `.` is never admitted, so no hidden name and no
`.substrate-*` service directory). `create_workspace` still mints `ws_…`
(`crates/substrate-daemon/src/app/service.rs:129`).

Every released bundle publishes `workspace-id` as `^ws_[A-Za-z0-9]+$`
(`contracts/substrate-wire/0.18.0/schemas/common.json:294-296`), and the daemon's exec admission
enforces the same prefix (`crates/substrate-daemon/src/app/operations.rs:416-419`). Over the socket
an adopted directory can be read and written, but `exec.start` refuses it, and the published
schemas reject the very addresses the file routes already answer. A client running a gate in its
existing checkout needs the exec half. This ADR widens the identifier rather than adding an adopt
route or leaving adoption to in-process embedders.

## Decision

1. **Successor bundle `0.19.0`** (invariant 6: `0.18.0` and every earlier directory stay byte for
   byte) widens `workspace-id` to the root-name set:

   ```json
   "workspace-id": { "type": "string", "minLength": 1, "maxLength": 255,
                     "pattern": "^[A-Za-z0-9_][A-Za-z0-9_-]*$" }
   ```

   The pattern is exactly `validate_root_name`'s set; `maxLength` 255 is the Linux `NAME_MAX` the
   component must fit in, published as a new wire constant `MAX_WORKSPACE_ID_BYTES` and bound from
   `xtask/src/bundle.rs` (no new `$wire` binding, since `xtask/src/render.rs` must not change).
   `ws_…` ids remain members, so every existing id and client keeps working: the change is
   additive for senders and widening for receivers.
2. **Every consumer widens with it**, because they all `$ref` the one definition: 27 schemas under
   `contracts/substrate-wire/0.18.0/schemas/` (13 address schemas, 10 result schemas,
   `inputs/exec-start.json`, `inputs/pipe-session-start.json`, `event.json` and `resource.json`), plus the one
   inline copy `^workspace:ws_[A-Za-z0-9]+$` in `results/reconciliation-snapshot-get.json`, which
   becomes `^workspace:` followed by the same set.
3. **The daemon admits exactly that set** on `exec.start`, raw-pipe and PTY session start, and every
   route that names a workspace, through one predicate in `substrate-wire` shared by the admission
   check and the host's `workspace_path` (which today also admits a leading `-`; that difference
   closes).
4. **Refusals, all before dispatch and before any durable operation record (ADR 0005):**
   a name outside the set on `exec.start` or session start keeps the existing shape refusal
   (`request.schema-invalid`, 422, `crates/substrate-daemon/src/app/responses.rs:70-80`); on a
   workspace route it is `workspace.path-escape` (field `workspace`), the code the host already
   returns (`crates/substrate-host/src/lib.rs:829-838`). A name in the set whose directory does not
   exist stays `resource.not-found`. No refusal is renamed and none is added, so no client
   branches on a new code.
5. Creation is unchanged: `workspace.create` still mints `ws_…`. Adoption is the operator placing a
   directory under the workspace root; there is no adopt route and no durable adoption record
   (an adopt route with a durable record was considered and not taken).

## Consequences

- The confinement floor is unchanged: an adopted workspace is mounted, bounded and torn down by the
  same code path as a minted one, and the delegated lane runs its isolation cases against an
  adopted workspace to prove it.
- A runtime vector adopts an un-prefixed directory, starts an exec in it through the shipped daemon
  and observes output and exit; another sends a name outside the set and observes the refusal with
  no operation recorded.
- The daemon and Rust SDK advertise `substrate-wire/0.19.0` and its bundle digest; clients that
  validate against `0.18.0` keep sending `ws_…` ids and see no change; a client reading an
  adopted id in an event or a resource listing needs `0.19.0`.
- **Known limits, from a consumer's spike, unchanged by 0.19.0:**
  - the driver's default aperture root is `<workspace_root>/.substrate-apertures`
    (`crates/substrate-host/src/lib.rs:233`), so an adopted tree's parent directory receives it
    when apertures are declared. `HostConfig::aperture_root` already moves it elsewhere; `0.19.0`
    neither changes the default nor exposes it on the wire. The workspace name set cannot collide
    with it (no `.`).
  - git does not run inside the exec. A managed worktree's `.git` is a file pointing at a common
    git directory outside the workspace, which the confined mount does not contain, and git's
    writes there are not writes to the workspace. `0.19.0` does not change this; a caller can
    bind that directory as a read-only root for read-only git commands, and writable git inside an
    exec is a separate decision.
- Not stable: `0.19.0` is a development bundle like its predecessors (atlas ADR 0019).
