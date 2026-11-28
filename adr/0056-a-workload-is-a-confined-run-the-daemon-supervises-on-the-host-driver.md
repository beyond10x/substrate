---
status: accepted
date: 2026-11-28
---

# ADR 0056: a workload is a confined run the daemon supervises on the host driver

Decider: the operator, 2026-11-28 11:08 CET, relayed by the operator's session (D1, option A);
`approval-record:operator-answers-design-38-questions-2026-11-28`. Carries
[design 38](../docs/design/38-long-lived-service-on-the-host-driver.md) § 4 and decisions W1 to W6,
W8 to W10 and W22. The design comes before code (invariant 8): this record adds a resource family,
a capability fact, start members, events, refusal codes, store tables and a successor bundle, and no
code yet. The back-off and bound figures are proposals the slice holds with cases on a test clock; a
figure moves only through an amendment.

## Context

The `weft` repository needs a long-lived service on a workstation with no container daemon. What the
tree has today, and what it lacks:

- A background exec has no restart policy ([design 01](../docs/design/01-contract.md) § 3.2). Design 01
  § 3.3 specifies workloads (spec, status, operations) and § 8 marks Workload unserved on the host
  driver. No released bundle holds a workload route (`contracts/substrate-wire/0.29.0/operations.json`),
  and the store has no table for workloads (`crates/substrate-store/src/schema.rs`).
- A host process is supervised, has its own cgroup, a bounded log ring, an event stream and leases,
  and is unconfined and does not restart: [ADR 0036](0036-host-processes-are-a-supervised-resource-family.md)
  says its supervisor "does not restart" a process. A host process's environment values are never
  stored.
- An exec runs under bubblewrap with `--unshare-user --disable-userns`, `--unshare-ipc --unshare-pid
  --unshare-net --unshare-uts`, `--new-session`, `--die-with-parent`, `--clearenv` and a seccomp
  program, in its own cgroup v2 directory created before the spawn with `pids.max`, `memory.max`,
  `memory.swap.max` 0 and a `cpu.max` quota (`command`, `Cgroup::create`, `exec_cpu_quota` in
  `crates/substrate-host/src/process.rs`). The child is `kill_on_drop(true)`, so it dies with the
  daemon; a host process does not.
- `ExecLimits` is `{timeout_ms, output_bytes, processes, memory_bytes, cpu_millis}`, where
  `cpu_millis` is a cumulative budget whose quota is derived over `timeout_ms`
  (`crates/substrate-wire/src/lib.rs`). It is not a rate.
- A lease expiry and a deadline stop are operations the daemon originates under its own actor,
  recorded durably before the driver acts (`supervisor_operation` in
  `crates/substrate-daemon/src/app/processes.rs`). The ledger holds at most 100,000 rows per subject
  (`OPERATION_LEDGER_*_MAX_*`).
- The event union is closed: each variant has a `resource_kind`, a resource id, a `transition`, a
  cause of kind `operation` and an observation (`contracts/substrate-wire/0.29.0/schemas/event.json`).
- The daemon names the host crate only in the composition root (`crates/substrate-daemon/tests/driver_port.rs`;
  invariant 4).

A workload also needs a root of its own: it runs from an image
([ADR 0055](0055-the-daemon-unpacks-a-root-filesystem-from-a-sha256-named-archive.md)).

## Decision

1. **`workloads` is a family of the host driver** (W1): a long-lived, confined, daemon-supervised
   run of one program from one image under a restart policy. It sits beside exec and host process.
   It is not Docker-backed. The supervisor is a module of the host crate beside `host_process`; the
   daemon reaches it through the `Driver` trait only. One capability fact, `workload.supervised`,
   `const true`, is published only after the exec confinement facts were proved and a throwaway
   image-view run succeeded, never after reading configuration; it is absent on the portable lane and
   on the namespace driver.

2. **The spec of the first slice.**

   ```
   { name, image: {digest}, argv, cwd?, env, restart, limits, lease_ttl?, labels }
   ```

   - `name` is unique among the subject's workloads: `workload.name-taken` (409). The id is `wl_...`.
   - `argv` is the program and arguments as one list, as every start takes it (design 04 § 3). Design
     01's `command` and `args` become it; the archive carries no default command.
   - `cwd` is an absolute view path, default `/`.
   - `env` is the exec's closed environment shape. **It is stored in the `workloads` table**,
     because a relaunch after a daemon restart needs it (W22). It is declared non-secret (design 04
     § 3); a secret travels as a slot or as a per-run value
     ([ADR 0058](0058-a-start-carries-a-per-run-secret-value-held-in-memory.md)), never in `env`.
   - `restart` is `never`, `on-failure` or `always`.
   - `limits` is `{processes, memory_bytes, cpu_millicores}`, all required (W9). `cpu_millicores` is a
     rate (1,000 is one core), capped by the operator's `exec_cpu_cores`, and sets `cpu.max`. A
     workload has no `timeout_ms` and no cumulative `cpu_millis`; a start naming either is
     `request.schema-invalid`.
   - `lease_ttl` is optional, `MIN_LEASE_TTL_MS` to `MAX_LEASE_TTL_MS` (1,000 to 86,400,000);
     `labels` as everywhere.
   - Not in the first slice: `mounts`, `endpoints`
     ([ADR 0057](0057-an-inbound-endpoint-is-a-daemon-relay-into-a-no-egress-run.md)), `secret_slots`
     and `secret_values`. A workload names apertures by the existing `aperture` and `apertures`
     lists.

3. **Operations served** (W2).

   | operation | route | notes |
   |---|---|---|
   | deploy | `POST /v1/workloads` | `workloads.deploy` |
   | get, list | `GET /v1/workloads[/{id}]` | |
   | stop, start, restart | `POST /v1/workloads/{id}/(stop\|start\|restart)` | |
   | delete | `DELETE /v1/workloads/{id}` | |
   | logs | `GET /v1/workloads/{id}/logs` | a bounded read of the ring; no WebSocket stream |
   | lease renewal | `POST /v1/workloads/{id}/lease/renew` | design 01 § 5 |

   Replace (`PUT /v1/workloads/{id}`) is not served and `workload.replace` is not published; the
   consumer stops, deletes and deploys. Every mutation is keyed and durable before dispatch
   ([ADR 0005](0005-operations-are-durable-before-driver-dispatch.md)).

4. **Status** is design 01's: `{desired: running|stopped, observed: {state, probed_at} | null,
   restarts, last_exit: {code?, signal?, at, cause} | null}`, with `observed: null` for "cannot
   answer", never "running". `cause` is `exit`, `signal`, `limit`, `stop`, `lease-expired`,
   `daemon-restart` or `start-failed`. The applied record carries what an exec's does plus `image`
   and the run counter.

5. **The state machine** (W3). Desired state is what the caller asked and is durable before dispatch;
   observed state is what the supervisor saw. The observed states are `starting`, `running`,
   `backoff`, `stopping`, `stopped`, `exited` and `failed`.
   - Deploy writes desired `running` durably, then `starting`, then `running` when the spawn is
     proved.
   - An end of the tree: with `never`, `exited`. With `on-failure`, status 0 is `exited`; any other
     status, a signal, or an end at `memory.max` or `pids.max` is `backoff`; a signal the daemon sent
     for a stop is not a failure. With `always`, `backoff` unless desired is `stopped`.
   - Back-off (W4) is `min(1,000 ms x 2^k, 60,000 ms)` with `k` the consecutive ends since a run
     that lasted 30,000 ms; after 100 consecutive ends without such a run the state is `failed`.
     `start` resets it. The constants are `WORKLOAD_BACKOFF_BASE_MS`, `WORKLOAD_BACKOFF_MAX_MS`,
     `WORKLOAD_STABLE_MS` and `MAX_WORKLOAD_CONSECUTIVE_RESTARTS`. `restarts` counts every start
     after the first.
   - Stop: desired `stopped`, `stopping`, signal the cgroup, wait the grace interval, `cgroup.kill`,
     observe emptiness (design 04 § 7), `stopped`. From `backoff`, `failed` or `exited` it goes
     straight to `stopped`; stopping a stopped workload answers the record.
   - Start from `stopped`, `exited` or `failed` makes desired `running` and `starting`; from any
     other state it answers the record unchanged. Restart is stop then start under one operation; a
     failed stop does not start.
   - Delete (W8) is allowed when the state is `stopped`, `exited` or `failed` and no cgroup of the
     workload exists, else `workload.not-stopped` (409). It removes the record, the ring and the
     `image_holds` row.

6. **Confinement is exactly an exec's.** A run is built by the same `command` with the image view of
   ADR 0055, in its own cgroup named from the workload id and a run counter, with `pids.max`,
   `memory.max`, `memory.swap.max` 0 and `cpu.max`, the seccomp program, the non-nestable user
   namespace, the cleared environment and a network namespace with loopback only.

7. **Which supervisor actions are ledger operations** (W5), following the host-process precedent.
   - A lease expiry is a stop the daemon originates under its own actor, recorded durably before the
     driver acts. Desired becomes `stopped`, the stop runs, the event reason is `lease-expired`, the
     cgroup is observed empty and the record stays (W8).
   - **A restart by policy and a relaunch after a daemon restart are not operations.** A crash loop
     would write a row per restart against a subject bound of 100,000 rows and would then refuse
     every other mutation of the subject. They are events and `restarts`. The event cause is the
     operation that last put the workload at desired `running`: the deploy, start or restart.

8. **After a daemon restart a workload is relaunched** (W6). An exec's child dies with the daemon and
   a workload on that confinement does too: a workload does not survive its daemon. Reconciliation
   runs at startup before the workload routes answer, for each record with desired `running`:
   1. `observed` is `null` and `probed_at` is cleared.
   2. A cgroup of the workload that exists is killed with `cgroup.kill`, waited empty and removed.
      **No relaunch happens before that removal**; a run's cgroup is created before its spawn, so the
      absence of every cgroup of the workload proves no process of it. If removal fails the state
      stays `null` and the supervisor retries with the back-off; it never spawns while a cgroup of
      the workload exists.
   3. Then `starting`, the run counter advances, the previous `last_exit` is `cause:
      daemon-restart`, and `restarts` counts it. The restart policy is not applied: `never`
      relaunches too, because desired is `running`.

   Desired `stopped` is left alone and a leftover cgroup is removed. A deploy that is durable but
   was never dispatched is started by the same pass; unlike a host process it is not `abandoned`,
   because its spec is stored. A relaunch re-checks the predicates of the spec (`workload.supervised`,
   `image.archive`, an aperture) against the current snapshot; one that no longer holds ends the
   workload `failed`, `cause: start-failed`, with the refusal's code in the event.

9. **Events and logs.** The event union grows one variant per transition, as ADR 0036 S12 did for
   `process`: closed `resource_kind: workload`, a new `workload-id` (`^wl_[A-Za-z0-9]+$`, at most 32
   characters), the fields every variant has and an observation `workload-transition` of state,
   `restarts` and `last_exit`. Transitions: `workload.deployed`, `.running`, `.backoff`, `.exited`,
   `.failed`, `.stopping`, `.stopped`, `.lease-expired`, `.deleted`. None carries `env` or a log
   byte. Stdout and stderr of every run go to one ring of `MAX_WORKLOAD_LOG_BYTES` (8 MiB), lines cut
   at 16 KiB, a read at most 1 MiB (W10); it survives program restarts and ends with the record.

10. **Refusals and their order** on a deploy, each before dispatch and recorded as refused:
    1. `request.schema-invalid` (422): shape, bounds, a `timeout_ms` or `cpu_millis` member.
    2. `workload.unserved` (501): the fact is absent. This is the portable lane's answer.
    3. `workload.limit` (429): `MAX_WORKLOADS_PER_SUBJECT` (32) or `MAX_WORKLOADS_PER_DEPLOYMENT` (128).
    4. `workload.cpu-over-ceiling` (422): `cpu_millicores` above `exec_cpu_cores`.
    5. `workload.name-taken` (409).
    6. `image.not-found` (404) or `image.archive-unserved` (501).
    7. Members of later slices, in their slice's order.
    8. From the driver at dispatch: `image.mount-target-absent` and `image.env-pwd-unserved`.

    A stop, start, restart or delete of an absent workload or another subject's is
    `workload.not-found` (404); delete's own refusal is `workload.not-stopped` (409). Each code is a
    wire constant with a vector.

## Alternatives

- *Docker-backed workloads only (D1 B).* The planned `story:docker-image-backed-workload-slice`. Needs
  a container daemon per node and is root-equivalent to its host (design 04 § 1).
- *No family; the consumer re-starts long execs (D1 C).* Makes the consumer the supervisor, and a
  daemon restart then kills every service with nobody to relaunch it.

## Consequences

- **A workload does not survive its daemon.** A daemon restart relaunches it after the old cgroup is
  proved removed; the service is down for the restart and its in-memory state is lost.
- `env` is at rest on the node, in the `workloads` table, which ADR 0036 avoids for host processes.
  The contract declares `env` non-secret and the ledger holds no input, so the ledger does not hold
  it; the table does.
- A crash loop costs events and a counter and no ledger rows. The cost is that a policy restart has
  no operation of its own to cite.
- **What stays true of the isolation floor.** A workload is confined exactly as an exec is: the
  unprivileged non-nestable user namespace, the seccomp program with the `ioctl` refusals of
  [ADR 0050](0050-the-floor-refuses-the-ioctl-requests-that-exchange-file-blocks.md), pids and
  memory+swap bounds, the cleared environment, no egress, output draining and whole-tree kill with
  the emptiness proof, owner-private durable state. Two items differ by design, and both are stated,
  not hidden: the cumulative CPU budget and the timeout are not applied to a workload, which runs
  until stopped, so its CPU bound is the `cpu.max` rate and its lifetime bound is the lease and the
  stop (W9); and a workload has no workspace in the first slice. Weakening any other floor item is invariant 3's named
  refusal, never a quiet downgrade.
- Delete is refused while a process may exist; the caller stops first. This is the reversible
  choice.
- `AGENTS.md` invariant 6, the gate section and the lane counts of the runtime vectors move with
  the successor bundle, as with every successor. Design 01
  § 3.3 and § 8 are edited with the slice.

## Not decided here

- Whether a workload survives a daemon restart. Surviving needs the host-process supervisor's pidfd
  re-attachment under an exec's confinement and no `--die-with-parent`.
- Volumes and `mounts` (design 38 § 7, a later slice with no record of its own in this set), the
  endpoint of a workload (ADR 0057), per-run secret values (ADR 0058).
- `workload.replace`, a WebSocket log stream, a workspace mounted into a workload.
- Metrics for workloads ([ADR 0045](0045-operational-health-and-metrics-use-a-separate-listener.md)),
  the Kubernetes namespace driver, the Docker driver and placement on more than one host.
- Whether to share or copy the host-process log ring (`host_process/log.rs`); that is the story's.
- The number of the successor bundle, taken when it is cut.
- Inferred in design 38 § 16 and proved by the slice: that `cpu.max` from `cpu_millicores` behaves as
  `exec_cpu_quota` does with no wall limit (I15), and that the run's mapped uid is the daemon's, so
  a service that insists on root ownership fails (I9).
