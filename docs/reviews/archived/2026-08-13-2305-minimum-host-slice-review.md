# Code review: minimum host substrate slice

- **Date:** 2026-08-13T23:05:47+02:00
- **Scope:** `38258cd..0adf749` — `feat(contract): start substrate wire bundle`,
  `feat(contract): close phase-2 execution bundle`, `feat: implement minimum host substrate slice`
  (~8.4k lines: `substrate-wire`, `substrate-store`, `substrate-host`, `substrate-daemon`, contract bundle, tests, `scripts/check-runtime-vectors.py`)
- **Method:** multi-agent review — 10 finder angles plus a gap sweep, each finding adversarially
  verified. 37 findings survived verification (24 confirmed, 7 plausible, 6 from the gap sweep);
  the 15 most severe are recorded below, cleanup-tier findings were cut at the cap.
- **Verdict:** implementation is not sound yet. Idempotency/replay is broken in three independent
  ways, the exec lifecycle has self-DoS and stuck-state paths, workspace destroy is defeatable by
  a sandboxed exec, and five published conformance vectors certify behavior the implementation
  cannot produce.

All `file:line` references are relative to the repository root at commit `0adf749`.

## Findings (most severe first)

### 1. Tracked-exec map never pruned for waited/abandoned execs — permanent self-DoS

`crates/substrate-host/src/process.rs:235` — resource-exhaustion

Tracked-exec map entries are only removed inside `observe()`, so execs run with `wait=true`
(returned inline, never observed) or abandoned `wait=false` execs permanently fill
`max_tracked_execs=128` and all further exec starts fail.

**Failure scenario:** Run 128 execs with `wait=true` and never GET them afterward:
`executions.len()` stays at 128 (only `observe()` at `process.rs:289-291` prunes; `run_child`
never does), so every subsequent `POST /v1/execs` fails 429 `exec.tracking-capacity` until daemon
restart — permanent self-DoS under normal fire-and-forget usage.

### 2. `ExecState::Unknown` never resolves; blocks workspace destroy forever after restart

`crates/substrate-store/src/lib.rs:526` — correctness

`ExecState::Unknown` is counted as nonterminal by `workspace_has_nonterminal_execs`,
`reconcile_after_restart` rewrites interrupted execs to `Unknown`, and no code path ever
transitions `Unknown` out — so after any restart with an unobserved exec the owning workspace is
undestroyable forever, and signalling that exec durably records a 404.

**Failure scenario:** Start exec with `wait=false`, restart daemon (`main.rs:75` reconcile sets
`state=unknown`); every `DELETE /v1/workspaces/{id}` returns 409 `workspace.execs-active` forever
(`exec_get`'s `Err` fallback re-serves the stored Unknown record; nothing deletes exec rows).
`POST /v1/execs/{id}/signal` falls past the `Exited|Cancelled` short-circuit (`app.rs:691-693`),
the driver returns `not_found`, and a terminal 404 is durably recorded for that operation id.

### 3. `wait_terminal` loses a notify between the state check and the first poll

`crates/substrate-host/src/process.rs:663` — race-condition

`wait_terminal` creates `notify.notified()` but does not enable it before re-checking state, so a
`notify_waiters()` fired between the `is_terminal` check and the first poll is lost, and the
timeout path returns `exec.observe-timeout` without re-checking the now-terminal state.

**Failure scenario:** `exec.start` with `wait=true`; child exits and `run_child` sets terminal
state + `notify_waiters()` in the check-to-poll window → the request hangs for the full
`timeout_ms+5s`, then `finish_driver_error` durably records a 500 `exec.observe-timeout` for an
exec that exited quickly; replays of that op id return the false 500 forever.

### 4. `observe()` drops terminal state from memory before it is persisted

`crates/substrate-host/src/process.rs:290` — durability

`observe()` destructively removes a terminal exec from the in-memory map before the daemon
persists it, so a `put_exec` failure loses the terminal state and output forever, and a
concurrent GET in the window serves the stale Running record.

**Failure scenario:** Exec exits; `GET /v1/execs/{id}` removes the map entry
(`process.rs:289-291`), then `app.rs:595` `put_exec` fails (SQLite busy) → 500; the observation
now exists nowhere: store stays `state=Running` with empty output permanently, workspace destroy
returns 409 forever, and a concurrent second GET gets driver NotFound and serves the stale
Running/empty-output snapshot.

### 5. `env.set` containing `PWD` breaks the `/usr/bin/env` command line; every exec exits 127

`crates/substrate-host/src/process.rs:518` — correctness

The sandbox command line is built as `/usr/bin/env -u NAME NAME=value -- argv...`, and GNU env
stops option parsing at the first assignment, treating the following `--` as the program to
execute — so an `env.set` containing `PWD` (passes validation) makes every exec fail with exit
127 without running the user's program.

**Failure scenario:** `exec.start` with `env.set {"PWD": "/workspace"}` and argv
`["/usr/bin/true"]` → env exits 127 with `env: '--': No such file or directory` (empirically
verified: `/usr/bin/env -u PWD PWD=v -- echo hi` exits 127); the exec is durably recorded as
exited code 127 though argv never ran.

### 6. Unix accept loop propagates transient errors with `?` — one EMFILE kills the daemon

`crates/substrate-daemon/src/main.rs:91` — availability

The unix-socket accept loop propagates `listener.accept()` and `peer_cred()` errors with `?`, so
one transient error terminates the whole daemon and skips socket cleanup.

**Failure scenario:** A client opens connections until the daemon hits its fd limit: `accept()`
returns EMFILE (or ECONNABORTED for a connection aborted while queued) →
`accepted.context("accept unix peer")?` returns from main, killing all in-flight execs and
connection tasks, and the socket file removal at `main.rs:121` never runs — a routine transient
error becomes total service loss.

### 7. Mutation handlers return 404 before consulting the idempotency reservation

`crates/substrate-daemon/src/app.rs:441` — idempotency

Mutation handlers do resource-existence lookups and return 404 before consulting the idempotency
reservation (`begin`/`reserve`), so a legitimate retry of a previously-successful mutation
returns 404 instead of the stored replay answer.

**Failure scenario:** `DELETE /v1/workspaces/X` succeeds but the response is lost; the client
retries with the same operation id; `store.workspace` now returns `None` (row deleted by
`complete_workspace_absence`), so `app.rs:436-441` returns 404 `resource.not-found`
(`retriable:false`) before `begin()` at `app.rs:461` could replay the stored 200 — replay
stability is violated on `workspace_destroy`, `workspace_file_write` (`app.rs:305`), `exec_start`
(`app.rs:534`), and `exec_signal` (`app.rs:671`).

### 8. Retriable errors are persisted as terminal replay answers

`crates/substrate-daemon/src/app.rs:1199` — idempotency

`finish_driver_error` persists retriable errors (429 `exec.concurrency-limit`, 500 machinery
failures) as the terminal replay answer for the operation id, so a client that honors
`retriable=true` and retries the same op id gets the stored error replayed forever.

**Failure scenario:** `POST /v1/execs` hits ActivePermit exhaustion → `DriverError::exhausted`
(`retriable:true`) → `complete_error` marks the operation terminal (store `lib.rs:383-386`); the
client retries after load drains, but `reserve()` (`lib.rs:142-143`) replays the stored 429
unconditionally — the exec can never run under that operation id despite the response saying
retriable.

### 9. 1 MiB body limit makes the advertised 1 MiB max file size unreachable

`crates/substrate-daemon/src/app.rs:28` — correctness

`BODY_LIMIT` = 1 MiB applies to the whole JSON envelope while the probed capability advertises
`workspace.max-file-bytes = 1_048_576`; base64 expansion (4/3) makes any file content above
~786 KB unwritable through the API.

**Failure scenario:** `PUT /v1/workspaces/{id}/files/f` with 1,048,576 content bytes (exactly the
advertised max): the base64 `data` field alone is ~1.4 MB, so `to_bytes(body, BODY_LIMIT)` at
`app.rs:764` rejects with 429 `request.body-limit` — writes between ~786 KB and the advertised
1 MiB are impossible and the `fs.rs:228` write-limit path is unreachable at default config.

### 10. A FIFO, device node, or non-UTF-8 filename makes a workspace unlistable and undestroyable

`crates/substrate-host/src/fs.rs:473` — correctness

`list_directory` errors on any FIFO/socket/device entry or non-UTF-8 filename (also leaking the
`fdopendir` `DIR*` and fd on the early returns at `fs.rs:463-464`), and `destroy_workspace`
depends on it while `delete_file` only unlinks regular files — a sandboxed exec can make its
workspace permanently unlistable and undestroyable.

**Failure scenario:** Exec runs `mkfifo /workspace/p` (or creates a filename with byte 0xFF, or
nests dirs deeper than `MAX_PATH_DEPTH=64`) inside the rw-bound `/workspace`: every directory
read returns `workspace.path-escape` and `DELETE /v1/workspaces/{id}` fails forever
(`remove_children` → `list_directory` errors; `delete_file` rejects non-S_IFREG at `fs.rs:333`);
each retry on the non-UTF-8 case additionally leaks one fd, so repeated retries drive the daemon
to EMFILE process-wide.

### 11. `signal()` marks an exec Cancelled even if it exits normally

`crates/substrate-host/src/process.rs:357` — race-condition

`signal()` sets `cancellation_requested=true` unconditionally before delivering the signal, so an
exec that exits naturally in the race window — or survives the signal and later exits cleanly —
is durably recorded as Cancelled instead of Exited.

**Failure scenario:** `POST /v1/execs/{id}/signal` (INT, grace) to a process that traps SIGINT
and later finishes with exit code 0: `run_child` computes `cancellation=true` at
`process.rs:548` and records `state=Cancelled` with `exit.code=0` via `complete_exec` — clients
and the durable store see a cancelled exec that actually completed successfully; same
misclassification if the process exits naturally while the signal request is in flight.

### 12. Refusal durability diverges between exec.start and workspace.create

`crates/substrate-daemon/src/app.rs:521` — contract-drift

Refusal durability diverges by route: `exec.start` records schema/validation refusals durably
(binding the op id to the malformed request's hash) while `workspace.create` returns identical
validation failures with no durable record — one of the two violates the contract, and
`exec.start` op ids are permanently burned by a fixable typo.

**Failure scenario:** `exec.start` with one argv entry of 4097 chars → 422 recorded as a durable
refusal; the client fixes the argument and retries with the same operation id → 409
`operation.request-conflict` forever; the identical fix-and-retry flow on `workspace.create`
(validated at `app.rs:156` before any record) succeeds.

### 13. Five published conformance vectors expect behavior the implementation cannot produce

`contracts/substrate-wire/0.1.0/vectors/http/file-delete-depth.json:44` — contract-drift

Five published conformance vectors demand behavior the implementation can never produce — and
nothing in the repo executes vectors, so the drift shipped:

| Vector | Expects | Implementation |
| --- | --- | --- |
| `file-delete-depth.json` | code `workspace.path-depth` | emits only `workspace.path-escape`; `path-depth` has zero hits in `crates/` |
| `machinery-failure.json:49` | `workspace.driver-failed`, address `workspace.file` | driver emits `workspace.write-failed`, address `null` |
| `exec-signal.json:55` | exit `{code:null, signal:"TERM"}` | leader excluded from `signal_all`; TERM yields code 143, grace-expiry yields KILL — the repo's own `check-runtime-vectors.py:397` asserts KILL |
| `crash-before-dispatch.json:24` | `operation_state_after_restart="accepted"` | `reconcile_after_restart` unconditionally rewrites accepted→unknown |
| `cross-subject-not-found.json:38` | `error.address = "operation"` | `not_found` always emits `"resource"` |

**Failure scenario:** Any conformance runner built per `runner.json`
(`exact-json-value-equality`) replaying these vectors against the daemon fails all five cases
permanently; `coverage.json` cites them as the evidence for the corresponding requirements, so
the published contract certifies behavior the reference implementation does not have.

### 14. `workspace_get` can resurrect a concurrently destroyed workspace row

`crates/substrate-daemon/src/app.rs:235` — race-condition

`workspace_get`'s read → observe → `put_workspace` sequence can resurrect a workspace row that a
concurrent destroy deleted, creating a permanent ghost record with no backing directory.

**Failure scenario:** `GET /v1/workspaces/X` reads the row and observe succeeds; a concurrent
DELETE completes (row deleted, directory removed); the GET then upserts the row back
(`upsert_workspace` ON CONFLICT DO UPDATE, store `lib.rs:675`) and returns 200 Ready; subsequent
DELETE fails in the driver (`openat2` ENOENT → `not_found` at `fs.rs:593`) before reaching
`complete_workspace_absence`, so the ghost row is permanent (`store.remove_workspace` has zero
callers).

### 15. `workspace_destroy` races `exec_start` — a workspace can be deleted under a live exec

`crates/substrate-daemon/src/app.rs:562` — race-condition

`workspace_destroy`'s nonterminal-exec check is not atomic with `exec_start`, whose exec row is
first written only after the process is spawned — so destroy can recursively delete a workspace
while an exec is starting or running in it.

**Failure scenario:** T1: `POST /v1/execs` passes the workspace lookup and enters
`driver.start_exec`; T2: `DELETE /v1/workspaces/{id}` runs `workspace_has_nonterminal_execs` (no
exec row yet → false) and begins `remove_children` while bwrap rw-binds the same directory — an
exec runs in a half-deleted root, destroy fails mid-traversal (ENOTEMPTY), and the store ends
with a Running exec bound to a workspace whose destroy was recorded.

## Verified but cut at the 15-finding cap

- Signal-vs-exit cgroup race durably recording a 404 (`crates/substrate-host/src/process.rs:356`).
- Dual-daemon socket steal via `prepare_socket` (`crates/substrate-daemon/src/main.rs:131`).
- Zero `spawn_blocking` — blocking fs/SQLite calls run on tokio workers (`crates/substrate-host/src/lib.rs:327`).
- Driver `admit()` missing upper bounds on `processes`/`memory` (`crates/substrate-host/src/process.rs:429`).
- `put_exec` fsync on every poll (`crates/substrate-daemon/src/app.rs:595`).

## Cleanup tier (not individually verified, noted for later)

- Duplicated secret-denylist and output-slice logic.
- Mirror `ExecObservation`/`StoredExec` structs.
- Unused dependencies.
- Router rebuilt per connection.
- Redundant base64 re-encode; `write_atomic` read-back.
- JCS canonicalization duplicated across Python scripts.
- Docs claim phase 2 complete while nothing executes the published vectors.
