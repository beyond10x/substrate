# Design 36: limit hits on the confinement adapters

Status: accepted on 2026-11-24. The first consumer's session answered the five questions of § 11,
each with the choice this document is written with
(`approval-record:first-consumer-answers-the-questions-of-design-36-2026-11-24`). Written
against `main` at `c407457cc`. Serves three stories of
`epic:runs-are-governed-and-limit-hits-are-events`:

1. `story:run-observation-names-the-limit-that-ended-it` (§ 2),
2. `story:process-count-limit-hit-is-observed` (§ 3),
3. `story:audit-records-run-end-and-limit` (§ 4).

It changes no wire byte and no contract bundle: the observation and the audit record are
substrate-host's confinement API
([ADR 0044](../../adr/0044-confinement-is-a-contract-family-served-by-declared-adapters.md)
decisions 7 and 8). § 10 lists the decisions, § 11 the five questions that have more than one
defensible answer, § 12 every claim that is inferred and not read in this tree, § 14 what was not
read.

The requirement, relayed 2026-11-21: "hard limits per run and per action (memory, CPU, wall time,
process count) … and every limit hit visible as an event". The first consumer runs on the
in-process `linux` adapter, not on the daemon.

A run through `substrate_host::confinement` can be bounded today. What is missing is the answer
afterwards: an ended run's observation carries its exit and nothing about a bound, and the audit
sink hears of a run's start and never of its end. This design adds three things to the adapter
API: the limit that ended a run, the number of process creations the process bound refused, and
one audit record per ended run that carries both.

## 1. Evidence: what the tree does at `c407457cc`

| what | where |
|---|---|
| `RunObservation` holds `run`, `adapter`, `inner`, `declared_insecure`, `state`, `exit` and `memory_oom_kills`. It is not `#[non_exhaustive]` and does not derive `Serialize` | [`confinement/mod.rs`](../../crates/substrate-host/src/confinement/mod.rs), `RunObservation` |
| `RunState` is `Running`, `Ended`, `Unknown`; `RunExit` is `code` and `signal`. Neither derives `Serialize` | the same file, `RunState`, `RunExit` |
| `AuditRecord` has four variants: `ProfileAdmitted`, `ProfileRefused`, `RunStarted`, `PrivateTmpdirLeft`. It serializes with the tag `event` in snake case and is not `#[non_exhaustive]` | the same file, `AuditRecord` |
| `AuditSink` is a synchronous `Fn(&AuditRecord)`. Its documentation: called on the thread that admitted or started, and for `private_tmpdir_left` on the task that ended the run; a sink that blocks blocks that call | the same file, `AuditSink` |
| a record is written only where the profile has a sink | the same file, `audit` |
| `ConfinementAdapter` is a public trait that is not sealed; two integration tests implement it | the same file; `tests/security_review_confinement.rs` (`CopiedProof`), `tests/adversary_confinement_s0.rs` (`SystemdAroundLocal`) |
| the bounds of a profile are `ProcessBounds` (`timeout_ms`, `processes`, `memory_bytes`, `cpu_millis`) and `IoBounds` (`output_bytes`). There is no type named `Limits`; the family is `ContractFamily::Limits` | [`confinement/profile.rs`](../../crates/substrate-host/src/confinement/profile.rs) |
| the host driver ends an exec at its timeout, at its cumulative CPU budget (`cpu.stat` read each millisecond), at an aperture's byte ceiling, at a stalled live output queue and at a terminal output bound | [`process.rs`](../../crates/substrate-host/src/process.rs), `wait_for_child` |
| the host driver names the bound in `resource.refusal`, in this order of calls: `record_aperture` (`exec.aperture-byte-limit`), `record_pipe_backpressure` (`session.output-backpressure`, which replaces a code already set), `record_terminal_output_bound` (`session.output-limit`, only where no code is set), `record_resource_bound` (only where no code is set: `exec.metrics-unavailable`, else `exec.memory-limit`, else `exec.cpu-limit`, else `exec.timeout`), `record_workdir_missing` (only where no code is set) | the same file, `run_child` and the five functions |
| the host driver reads the cgroup's OOM count (`memory.events` `oom_kill`) for an exec that asked for no measurements, before `reconcile_cgroup` removes the cgroup. A counter that cannot be read counts as zero there | the same file, `run_child` (`unmeasured_oom_kills`), `Cgroup::memory_oom_kills` |
| the host driver reads `pids.peak` and `pids.events` `max` only into `ResourceUsage`, which exists only for an exec that asked for measurements | the same file, `Cgroup::resource_usage`; [`substrate-wire`](../../crates/substrate-wire/src/lib.rs), `ResourceUsage.process_limit_hits` |
| an exec's cgroup has `pids.max`, `memory.max`, `memory.swap.max` 0 and `memory.oom.group` 1 | the same file, `Cgroup::create` |
| `ExecObservation` holds the wire `resource` and host-side members beside it (`stdout_kept`, `output_complete`, `cgroup`, `leader_pid`) | [`observation.rs`](../../crates/substrate-host/src/observation.rs) |
| `linux` asks the driver for no measurements. Its observation maps the exec's state and exit, sets `memory_oom_kills: None` and does not read `resource.refusal` | [`confinement/linux.rs`](../../crates/substrate-host/src/confinement/linux.rs), `exec_input`, `observation` |
| `linux` has a task per run only for a piped standard input: it awaits `ProcessRuntime::wait_ended` and ends the writer | the same file, `start_run` |
| an adapter start has no terminal and no unrecorded output, so `session.output-limit` is not recorded on it | [`process.rs`](../../crates/substrate-host/src/process.rs), `start_for_adapter` |
| `systemd` around `linux` admits the profile on the inner adapter without the sink and writes the records itself; observations are relabelled | [`confinement/systemd.rs`](../../crates/substrate-host/src/confinement/systemd.rs), `SystemdLinux::inner_admission`, `relabel` |
| `systemd` around `local` makes one cgroup per run with the same four files, ends the run at its deadline or CPU budget from one task without recording which, and removes the cgroup when the run ended | the same file, `create_run_cgroup`, `SystemdLocal::watch`, `drain_tree` |
| `oci` passes `--pids-limit`, `--memory`, `--memory-swap` and `--cpus` (a rate). Its monitor ends the run at its own deadline and then reads the runtime's state, which holds the flag `OOMKilled` and the exit code | [`confinement/oci.rs`](../../crates/substrate-host/src/confinement/oci.rs), `monitor`, `ContainerState` |
| `pod` renders `activeDeadlineSeconds` and CPU as a rate. It sets `memory_oom_kills` to 1 when the container's `terminated.reason` is `OOMKilled` and to 0 otherwise. Its monitor asks for a stop one minute past the deadline through the function an embedder's `signal` uses. It does not read the Pod's `status.reason` | [`confinement/pod.rs`](../../crates/substrate-host/src/confinement/pod.rs), `ended`, `monitor`, `request_stop`, `Stop` |
| `local` and `macos` serve no `limits` family and start no timer for a run | [`confinement/local.rs`](../../crates/substrate-host/src/confinement/local.rs), [`confinement/macos.rs`](../../crates/substrate-host/src/confinement/macos.rs) |
| every adapter that reads the run's pipes itself ends a run whose live output stalled, and keeps the fact | [`confinement/live.rs`](../../crates/substrate-host/src/confinement/live.rs), `LiveOutput::stall`, `LiveOutput::stalled` |
| every adapter but `linux` has one task per run that publishes the run's end on a watch channel. Each adapter writes `run_started` after it spawned that task | `local.rs`, `macos.rs`, `systemd.rs`, `oci.rs`, `pod.rs`: the `phase.send` of each and the `audit_start` call after it |
| `macos` already writes one record from the run's end task, with a clone of the admitted profile | `macos.rs`, `remove_private_tmpdir` |
| no adapter has a `Drop` that ends its runs | `grep 'impl Drop'` over `confinement/`: only `LinuxHold` in `systemd.rs` |
| the specification models opening an adapter and admitting a profile. It has no run, no observation and no `run_started` | [`spec/confinement/confinement.yaml`](../../spec/confinement/confinement.yaml) |

## 2. The limit that ended a run

### The field

**D1. `RunObservation` gains `limit: Option<RunLimit>`.** `RunLimit` is an enum of this crate with
five variants. The field is `None` while the run runs, `None` for a run no limit ended, and `None`
where the adapter did not observe which limit ended it. A consumer cannot tell the last two apart
from the field; the table of D6 says per adapter which limits it observes.

```rust
/// A bound that ended a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum RunLimit {
    /// `ProcessBounds.memory_bytes`: the kernel's OOM killer ended the run.
    Memory,
    /// `ProcessBounds.cpu_millis`: the run's cumulative CPU time reached its budget.
    CpuTime,
    /// `ProcessBounds.timeout_ms`: the run's deadline passed.
    WallTime,
    /// The byte ceiling of an egress aperture the run used.
    EgressBytes,
    /// The run's live output waited unread past the stall deadline.
    OutputStall,
}
```

Reason. One field with a closed set of names is what a consumer can match on. A string would
repeat the host driver's refusal codes, which are wire identifiers of another surface. The set is
closed in the sense that this crate defines every member; it is `#[non_exhaustive]` so a later
bound (a rate bound is reserved, ADR 0109 F1 B) adds a variant without breaking a consumer's
`match` (Q2).

**D2. The limit is named on an `Unknown` run as well.** Where the adapter observed the limit and
then could not establish that every process is gone, the observation has `state: Unknown`, no
exit, and the limit. It then reads: this bound was reached and the adapter ended the run for it;
how the run ended is not known.

Reason. The hit is a fact of its own. The host driver already keeps the refusal on an exec whose
state it sets to `Unknown` (`run_child` sets the state after `record_resource_bound`).

**D3. The process-count bound is no variant.** The kernel refuses the creation and the program
carries on. § 3 reports it as a count. The output byte bound of `IoBounds` is no variant either:
it cuts the record and ends nothing, and `RunOutput.stdout_truncated`, `stderr_truncated` and
`OutputEvent::Bounded` already say so.

### More than one limit

**D4. Where more than one limit is true at the end, the one named is the first of: output stall,
egress bytes, memory, CPU time, wall time.** This is the order in which the host driver names its
refusal today (§ 1, the `record_*` row), and every other adapter applies the same order to the
limits it observes.

Reason. `linux` is the first consumer's adapter and its order is released behaviour of the wire
`refusal`. An adapter with a different order would name another limit for the same run. The
story's "memory, CPU, timeout" is the tail of this order.

### The aperture byte ceiling and the output stall

**D5. Both are variants: `EgressBytes` and `OutputStall`.** `linux` maps
`exec.aperture-byte-limit` and `session.output-backpressure` to them. The adapters that read the
run's pipes themselves name `OutputStall` from `LiveOutput::ended_run`. A stall is named only where
it came before the leader's exit: a program that exits on its own and whose unread output then
stalls names no limit, while `output_stalled` and `OutputEvent::Stalled` still report the stall.
Nor does a stall whose kill comes after the embedder's `KILL` (`local`, `macos`, `systemd` around
`local`, `oci`): that run names no limit (D1).
On `linux` the driver marks a backpressure it flagged after the child had exited on its own,
whether or not its supervision had taken the exit yet: asked under the lock its reap holds, so it
never asks about a reaped pid (`EndOfRun::output_stall_after_exit`). A backpressure flagged after
the supervision began to end the run by a bound of its own is not marked, and D4 names the stall.
Such an exec names the limit that ended it, or none when it ended by itself: in D4's order, the
aperture ceiling where the driver reached it (`EndOfRun::egress_ceiling_reached`), then memory
where the OOM count is above zero. Its record keeps `session.output-backpressure`. The OOM count
names `Memory` there also where the run's other counters could not be read: the driver reads it
from the cgroup by itself (`Cgroup::end_of_run` falls back to the `oom_kill` counter), so it is a
measurement the driver has, and the record carries no `exec.metrics-unavailable` to rank (D6).

Reason. Both end a run by the adapter's own hand: the driver kills the cgroup. Left absent, such a
run would read as killed by signal 9 with no cause, which is the case this design exists to
remove. The stall is already observable by a second route (`output_stalled`, `OutputEvent::Stalled`,
[design 32](32-adapter-stdin-and-live-output.md)); the variant puts it where the other causes are
and on the audit record. Listed as Q1.

### How `linux` maps the host observation

**D6. `linux` maps the code of the ended exec's `resource.refusal` with one function, and every
other code is `None`.**

| code on the ended exec | `limit` |
|---|---|
| `session.output-backpressure` (`substrate_wire::SESSION_OUTPUT_BACKPRESSURE`) | `OutputStall` |
| `exec.aperture-byte-limit` | `EgressBytes` |
| `exec.memory-limit` | `Memory` |
| `exec.cpu-limit` | `CpuTime` |
| `exec.timeout` | `WallTime` |
| `exec.metrics-unavailable` | `None`: the driver could not measure the run; the state is `Unknown` |
| `session.output-limit` (`SESSION_OUTPUT_LIMIT`) | `None`: not recorded on an adapter start (§ 1) |
| the working-directory code of `record_workdir_missing` (`WORKDIR_REFUSALS[2]`) | `None`: the program did not start; no bound ended it |
| any other code, and no refusal | `None` |

Reason. The driver already decided which bound ended the exec, from its own timer, its own CPU
reading, its relay's counters and the cgroup's OOM count. The adapter repeats none of that. The
function is total over strings, so a code a later driver adds is `None` until a variant exists
for it, never a guess. The driver names `exec.metrics-unavailable` before a memory, CPU or
wall-time bound; on such an exec a bound that was also reached is not named, and the state is
`Unknown`. That rule is about the code on the record: an exec whose stall is marked after its
exit carries `session.output-backpressure` instead, and names the limit D5 reads from its end.

### Per adapter

**D7. One row per adapter. No adapter derives a limit from an exit code or a signal alone.**
"absent" means the field stays `None` for that cause.

| adapter | `Memory` | `CpuTime` | `WallTime` | `EgressBytes` | `OutputStall` |
|---|---|---|---|---|---|
| `linux` | the driver's `exec.memory-limit`: `memory.events` `oom_kill` above zero in the exec's cgroup; under a stall after the exit, the end's OOM count above zero (D5) | the driver's `exec.cpu-limit`: its supervision read `cpu.stat` at or above the budget, at a tick or when its timer fired, and killed the cgroup | the driver's `exec.timeout`: its own timer fired before the child's exit and below the CPU budget | the driver's `exec.aperture-byte-limit`: its relay's counters; under a stall after the exit, `EndOfRun::egress_ceiling_reached` (D5) | the driver's `session.output-backpressure`: its live queue flag, unless flagged after the child exited on its own (D5) |
| `systemd` around `linux` | `linux`'s, relabelled | `linux`'s | `linux`'s | `linux`'s | `linux`'s |
| `systemd` around `local` | `memory.events` `oom_kill` above zero in the run's cgroup, read by the run's end task before the cgroup is removed | its watch task records that the CPU check fired, before it kills, while the leader has not exited and the embedder has not stopped it | its watch task records that the deadline fired, on the same terms | absent: it starts no aperture | `LiveOutput::ended_run` |
| `oci` | the runtime's state has `OOMKilled` true and the run's exit, as the adapter reads it, is a signal: an exit status of 129 to 192, read as signal status − 128, or, beside `OOMKilled`, any status outside 0 to 128 (193 and above, or negative), read as `SIGKILL` | absent: `--cpus` is a rate, and no cumulative budget ends a run | its monitor's own deadline fired before the runtime client returned, before it kills the container | absent | `LiveOutput::ended_run`: the stall came before the runtime client returned |
| `pod` | the container's `terminated.reason` is `OOMKilled` and its exit is a signal or a code above 128, whoever stopped the run | absent: a rate | the Pod's `status.reason` is `DeadlineExceeded`, or the monitor's own stop past the deadline was the one that ended it | absent | absent: no live output (design 32 D10) |
| `local` | absent: no `limits` family | absent | absent | absent | `LiveOutput::ended_run` |
| `macos` | absent: no `limits` family | absent | absent | absent | `LiveOutput::ended_run` |

Notes on the rows.

- `linux`: the CPU bound of a profile is held twice by the driver. The cumulative budget ends
  the run and is what `CpuTime` names. `exec_cpu_quota` in
  [`process.rs`](../../crates/substrate-host/src/process.rs) also writes `cpu.max`: the declared
  CPU time spread over the declared wall time, at least 1 ms in each period of 100 ms and at most
  `HostConfig::exec_cpu_cores` whole periods. So a run whose budget is at least one hundredth
  of its timeout reaches the budget no earlier than the timeout, and where both are true in one
  check D4 names `CpuTime`. A run whose budget is under that share can reach it earlier. The
  first consumer's session named this on 2026-11-24 and accepted the order. The rate is not
  changed here (§ 13).
- `linux`: where the child's exit and the timer are ready at once, the driver takes the exit, so
  a child that exited before its deadline is never recorded timed out; where the timer fires
  with the CPU budget passed, the run is recorded CPU-exhausted (D4).
- `systemd` around `local`: the watch task tests the CPU budget before the deadline, so one tick
  in which both are true names `CpuTime` (D4). It decides under the lock that marks the leader's
  exit: a run whose leader exited, by itself or by the embedder's kill, names no limit, also when
  a bound passes while the end task still drains the tree. The run's cgroup has `memory.oom.group` 1, so an
  OOM kill ends every process in it. The run is declared insecure, and the adapter's module
  documentation says what such a run can do to its own cgroup; the limit and the counters are
  those of the cgroup the run was started in.
- `oci`: the adapter sets no group kill, and whether the runtime's flag means the whole container
  ended is not read here (§ 12). So the flag alone is not taken as the end of the run: the run's
  main process must have ended by a signal as well. The adapter reads an exit status of 129 to
  192 as a signal, and, beside `OOMKilled`, any status outside 0 to 128 (193 to 255, or a
  negative one) as `SIGKILL`, so a main process that itself exits with such a status after a
  child's OOM kill names `Memory`; one that exits with 0 to 128 names none. Where the runtime client's
  return and the deadline are ready at once, the monitor takes the return. A stall's kill asked
  after the embedder's `signal(Kill)` set its flag, before that call reached the runtime, names
  no stall; one asked before names the stall. No lock orders the two runtime-client calls, so
  which reaches the container first is the runtime's.
- `pod`: the same rule, with the exit the container status reports: the flag beside a signal or
  an exit code over 128 names `Memory`, whoever stopped the run, so a main process that itself
  exits with a code over 128 after a child's OOM kill names it too. The monitor's stop past the
  deadline goes through `request_stop`, as an embedder's `signal` does; the `Stop` record gains
  its origin so the two are told apart. An embedder's own `signal` names no limit on any adapter.
- `pod` reads `status.reason` at the end. That the kubelet writes `DeadlineExceeded` there is not
  read in this tree (§ 12); the pod lane's case holds it.

### `memory_oom_kills`

**D8. `linux` and `systemd` report `memory_oom_kills` for an ended run; `oci`, `local` and
`macos` keep it `None`; `pod` keeps what it reports today.**

| adapter | `memory_oom_kills` |
|---|---|
| `linux` | `ExecObservation.end_of_run.memory_oom_kills` (D9): `Some(n)` where the driver read the counter, `None` where it could not |
| `systemd` around `linux` | `linux`'s |
| `systemd` around `local` | `memory.events` `oom_kill` of the run's cgroup, read once by the end task; `None` where the read failed |
| `oci` | `None`: the runtime's state holds a flag, not a count |
| `pod` | unchanged: `Some(1)` for `OOMKilled`, else `Some(0)`, `None` where the container status never appeared |
| `local`, `macos` | `None` |

The field stays `None` while the run runs, as its documentation says today.

### The host-internal end-of-run member

**D9. `ExecObservation` gains `end_of_run: Option<EndOfRun>`, outside `resource`.**

```rust
/// What the host driver read from the exec's own cgroup when the exec ended.
/// Host-internal: no wire member carries it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EndOfRun {
    /// `memory.events` `oom_kill`. `None` where the file could not be read.
    pub memory_oom_kills: Option<u64>,
    /// `pids.events` `max` (§ 3). `None` where the file could not be read.
    pub process_limit_hits: Option<u64>,
}
```

- It is `Some` only on an observation whose end `run_child` recorded. It is `None` while the exec
  runs, on an observation rebuilt from a stored record, and on an observation another driver made.
- The driver reads both counters once for every exec, measured or not, at the place where it
  reads `unmeasured_oom_kills` today: after the exec's leader exited and before `reconcile_cgroup`
  removes the cgroup. A measured exec takes the two values of its terminal `ResourceUsage`.
- A counter that cannot be read is `None` in the member (invariant 3: absent, never zero). What
  the driver names as the refusal does not change: an unreadable OOM counter still names no
  memory limit.
- `resource` and every wire type are untouched. `ResourceUsage` keeps being published only for
  an exec that asked for measurements.
- `story:run-observation-names-the-limit-that-ended-it` adds the member with `memory_oom_kills`.
  `story:process-count-limit-hit-is-observed` adds `process_limit_hits` and its read.
  `story:run-observation-carries-usage` extends the same struct.

Reason. The counters live in a directory the driver removes before it publishes the end. The
adapter cannot read them afterwards, and the wire `resource` must not grow a member for an
in-process reader. The other host-side members of `ExecObservation` are the precedent.

Cost. `ExecObservation` is built with a struct expression in about forty places in four crates
(`grep 'ExecObservation {'` over `crates/`: the host driver, the daemon, the Kubernetes driver,
the SDK and their tests). Each gains `end_of_run: None`.

## 3. The process count

**D10. What is observed is the number of process creations the run's process bound refused: the
`max` counter of `pids.events` in the run's cgroup.** The peak number of processes is not part
of this section; it is a usage figure and belongs to `story:run-observation-carries-usage`.

Reason. The requirement asks for the hit. The counter is the kernel's own count of it and needs
no sampling. The wire already names the same counter `process_limit_hits`.

**D11. `RunObservation` gains `process_limit_hits: Option<u64>`.** `Some(0)` says the adapter
read the counter and nothing was refused. `None` says the adapter does not read it or could not
(invariant 3).

**D12. A hit never ends a run and never sets `limit`.** A run that hits its process bound and
then its deadline has `limit: Some(WallTime)` and `process_limit_hits` above zero. This is the
shape the epic's acceptance test reads.

**D13. A running observation carries the count on the adapters that read it.** `observe` of a
running run reads the counter at that moment. A consumer that needs the count before the end
polls `observe`. `wait` and the ended observation carry the final value. Listed as Q3.

- `linux`: the driver gains a crate-private read of `pids.events` for a running exec, through
  `Cgroup::existing`, as `ProcessRuntime::observe` reads the usage of a measured exec today. The
  adapter's `observe` calls it for a running run. The adapter's `wait` loop polls every 20 ms
  (`WAIT_POLL`) and does not call it.
- The ended value is `end_of_run.process_limit_hits` (D9).
- A read that fails gives `None` for that observation; it does not fail `observe`.

**D14. Which adapters report it.**

| adapter | `process_limit_hits` | source |
|---|---|---|
| `linux` | reported, running and ended | `pids.events` `max` of the exec's cgroup. Every `linux` run has a process bound (the profile's or `DEFAULT_BOUNDS`), so the counter always exists |
| `systemd` around `linux` | `linux`'s | |
| `systemd` around `local` | reported, running and ended | `pids.events` `max` of the run's cgroup; the end task reads the final value before the cgroup is removed |
| `oci` | `None` | the adapter passes `--pids-limit`; the state it reads from the runtime holds no such counter |
| `pod` | `None` | the bound is the operator's `pod_pids_limit`; the Pod status the adapter reads holds no such counter |
| `local`, `macos` | `None` | no `limits` family |

The final value is the counter as read when the run's leader had exited. Descendants are killed
directly after the read; a creation refused between the two is not counted. The driver's
terminal usage sample has the same order today (`run_child`).

## 4. The `run_ended` audit record

### The record

**D15. `AuditRecord` gains the variant `RunEnded`, serialized with `"event": "run_ended"`.** It
is the run's ended observation plus the profile's id.

```rust
RunEnded {
    run: RunId,
    profile: ProfileId,
    adapter: AdapterName,
    #[serde(skip_serializing_if = "Option::is_none")]
    inner: Option<InnerAdapter>,
    /// The reason of a declared-insecure run, as `run_started` carries it.
    #[serde(skip_serializing_if = "Option::is_none")]
    declared_insecure: Option<String>,
    /// `ended` or `unknown`. Never `running`.
    state: RunState,
    #[serde(skip_serializing_if = "Option::is_none")]
    exit: Option<RunExit>,
    #[serde(skip_serializing_if = "Option::is_none")]
    limit: Option<RunLimit>,
    #[serde(skip_serializing_if = "Option::is_none")]
    memory_oom_kills: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    process_limit_hits: Option<u64>,
}
```

- `RunState` and `RunExit` gain `Serialize`. `RunState` serializes in snake case; an absent
  `code` or `signal` of `RunExit` is omitted.
- Every optional member is omitted when absent, as in the existing variants. A reader takes a
  missing `limit` as "none named" and a missing count as "not read", as on the observation.
- The record is built from the `RunObservation` the adapter's own observation function returns
  for the ended run. A consumer that reads the record learns what `observe` would say.
- `story:audit-records-run-end-and-limit` adds the variant with every member but
  `process_limit_hits`; `story:process-count-limit-hit-is-observed` adds that member.
  `story:run-observation-carries-usage` adds its members to the observation and the record
  together.

A run on `linux` that its memory bound ended:

```json
{"event":"run_ended","run":"…","profile":"…","adapter":"linux","state":"ended",
 "exit":{"signal":9},"limit":"memory","memory_oom_kills":1,"process_limit_hits":0}
```

### Once per run

**D16. `AdmittedProfile::audit_start` returns a crate-private value that writes the run's one
`run_ended`.** Only `audit_start` makes the value, it is not `Clone`, and writing consumes it. So
a run that received no `run_started` gets no `run_ended`, and no run gets two. `observe` and
`wait` never write a record.

**D17. `run_ended` follows `run_started`.** Every adapter writes `run_started` after it spawned
the run's end task (§ 1). The adapter therefore hands the value of D16 to the end task after
`audit_start` returned, through a one-shot channel. The end task takes it after the run ended. A
start that failed after the task was spawned sends nothing, and the task writes nothing.
`linux` and `systemd` around `linux` have no end task before the run started (D20): they spawn
theirs after `audit_start` returned and move the value into it, with no channel, and spawn none
where the profile has no sink (D19). Amended 2026-11-24 with the change that built the record.

**D18. The record is written for a run that ends `Unknown` as well.** The end task writes it on
every path on which it publishes the run's end, `Ended` or `Unknown`. For `Unknown` the record
has no exit, and the limit and the counters where the adapter observed them (D2).

**D19. A profile without a sink writes nothing, and costs nothing.** `audit` already writes only
where `profile.audit` is set. Without a sink `audit_start` returns an inert value, and `linux`
spawns no task for it.

### Which task calls the sink

**D20. The run's end task writes the record, after it published the run's end, and the sink runs
on the runtime's blocking pool.**

| adapter | the task |
|---|---|
| `local`, `macos`, `systemd` around `local`, `oci`, `pod` | the task that publishes the run's phase today. `macos` writes `private_tmpdir_left` first, as today |
| `linux` | a new task per run that has a sink: it awaits `ProcessRuntime::wait_ended`, observes the exec once and writes. It is the shape of the task that ends a piped standard input today |
| `systemd` around `linux` | a task of the outer adapter: it awaits the inner run's end and writes with its own admitted profile and the relabelled observation. The inner adapter has no sink (§ 1) |

- The run ends whether or not anybody calls `observe` or `wait`; the tasks above do not depend
  on a caller.
- The end is published first. `wait` on the adapters with a phase channel returns from that
  channel; `wait` on `linux` polls the driver. Neither waits for the record.
- The sink is called through `tokio::task::spawn_blocking`. A sink that blocks then holds one
  thread of the blocking pool and no worker of the runtime, so it delays neither `wait` nor
  another run, also on a current-thread runtime.
- The consequence a consumer must know: `run_ended` may arrive after `wait` returned. A consumer
  that needs the record waits for its own sink. Listed as Q4.
- `AuditSink`'s documentation gains the sentence for `run_ended`. The other records keep their
  thread.

### A dropped adapter, and a runtime that ends first

**D21. Dropping an adapter does not end its runs and does not lose their records.** No adapter
ends a run when it is dropped today (§ 1), and this design does not change that. The end task
owns what it needs: the value of D16 holds a clone of the admitted profile and with it the sink.
The record is written when the run ends.

**D22. A value of D16 that is dropped unwritten writes `run_ended` with `state: unknown`.** This
is the case of a tokio runtime that shuts down while the run still runs: the end task is dropped
with it. The record then has no exit, no limit and no count, and is written synchronously on the
thread that drops the task. It says what `RunState::Unknown` means: the adapter lost sight of
the run. Nothing is written from a thread that is panicking. A process that exits or is killed
writes nothing; a consumer that must account for that pairs `run_started` with `run_ended` in
its own store. Listed as Q5.

### What the record is not

**D23. A declared-insecure run's record carries the reason in `declared_insecure`,** as its
`run_started` and its observation do. Its limit and counts follow the adapter's row: none on
`local`, the run cgroup's on `systemd` around `local`.

**D24. No record is written for a denied execution or a denied connection, and none while the
run runs.** The epic puts both out of scope. A hit of the process bound produces no record at
the moment of the hit; the count reaches the sink on `run_ended`, and before that a consumer
polls `observe` (D13). [Design 35](35-executable-lists.md) § 11 lists the options for a denied
execution and designs none.

## 5. The specification

**D25. `spec/confinement/confinement.yaml` gains no type, command, event or scenario. It gains
one `UNMAPPED:` block, and `spec/confinement/README.md` one entry under *Open questions*.**

What the model states today: `OpenAdapter` and `AdmitProfile`, two entities, two events. It has
no run. `run_started` is not in it, and the audit record of a refusal is `UNMAPPED` because ESS
refuses an event on an outcome that reports an error. Its own comment says "The model runs no
program".

What the block states, as text:

- the ended observation's `limit`, `memory_oom_kills` and `process_limit_hits`, and the
  `run_ended` record, are `UNMAPPED`;
- the reason: the model has no run entity, and a scenario cannot start a program or reach a
  bound, so no step could produce the values;
- where they are held instead: the per-adapter rows of this document (D7, D8, D14) and each
  adapter lane's cases (§ 8).

Consequences.

- The generated Rust types (`crates/substrate-host/src/generated/confinement.rs`) get no
  `RunLimit`. It is written by hand in `confinement/mod.rs`, as `InnerAdapter` is in `facts.rs`.
  A generated enum could not be `#[non_exhaustive]`.
- `spec/confinement-suite.json` keeps its scenarios. The committed projections carry a digest of
  their source in their header, so a change of the YAML regenerates them in the same change
  (`cargo xtask check-ess`).
- `story:process-count-limit-hit-is-observed` says the specification "states the field and a
  scenario". This decision does not follow that sentence; the story's text needs the amendment.
  A run entity in the model is named in § 13.

## 6. The wire is untouched

No file under `contracts/` or `crates/substrate-wire/` changes. `ExecObservation.end_of_run` is a
member of a host type beside `resource`, not of `Exec`. `cargo xtask check-bundles` reads the
same bytes as before. The wire's own publication of the process count is
`story:exec-observation-reports-process-count-hits`.

## 7. Compatibility for a Rust consumer

None of `RunObservation`, `RunState`, `RunExit` and `AuditRecord` is `#[non_exhaustive]` today.

**D26. `RunObservation` and `AuditRecord` become `#[non_exhaustive]` with this change, and
`RunObservation` gets a constructor.** `RunObservation::new(run, adapter, state)` returns an
observation with every optional member `None`; the fields stay public, so a consumer sets what
it needs. The variant `RunEnded` is `#[non_exhaustive]` too. Listed as Q2.

Reason. The change breaks a consumer's struct expression in any case. The next story of the epic
adds four more fields to the same struct and the same variant. One break now replaces one per
story.

What breaks at compile time, once:

| consumer code | after the change |
|---|---|
| builds `RunObservation { … }` with a struct expression (a test double of the public trait) | does not compile outside this crate; it becomes `RunObservation::new(…)` and field assignments |
| destructures `RunObservation { run, state, .. }` | compiles: the pattern has `..` |
| destructures every field of `RunObservation` without `..` | does not compile; add `..` |
| reads fields of an observation | compiles |
| `match` over `AuditRecord` with a wildcard arm, or `if let` / `matches!` on one variant | compiles |
| `match` over `AuditRecord` naming all four variants and no wildcard | does not compile; add a wildcard arm |
| builds an `AuditRecord` variant that exists today | compiles: only the enum and the new variant are marked |
| `match` over `RunLimit` | needs a wildcard arm from the start |
| implements `ConfinementAdapter` | compiles: the trait gains no method |
| a sink that serializes every record | receives a record with a new `event` value |

In this repository no file outside `crates/substrate-host/src/confinement/` builds a
`RunObservation` (`grep 'RunObservation {'`), and the three test files that name `AuditRecord`
use single-variant patterns. The consumers outside this repository were not read (§ 14).
`crates/substrate-host/tests/confinement-public-items.txt` is regenerated in each story's change,
and each story's `CHANGELOG.md` entry names the break.

The host driver's `EndOfRun` (D9) is public and `#[non_exhaustive]`, so a field it gains breaks
no consumer outside this crate. Story 1 gives it two public fields beside `memory_oom_kills`:
`output_stall_after_exit` and `egress_ceiling_reached` (D5). No public item of the
confinement module names `EndOfRun` or `ExecObservation`, so neither is in that list, and it does
not change for them.

## 8. How it is proved

Budgets: no test runs longer than 60 s, the gate's test step ends within 120 s, a job in the
build pool ends within 10 minutes ([`AGENTS.md`](../../AGENTS.md), § *The gate*). Every run below
is bounded by its own profile at two seconds or less, so each case is expected to take seconds.
No case was run or timed for this document. Each is timed before it is added, and one over 5 s
is named with its time in the change that adds it.

A lane case prints its own `executed` line, and the lane's script requires that line, so an
absent case is never read as passed (invariant 3). The delegated and `systemd` cases are run by
[`scripts/delegated-lane.sh`](../../scripts/delegated-lane.sh); without a delegated cgroup root
they print `ABSENT` under the gate's test step.

### Story 1, the limit

| lane | case | expects |
|---|---|---|
| gate's test step | unit, in `confinement/`: the mapping of D6 over every code the driver writes on an ended exec, and one unknown code | the table of D6, row by row |
| gate's test step | unit, in `process.rs`: the end-of-run read from a cgroup directory written as files | `Some` for a readable counter, `None` for a missing file |
| gate's test step | [`ess_confinement_local`](../../crates/substrate-host/tests/ess_confinement_local.rs): a run that exits 0 | `limit`, `memory_oom_kills` both `None` |
| delegated | [`ess_confinement_linux`](../../crates/substrate-host/tests/ess_confinement_linux.rs): a run that allocates past its memory bound | `Memory`, `memory_oom_kills` at least 1 |
| delegated | the same file: a run that spins past its CPU budget, with a budget under one hundredth of its timeout (D7, the note on `linux`) | `CpuTime` |
| delegated | the same file: a run that sleeps past its timeout | `WallTime` |
| delegated | the same file: a run that exits 0 | `limit` `None`, `memory_oom_kills` `Some(0)` |
| `systemd` | [`ess_confinement_systemd`](../../crates/substrate-host/tests/ess_confinement_systemd.rs), around `local`: memory, CPU, timeout | `Memory` with `memory_oom_kills` at least 1, `CpuTime`, `WallTime` |
| `systemd` | the same file, around `linux`: timeout | `WallTime`, relabelled `systemd` with `inner: linux` |
| `oci` (`SUBSTRATE_OCI_LANE=1`) | [`ess_confinement_oci`](../../crates/substrate-host/tests/ess_confinement_oci.rs): memory, timeout | `Memory`, `WallTime`; `memory_oom_kills` `None` |
| `pod` (`SUBSTRATE_POD_LANE=1`, [`scripts/pod-lane.sh`](../../scripts/pod-lane.sh)) | [`ess_confinement_pod`](../../crates/substrate-host/tests/ess_confinement_pod.rs): memory, deadline | `Memory`, `WallTime` |
| gate's static phase | `cargo xtask check-bundles`; no diff under `contracts/` and `crates/substrate-wire/` | unchanged |

The `pod` deadline case waits for the kubelet to enforce `activeDeadlineSeconds`, whose delay was
not measured. The monitor's own stop comes one minute past the deadline (`DEADLINE_GRACE`), which
no lane case may wait for: it is held by a unit case that hands the monitor a give-up instant in
the past.

### Story 2, the process count

| lane | case | expects |
|---|---|---|
| gate's test step | `ess_confinement_local`: any run | `process_limit_hits` `None` on the observation and on `run_ended` |
| delegated | `ess_confinement_linux`: process bound 4, the run tries to start 16 processes | above zero on the ended observation; one `run_ended` with the same count |
| delegated | the same file: a run under its bound | `Some(0)` on both |
| delegated | the same file: `observe` while the first run still runs, after its creations were refused | above zero, with `state: Running` |
| `systemd` | `ess_confinement_systemd`, around `local`: the first two cases | the same |
| `macos` (the hosted macOS check) | [`ess_confinement_macos`](../../crates/substrate-host/tests/ess_confinement_macos.rs): any run | `None` on both |
| delegated | [`consumer_surface`](../../crates/substrate-host/tests/consumer_surface.rs), public items only: one profile on `linux` with an allow-list, a deny-list and process bounds; the run sees a denied execution fail, exceeds its process bound and then its timeout | the observation and the `run_ended` record each have `limit` `WallTime` and `process_limit_hits` above zero. This is the epic's acceptance test |

### Story 3, the audit record

A case that expects a record waits for it on a channel its own sink writes to, with a bound of a
few seconds (D20).

| lane | case | expects |
|---|---|---|
| gate's test step | unit: the record's JSON | `"event": "run_ended"`, absent members omitted |
| gate's test step | unit: the value of D16 | written once; dropped unwritten it writes `state: unknown`; without a sink nothing |
| gate's test step | `ess_confinement_local`: a run that exits 0, nobody calls `observe` or `wait` | exactly one `run_ended` with no limit, after `run_started` |
| gate's test step | the same file: `observe` and `wait` on the ended run | no second record |
| gate's test step | the same file: a profile without a sink | nothing written |
| gate's test step | the same file, on a current-thread runtime: a sink that blocks on `run_ended` until the test releases it | `wait` returns while the sink is held |
| gate's test step | the same file: the adapter is dropped while the run runs | the record arrives when the run ends |
| delegated | `ess_confinement_linux`: a run that exceeds its memory bound, nobody calls `observe` or `wait` | exactly one `run_ended` with `limit` `memory`, after `run_started` |
| `systemd` | around `local` and around `linux`: one run each, nobody observes | one `run_ended` per run, with the outer adapter's name |
| `oci`, `pod`, `macos` | one run each, nobody observes | one `run_ended` per run |

Every change also regenerates
[`confinement-public-items.txt`](../../crates/substrate-host/tests/confinement-public-items.txt).

## 9. Per adapter, in one table

| adapter | `limit` can be | `memory_oom_kills` | `process_limit_hits` | `run_ended` written by |
|---|---|---|---|---|
| `linux` | all five | read | read | a task of its own per run with a sink |
| `systemd` around `linux` | all five | read | read | a task of the outer adapter |
| `systemd` around `local` | `Memory`, `CpuTime`, `WallTime`, `OutputStall` | read | read | the run's end task |
| `oci` | `Memory`, `WallTime`, `OutputStall` | `None` | `None` | its monitor |
| `pod` | `Memory`, `WallTime` | `Some(0)` or `Some(1)` | `None` | its monitor |
| `local` | `OutputStall` | `None` | `None` | the run's end task |
| `macos` | `OutputStall` | `None` | `None` | the run's end task |

## 10. Decisions

| # | decision |
|---|---|
| D1 | `RunObservation.limit: Option<RunLimit>`; five variants; `None` for running, for no limit and for not observed |
| D2 | the limit is named on an `Unknown` run where the adapter observed it |
| D3 | the process bound and the output byte bound are no variants |
| D4 | of several true limits the first of output stall, egress bytes, memory, CPU time, wall time is named, on every adapter |
| D5 | the aperture byte ceiling and the output stall are variants (Q1) |
| D6 | `linux` maps five refusal codes; every other code is `None` |
| D7 | per adapter: which limits, from which source; none from an exit or a signal alone |
| D8 | `memory_oom_kills` on `linux` and `systemd`; `oci`, `local`, `macos` `None`; `pod` unchanged |
| D9 | `ExecObservation.end_of_run`, host-internal, read for every exec before the cgroup is removed; an unreadable counter is `None` |
| D10 | the process count is `pids.events` `max`; the peak is the usage story's |
| D11 | `RunObservation.process_limit_hits: Option<u64>` |
| D12 | a hit never ends a run and never sets `limit` |
| D13 | a running observation carries the count where it is read (Q3) |
| D14 | `linux` and `systemd` report it; `oci`, `pod`, `local`, `macos` `None` |
| D15 | `AuditRecord::RunEnded`, `"event": "run_ended"`: the ended observation plus the profile id |
| D16 | `audit_start` returns the one value that writes `run_ended` |
| D17 | `run_ended` follows `run_started`; the value reaches the end task after `audit_start` returned |
| D18 | written for `Unknown` as well |
| D19 | no sink: nothing written, no task added |
| D20 | the end task writes after it published the end; the sink runs on the blocking pool (Q4) |
| D21 | dropping an adapter neither ends a run nor loses its record |
| D22 | a runtime that ends first: `run_ended` with `state: unknown` (Q5) |
| D23 | a declared-insecure run's record carries its reason |
| D24 | no record for a denied execution or connection, none while the run runs |
| D25 | the specification gains one `UNMAPPED:` block and nothing else |
| D26 | `RunObservation` and `AuditRecord` become `#[non_exhaustive]`; `RunObservation::new` (Q2) |

## 11. Questions with more than one answer

Each changes what a consumer writes. The choice is the one this document is written with. The
first consumer's session answered all five on 2026-11-24, each with A.

**Q1. The aperture byte ceiling and the output stall.**

- A (chosen): two variants, `EgressBytes` and `OutputStall`. Every end the adapter imposed has a
  name. Cost: `local` and `macos`, which serve no `limits` family, can report a limit, and a
  consumer's `match` has two more arms.
- B: both absent. The enum is exactly the four bounds of the requirement minus the process
  count. Cost: such a run reads as killed by signal 9 without a cause; the stall is found only
  through `output_stalled`, and the aperture ceiling not at all on the adapter API.

**Q2. `#[non_exhaustive]` on `RunObservation` and `AuditRecord`.**

- A (chosen): mark both now and add `RunObservation::new`. Cost: a consumer's struct expression
  stops compiling once and is rewritten to the constructor; later fields break nothing.
- B: leave the types as they are. Cost: every story that adds a field (this design adds two, the
  usage story four) breaks every struct expression and every exhaustive `match` again; each fix
  is one line.

**Q3. The process count while the run runs.**

- A (chosen): `observe` of a running run reads the counter on `linux` and `systemd`. A consumer
  can act on a hit before the end by polling. Cost: one file read per `observe` of a running
  run, and a second read path beside the end-of-run one.
- B: the count appears only on the ended observation and on `run_ended`. Cost: nothing about a
  hit is visible before the end; a consumer that wants to stop a run for it cannot.

**Q4. When the sink is called for `run_ended`.**

- A (chosen): after the end is published, on the blocking pool. `wait` never waits for a sink.
  Cost: the record may arrive after `wait` returned; a consumer that reads its sink right after
  `wait` must wait for the record.
- B: before the end is published, on the end task. After `wait` the record is in the sink. Cost:
  a sink that blocks holds `wait` and every `observe` of that run, and on a current-thread
  runtime every other task. The story rules this out.

**Q5. A runtime that shuts down while a run still runs.**

- A (chosen): the dropped value writes `run_ended` with `state: unknown`. Every `run_started` in
  a process that returns from its runtime has its `run_ended`. Cost: a sink is called from a
  destructor, on the thread that drops the runtime.
- B: nothing is written. Cost: a consumer cannot tell a run that is still running from one whose
  adapter went away; it must time out on its own.

## 12. Inferred, not observed

Nothing was built or run for this document. Beyond that, these claims rest on knowledge outside
this tree:

- That `memory.events` `oom_kill` and `pids.events` `max` only grow over the life of a cgroup.
- That an OOM kill in a cgroup with `memory.oom.group` 1 ends every process in it. The host
  driver's test `an_exec_that_crosses_its_memory_bound_leaves_no_process_in_its_cgroup` holds it
  for `linux`; it was read, not run.
- What a container runtime's `OOMKilled` flag covers (the main process or any process of the
  container), and whether the runtime sets a group kill. D7 does not depend on the answer.
- That the kubelet writes `status.reason` `DeadlineExceeded` on a Pod it ended for
  `activeDeadlineSeconds`, and how long after the deadline.
- That a container runtime and the kubelet report a main process killed by signal 9 as exit code
  137. `oci.rs` maps 129 to 192 to a signal, and, beside `OOMKilled`, any other status outside 0
  to 128 to signal 9; the `pod` rule of D7 takes a code above 128.
- That a task dropped by a runtime shutdown runs the destructors of what it owns on the thread
  that drops the runtime, and that a started `spawn_blocking` closure runs to its end.
- That the digest in the header of a generated projection covers comments of the YAML (D25).
- That the Kubernetes API exposes no counter of refused process creations for a Pod. What was
  read: the adapter reads none.

## 13. Not decided here

- A run entity in the ESS model, with the ended observation as a command's input and the
  per-adapter rows as its outcomes. It would let the suite hold D7, D8 and D14. It is its own
  story.
- The usage figures (wall time, CPU time, peak memory, peak process count):
  `story:run-observation-carries-usage`. It extends `EndOfRun`, `RunObservation` and `RunEnded`.
- The process count on the wire: `story:exec-observation-reports-process-count-hits`.
- A count on `oci` and `pod`. Reading the container's cgroup from the host is not designed.
- A cumulative CPU budget on `oci` and `pod`. Both bound a rate today.
- The CPU rate on `linux` (D7, the note on `linux`): a profile cannot ask for a CPU budget
  without the rate that spreads it over the timeout. A bound for the rate of its own is a
  capability change with its own story.
- Whether `pod`'s `memory_oom_kills` of 0 or 1 should become `None`, as a flag is not a count.
  D8 leaves it as released.
- A record of a denied execution or connection, a record at the moment of a hit, a wire event:
  the epic's boundaries.
- Limits summed over several runs, `rate_bounds`, a block I/O bound: the epic's boundaries.
- A bound on how long the sink may hold a thread of the blocking pool.
- A constructor or builder for `AuditRecord::RunEnded` for a consumer that tests its own sink.

## 14. Not read

- The consumers outside this repository that build `RunObservation` or match `AuditRecord`.
- `crates/substrate-host/src/confinement/stdin.rs` beyond the fact that it holds a test
  implementation of the trait; `render` and the admission code of each adapter beyond what § 1
  cites.
- `pod.rs`: `pod_end`, `stopped`, `capture`, the policy removal, and the unit cases beside the
  monitor. Whether an existing fake serves the give-up case of § 8 is not verified.
- `oci.rs`: the network arguments, the runtime probe and `OCI_STDIN_PINNED`.
- `macos.rs` before the run's start and its Seatbelt rendering.
- `scripts/delegated-lane.sh` and `scripts/pod-lane.sh` beyond the lines that set and check the
  lane variables; how each lane's required `executed` lines are listed is taken from `AGENTS.md`.
- `.config/nextest.toml` and the test groups; taken from `AGENTS.md`.
- How the daemon and the Kubernetes driver build and store `ExecObservation`; only the count of
  struct expressions was read.
- `spec/confinement/scenarios/`, `spec/confinement-suite.json`, `spec/confinement/system.yaml`
  and the generated suite types; `website/docs` and what `cargo xtask check-docs` holds of them.
- ADR 0109 (atlas), ADR 0014, ADR 0031 and ADR 0037; they are cited as the code and the
  specification cite them.

Checked for this document: every relative link target above exists (`ls` on each path). No
build, test or `cargo xtask check-links` was run.
