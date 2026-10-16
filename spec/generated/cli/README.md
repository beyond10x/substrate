# `b10x-substrate` CLI contract

Drive a Substrate daemon over its Unix socket

This generated package installs an unavailable handler. Application behavior enters through the `Handler` seam; schema-selected calls also require `DynamicValidator`. See `help.txt` for process options and `binding.json` for resolved types and targets.

## Commands

### `events`

Read one page of the daemon's event log

Callable: `events`. Input: `substrate.client.EventsRequest`. Result: `substrate.client.EventPage`.

Error `refused`: `substrate.client.RefusalData`.

Error `unavailable`: `substrate.client.UnavailableData`.

### `exec run`

Run a command in a workspace and wait for it to end

Callable: `exec-run`. Input: `substrate.client.ExecRunRequest`. Result: `substrate.client.ExecRunResult`.

Error `refused`: `substrate.client.RefusalData`.

Error `unavailable`: `substrate.client.UnavailableData`.

### `exec show`

Show an exec as the daemon observes it

Callable: `exec-show`. Input: `substrate.client.ExecKey`. Result: `substrate.client.ExecRow`.

Error `refused`: `substrate.client.RefusalData`.

Error `unavailable`: `substrate.client.UnavailableData`.

### `machine`

Show the daemon's capability snapshot and facts

Callable: `machine`. Input: `null (inputless)`. Result: `substrate.client.MachineRow`.

Error `refused`: `substrate.client.RefusalData`.

Error `unavailable`: `substrate.client.UnavailableData`.

### `metrics`

Show the resource usage the daemon observes for an exec or a workspace

Callable: `metrics`. Input: `substrate.client.MetricsKey`. Result: `substrate.client.MetricsRow`.

Error `refused`: `substrate.client.RefusalData`.

Error `unavailable`: `substrate.client.UnavailableData`.

### `operation show`

Show an accepted operation's ledger record

Callable: `operation-show`. Input: `substrate.client.OperationKey`. Result: `substrate.client.OperationRow`.

Error `refused`: `substrate.client.RefusalData`.

Error `unavailable`: `substrate.client.UnavailableData`.

### `session show`

Show a session as the daemon observes it

Callable: `session-show`. Input: `substrate.client.SessionKey`. Result: `substrate.client.SessionRow`.

Error `refused`: `substrate.client.RefusalData`.

Error `unavailable`: `substrate.client.UnavailableData`.

### `session signal`

Signal a session's process

Callable: `session-signal`. Input: `substrate.client.SessionSignalRequest`. Result: `substrate.client.SessionRow`.

Error `refused`: `substrate.client.RefusalData`.

Error `unavailable`: `substrate.client.UnavailableData`.

### `workspace create`

Create an empty workspace

Callable: `workspace-create`. Input: `substrate.client.WorkspaceCreateRequest`. Result: `substrate.client.WorkspaceRow`.

Error `refused`: `substrate.client.RefusalData`.

Error `unavailable`: `substrate.client.UnavailableData`.

### `workspace destroy`

Destroy a workspace

Callable: `workspace-destroy`. Input: `substrate.client.WorkspaceDestroyRequest`. Result: `substrate.client.WorkspaceAbsenceRow`.

Error `refused`: `substrate.client.RefusalData`.

Error `unavailable`: `substrate.client.UnavailableData`.

### `workspace renew`

Renew a workspace lease

Callable: `workspace-renew`. Input: `substrate.client.WorkspaceRenewRequest`. Result: `substrate.client.WorkspaceRow`.

Error `refused`: `substrate.client.RefusalData`.

Error `unavailable`: `substrate.client.UnavailableData`.

### `workspace show`

Show a workspace as the daemon observes it

Callable: `workspace-show`. Input: `substrate.client.WorkspaceKey`. Result: `substrate.client.WorkspaceRow`.

Error `refused`: `substrate.client.RefusalData`.

Error `unavailable`: `substrate.client.UnavailableData`.

## Runtime obligations

- handler:events: implement the owner-qualified callable and its declared result/error contract
- handler:exec-run: implement the owner-qualified callable and its declared result/error contract
- handler:exec-show: implement the owner-qualified callable and its declared result/error contract
- handler:machine: implement the owner-qualified callable and its declared result/error contract
- handler:metrics: implement the owner-qualified callable and its declared result/error contract
- handler:operation-show: implement the owner-qualified callable and its declared result/error contract
- handler:session-show: implement the owner-qualified callable and its declared result/error contract
- handler:session-signal: implement the owner-qualified callable and its declared result/error contract
- handler:workspace-create: implement the owner-qualified callable and its declared result/error contract
- handler:workspace-destroy: implement the owner-qualified callable and its declared result/error contract
- handler:workspace-renew: implement the owner-qualified callable and its declared result/error contract
- handler:workspace-show: implement the owner-qualified callable and its declared result/error contract
