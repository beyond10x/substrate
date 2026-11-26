# Design 36: limit hits on the confinement adapters

Status: accepted on 2026-11-24. The first consumer's session answered the five questions of § 11,
each with the choice this document is written with
(`approval-record:first-consumer-answers-the-questions-of-design-36-2026-11-24`). Written
against `main` at `c407457cc`. Serves three stories of
`epic:runs-are-governed-and-limit-hits-are-events`:

1. `story:run-observation-names-the-limit-that-ended-it` (§ 2),
2. `story:process-count-limit-hit-is-observed` (§ 3),
3. `story:audit-records-run-end-and-limit` (§ 4).

Amended on 2026-11-25, against `main` at `fe73a2c14`, on the first consumer's answer to one more
question (`approval-record:requester-answers-d101-2026-11-25`): D27 and D28, the `oci` note of
D7, and the measurement § 12 holds. The amendment serves a fourth story of that epic,
`story:an-oci-run-killed-at-its-memory-bound-names-the-limit-on-a-hosted-runner`.

Amended on 2026-11-26, against `main` at `34b81f6a25`: § 3a, the usage behind the limits,
D29 to D38 and Q6 to Q8, for `story:run-observation-carries-usage`, which § 13 named. The
consumer gave no answer that day; each choice is the default the wave took
(`approval-record:wave-2026-11-26-substrate`) and can be changed here before the change that
builds it merges.

It changes no wire byte and no contract bundle: the observation and the audit record are
substrate-host's confinement API
([ADR 0044](../../adr/0044-confinement-is-a-contract-family-served-by-declared-adapters.md)
decisions 7 and 8). § 10 lists the decisions, § 11 the questions that have more than one
defensible answer (five of 2026-11-24, three of 2026-11-26), § 12 every claim that is inferred and not read in this tree, § 14 what was not
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
  child's OOM kill names `Memory`; one that exits with 0 to 128 names none. The monitor reads
  the runtime's state once, after the runtime client returned and before it removes the
  container (D28, amendment of 2026-11-25; from 2026-11-24 to that day it read the state again
  for one second where the first read held no flag). A memory-bound kill whose state holds no
  flag at that read names no limit; the observation then says that the kill is not attributed
  (D27). So does a flag beside an embedder's `signal(Kill)` that was asked when the container
  had ended and so sent nothing: the monitor knows that a kill was asked, not which kill ended
  the container, and D27 states nothing for a kill the embedder asked for. Where the runtime
  client's
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

### A kill no source explains

Amendment of 2026-11-25. On a hosted runner the runtime's state held no `OOMKilled` for 128 of
610 runs that the kernel killed at their memory bound, and no later read held it (§ 12). D7
forbids naming `Memory` from the signal. Before this amendment such a run's observation was
that of any other `SIGKILL`: `exit.signal` 9 and nothing else.

**D27. `RunObservation` gains `limit_unread: Option<LimitUnread>`. `oci` sets it for a run whose
main process ended by `SIGKILL` that no source the adapter reads explains. `limit` stays
`None`.**

```rust
/// Why an ended run names no limit although a bound may have ended it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum LimitUnread {
    /// The run's main process ended by `SIGKILL`. The adapter sent none, the embedder asked for
    /// none, and the source the adapter reads for a memory-bound kill names none.
    UnattributedKill,
}
```

When `oci` sets `Some(UnattributedKill)`. All of these hold at the monitor's one read of the
runtime's state:

- the container has ended and its exit status is 137, which the adapter reads as signal 9;
- the state holds no `OOMKilled`;
- the monitor's own deadline did not fire, and the stall of the live output sent no kill;
- the embedder asked for no `signal(Kill)`;
- the run is published as `Ended`, so the runtime client returned and the container was removed.

When the member is `None`, on every adapter:

- beside a named limit: `limit` and `limit_unread` are never both `Some`;
- while the run runs, and on an `Unknown` run, which carries no exit;
- for every exit that is not signal 9. A bound's kill is `SIGKILL`; an end by another signal
  (status 129 to 192 other than 137) is read as before and states nothing;
- for a kill the adapter or the embedder sent. The deadline's kill names `WallTime`, the stall's
  `OutputStall`, and an embedder's `signal(Kill)` names nothing and is the embedder's own
  knowledge.

What the member says. The run was killed, and the adapter read no source that says by whom. It
is no limit: a consumer that counts limit hits counts `limit`, and reads this member as "a
bound may have ended this run, and this adapter cannot say". Four causes produce it on `oci`:

1. the kernel's OOM killer at the run's memory bound, where the runtime did not record its event
   (the measured case);
2. the kernel's OOM killer under memory pressure of the whole host;
3. a kill another party sent through the runtime;
4. a process of the run that sent `SIGKILL` to the run's command. A run can therefore produce
   the member by itself. It cannot remove it: the monitor decides from the runtime's state and
   its own flags, none of which the run writes.

Per adapter.

| adapter | `limit_unread` |
|---|---|
| `linux`, `systemd` | `None`: the memory row of D7 is the cgroup's own `oom_kill` count, read before the cgroup is removed, so a `SIGKILL` beside a count of zero is known not to be the memory bound's. Where that count could not be read, `memory_oom_kills` is `None` and says so |
| `oci` | `Some(UnattributedKill)` under the five conditions above, else `None` |
| `pod` | `None`: unchanged. Whether a container status can lack `OOMKilled` after a memory-bound kill was not measured (§ 13) |
| `local`, `macos` | `None`: no `limits` family, so no bound of the adapter sends a kill |

Reason for a member of its own, and not a sixth variant of `RunLimit`. `limit` answers "which
bound ended the run". A variant that means "perhaps one" would make every consumer's match on
`limit` decide whether it counts as a hit, and D7's rule would no longer hold for the field. The
enum is `#[non_exhaustive]`: a second reason a later adapter cannot read gets a variant.

Reason for stating it at all, and not leaving `None` beside `None`. The requirement is that
every limit hit is visible. Where the adapter cannot see one, the observation says that it
cannot, on the run where that is the case, and a consumer can tell that run from one a signal
of its own ended.

**D28. The `oci` monitor reads the runtime's state once. The re-read of one second is
removed.** `OOM_FLAG_WAIT` and the function that waits for the flag go.

Reason. In the measurement no flag arrived after the first read: 482 of 610 states held it at
the first read, and of the 128 others none held it within 5 s. The re-read named no limit in any
measured run and published the end of every unattributed kill one second later, the end of a
command that kills itself included.

What an exact answer on `oci` would take is not decided here (§ 13).

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

What the kernel counts in it depends on the kernel. Where the cgroup has no `pids.events.local`,
it counts every creation refused for a process of this cgroup, also one a bound of a cgroup above
it refused. Where it has that file, and the cgroup2 mount has no `pids_localevents` option, it
counts the hits of this cgroup's bound and of the bound of any cgroup beneath it. For `systemd`
this means that on a kernel without `pids.events.local`, runs whose run cgroups share a bound
above them each count a creation that shared bound refused for one of their processes.

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
- A running `observe` made after the end's read and before the kill of the tree is complete can
  read a higher count than the ended observation carries, because a process left in the tree can
  still be refused. The ended observation's count is the one read once the leader exited.

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

## 3a. The usage behind the limits

Added 2026-11-26 for `story:run-observation-carries-usage`. Numbered 3a so that every reference
to §§ 4 to 14 holds.

What the tree has at `34b81f6a25`. The wire's `ResourceUsage` (`substrate-wire`, `lib.rs`) holds
`wall_time_us`, `cpu_time_us`, `memory_peak_bytes` and `processes_peak`, among others. The host
driver fills it only for an exec that asked for `measurements` (`Cgroup::resource_usage` in
[`process.rs`](../../crates/substrate-host/src/process.rs)), and a measured exec whose counter
cannot be read is refused `exec.metrics-unavailable`. `linux` asks for no measurements. `systemd`
around `local` reads `cpu.stat` for its CPU budget and `memory.events`, `pids.events` at the end,
and no `memory.peak`, `pids.peak` or wall time. No other adapter reads a counter of the run.

**D29. `RunObservation` gains `usage: Option<RunUsage>`, one `#[non_exhaustive]` struct with
four `Option<u64>` members.**

```rust
/// What an ended run used (design 36 § 3a).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize)]
#[non_exhaustive]
pub struct RunUsage {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wall_time_us: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu_time_us: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_peak_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub processes_peak: Option<u64>,
}
```

- `usage` is `None` while the run runs, on every adapter that reads no figure (D36), and on
  D22's record. It is `Some` on an ended or `Unknown` run of an adapter that reads them.
- A consumer outside this crate builds one with `RunUsage::default()` and field assignments.

Reason. One member groups the four figures, so a consumer passes them on as one value, and a
later figure (block I/O, `story:block-io-and-bandwidth-limits`) is a member of `RunUsage` that
breaks no struct expression and no pattern (Q6).

**D30. A figure that cannot be read leaves that member absent, and only that one.** The other
members keep what was read. No member is ever zero for a figure that was not read (invariant 3).

Reason. A kernel without `pids.peak` still has `memory.peak` and `cpu.stat`; losing all four for
one missing file would hide what was read.

**D31. The members are named and counted as the wire's `ResourceUsage` names and counts them.**

| member | unit | source on `linux` and `systemd` |
|---|---|---|
| `wall_time_us` | microseconds | the adapter's own clock (D32) |
| `cpu_time_us` | microseconds | `cpu.stat` `usage_usec` of the run's cgroup |
| `memory_peak_bytes` | bytes | `memory.peak` of the run's cgroup |
| `processes_peak` | processes | `pids.peak` of the run's cgroup |

Reason. A daemon client and an embedder read one vocabulary, and no figure is converted.

**D32. Wall time runs from the run's start to its leader's exit, on every adapter that reports
it.** On `linux` it is the driver's clock from `started_at`, the instant before the driver
releases the start barrier, to the reap of the leader, whether the leader exited by itself or by
a bound's kill. On `systemd` around `local` it is the adapter's clock from the spawn of the leader
to the end task's wait for its exit. The time the adapter then spends draining output and ending
the tree is not counted.

Reason. The wall-time bound is held against the leader; what follows its exit is the adapter's
own work and differs per adapter.

**D33. An exec that asked for `measurements` reuses its terminal `ResourceUsage` sample for
`cpu_time_us`, `memory_peak_bytes` and `processes_peak` instead of reading again.** Where that
sample failed, the end reads each counter on its own, as `Cgroup::end_of_run` does for the OOM
and process counts (D9). `wall_time_us` is D32's for every exec, measured or not: a measured
exec's `RunUsage.wall_time_us` can be lower than its wire `usage.wall_time_us`, by the time between
the leader's exit and the terminal sample, which the drains take. The wire value does not move.

Reason. One counter is read once at the end, so the observation and the wire record of one
measured exec never carry two values of it (Q7).

**D34. Reading the usage refuses nothing, names no limit and changes no state.** An unreadable
figure is no `exec.metrics-unavailable`, does not make the exec `Unknown` and does not reach the
refusal. `linux` keeps asking the driver for no measurements (§ 1), and the wire's `resource.usage`
of an exec that asked for none stays absent.

Reason. An embedder that asked for nothing must not be refused for a counter it did not ask for
(the story's item 2).

**D35. A run that ends `Unknown` carries what was read before its cgroup went; D22's record
carries no usage.** On `linux` the driver reads the usage with the end's other counters, before
`reconcile_cgroup`, whatever state it then records. On `systemd` around `local` the end task
reads it before `drain_tree`, and an `Unknown` phase keeps it. A record of D22 is written when
the adapter lost sight of the run and read nothing.

Reason. D2 and D18: what the adapter observed is a fact also where the end is not known.

**D36. One row per adapter.**

| adapter | `wall_time_us` | `cpu_time_us` | `memory_peak_bytes` | `processes_peak` | when it is read |
|---|---|---|---|---|---|
| `linux` | present | present where `cpu.stat` reads | present where `memory.peak` reads | present where `pids.peak` reads | by `Cgroup::end_of_run`, once the leader exited and both drains ended, before `reconcile_cgroup` kills the tree and removes the cgroup; a measured exec from its terminal sample (D33) |
| `systemd` around `linux` | `linux`'s | `linux`'s | `linux`'s | `linux`'s | relabelled |
| `systemd` around `local` | present | present where `cpu.stat` reads | present where `memory.peak` reads | present where `pids.peak` reads | by the run's end task with `oom_kill`: after the tree's kill and before `drain_tree` removes the cgroup, so the CPU time includes what the tree's processes used until their kill |
| `oci` | absent | absent | absent | absent | `usage` is `None`: the state the monitor reads holds `StartedAt`, read as a flag, and no end time and no counter; the runtime's stats are not read |
| `pod` | absent | absent | absent | absent | `usage` is `None`: the container status it reads holds `reason`, `exitCode` and `signal`; `startedAt` and `finishedAt` are not read, and the Pod status holds no counter |
| `local` | absent | absent | absent | absent | `usage` is `None`: no `limits` family and no cgroup (the story's item 4) |
| `macos` | absent | absent | absent | absent | `usage` is `None`, as `local` |

On `linux` the CPU time is read once the leader exited and both drains ended, before the kill:
what a process left in the tree uses between that read and its kill is not counted, as D14 says
of the count. The two peaks are high-water marks and lose nothing to that order.

**D37. The `run_ended` record carries the same `usage`, omitted when absent.**
`AuditRecord::RunEnded` gains `usage: Option<RunUsage>` with `skip_serializing_if`; within it
an absent member is omitted. D15's rule holds: the record is built from the ended observation,
so it carries what `observe` would say. A run on `linux` that its timeout ended:

```json
{"event":"run_ended","run":"…","profile":"…","adapter":"linux","state":"ended",
 "exit":{"signal":9},"limit":"wall_time","memory_oom_kills":0,"process_limit_hits":0,
 "usage":{"wall_time_us":1003512,"cpu_time_us":2210,"memory_peak_bytes":1208320,
 "processes_peak":3}}
```

**D38. The host driver's `EndOfRun` (D9) gains `usage: RunUsage`.** It is `Some` on the
observation exactly where `end_of_run` is, so it is absent while the exec runs, on an observation
rebuilt from a stored record and on one another driver made. Each member is `None` where its
figure could not be read. `EndOfRun` is `#[non_exhaustive]`, so the member breaks no consumer
outside this crate; no public item of the confinement module names `EndOfRun`.

**The specification.** D25 governs: the `UNMAPPED:` block of `spec/confinement/confinement.yaml`
names `usage` and its four members beside the other members of the ended observation, and
`spec/confinement/README.md` says so under *Open questions*. No type, command, event or scenario
is added. The story's item 5 asks for "the field and a scenario"; it is read under D25, as story
2's sentence was: the model has no run entity, and no scenario could produce a figure.

**Compatibility.** `RunObservation` and the variant `RunEnded` are `#[non_exhaustive]` since D26,
so the new field breaks no Rust consumer at compile time. A sink that serializes every record and
refuses unknown members refuses a `run_ended` record with `usage`. `confinement-public-items.txt`
gains `RunUsage`, its members and the two fields.

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
    /// D27, amendment of 2026-11-25.
    #[serde(skip_serializing_if = "Option::is_none")]
    limit_unread: Option<LimitUnread>,
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
- `story:an-oci-run-killed-at-its-memory-bound-names-the-limit-on-a-hosted-runner` adds
  `limit_unread` to the observation and the record together (D27). A reader takes a missing
  `limit_unread` as "nothing stated".

A run on `linux` that its memory bound ended:

```json
{"event":"run_ended","run":"…","profile":"…","adapter":"linux","state":"ended",
 "exit":{"signal":9},"limit":"memory","memory_oom_kills":1,"process_limit_hits":0}
```

A run on `oci` that a kill ended which the adapter cannot attribute (D27):

```json
{"event":"run_ended","run":"…","profile":"…","adapter":"oci","state":"ended",
 "exit":{"signal":9},"limit_unread":"unattributed_kill"}
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
where the profile has no sink (D19). `pod` spawns its monitor before `audit_start` returned, and
the monitor may publish the run's end before `start` hands it the value: there `start` and the
monitor meet in one cell instead of a channel. The cell holds the value where `start` came first
and the ended observation where the publication came first, and whichever side comes second
writes the record; no task waits for the other side. A cell dropped with the value in it writes
`unknown` through the value's drop (D22); one dropped with an observation in it, as after a
start that failed once it spawned the monitor, writes nothing. Amended 2026-11-24 with the change
that built the record.

**D18. The record is written for a run that ends `Unknown` as well.** The end task writes it on
every path on which it publishes the run's end, `Ended` or `Unknown`. For `Unknown` the record
has no exit, and the limit and the counters where the adapter observed them (D2).

**D19. A profile without a sink writes nothing, and costs nothing.** `audit` already writes only
where `profile.audit` is set. Without a sink `audit_start` returns an inert value, and `linux`
spawns no task for it.

### Which task calls the sink

**D20. The run's end task writes the record, after it published the run's end, and the sink runs
on a thread of its own, one per record.** Amended 2026-11-24 with the change that built the
record, in place of the runtime's blocking pool: `local`, `macos` and `systemd` around `local`
detect each run's exit on that pool, whose size the consumer's runtime sets, and the write of a
record shares no bounded resource with any run's exit detection or `wait`.

| adapter | the task |
|---|---|
| `local`, `macos`, `systemd` around `local`, `oci` | the task that publishes the run's phase today. `macos` writes `private_tmpdir_left` first, as today |
| `pod` | the monitor that publishes the run's phase, or `start` where it hands the value of D16 after that publication (D17's cell); a task of its own awaits the write (amended 2026-11-24) |
| `linux` | a new task per run that has a sink: it awaits `ProcessRuntime::wait_ended`, observes the exec once and writes. It is the shape of the task that ends a piped standard input today |
| `systemd` around `linux` | a task of the outer adapter: it awaits the inner run's end and writes with its own admitted profile and the relabelled observation. The inner adapter has no sink (§ 1) |

- The run ends whether or not anybody calls `observe` or `wait`; the tasks above do not depend
  on a caller.
- The end is published first. `wait` on the adapters with a phase channel returns from that
  channel; `wait` on `linux` polls the driver. Neither waits for the record.
- The sink is called on a thread started for the one record, named `substrate-run-ended`
  (`std::thread::Builder`). A sink that blocks or panics then holds that thread only: no worker
  of the runtime and no thread of its blocking pool, so it delays neither `wait`, nor another
  run, nor another record, whatever the size of the pool, on a current-thread runtime and with no
  runtime at all. The thread enters the runtime that was current when the record was handed to
  it, so a sink that reaches the runtime on `run_started` reaches it on `run_ended`; a sink that
  spawns onto that runtime once it shut down gets Tokio's behaviour, a task dropped unrun, and
  the record is still written. A sink that panics is contained where the consumer is built with
  `panic = "unwind"`; with `panic = "abort"` it ends the process, as on every other record.
- The end task does not end at the hand-off. It waits on a channel until the record's write is
  over, so it holds no thread while it waits, and it ends once the sink returned. Where the
  runtime drops the task before that (`drop(runtime)`), the task's guard joins the record's
  thread on the dropping thread: the runtime's drop returns once `run_ended` is in the sink, so
  every `run_started` in a process that returns from its runtime has its `run_ended` (Q5). A
  guard dropped by a thread that is panicking does not join: the record's thread is already
  running and still writes while the process lives, and a panic is never held by a sink. A sink
  that never returns holds the runtime's drop, as a started blocking-pool job does. On `pod`,
  whose monitor goes on to remove the Pod and its policy after it published the end, the side of
  D17's cell that comes second, the publication or `start`, hands the ended record to its thread
  before it returns and awaits the write on a task of its own; a runtime that drops that task
  joins the thread like any other. So a sink that blocks does not hold the Pod's or its policy's
  removal, with one exception: where the system refused the record's thread and the runtime's
  blocking pool is shutting down, the record is written on the side's own task (below). Amended
  2026-11-24 with the change that built the record.
- A sink is a synchronous call. It may hand work to the runtime it finds current (a spawn, a
  channel send) and must not wait on that runtime (`Handle::block_on`, a blocking wait for a
  task, a timer or I/O of it). Once nothing drives that runtime, nothing drives such a wait: on
  a current-thread runtime that is once its `Runtime::block_on` returned, before its drop
  begins, and on every runtime once its drop began. The sink then does not return, the guard's
  join does not return, and the runtime's drop does not return. This is the case above of a sink
  that never returns, reached by a sink that waits on the runtime that waits for it;
  `limit_a_sink_that_waits_on_its_runtime_holds_that_runtimes_drop` holds it on a current-thread
  runtime with a 200 ms timer: 1 s after the drop began it has not returned and the record is
  not in the sink. Its counterpart,
  `a_sink_whose_wait_its_runtime_drives_returns_and_the_drop_returns`, holds that the same sink
  returns, and the drop returns with the record in the sink, where `Runtime::block_on` still
  drives the timer while the sink waits. The other records are called on the caller's
  thread, inside the runtime where the caller is async. Tokio 1.53.1 (`runtime/handle.rs`, the
  documentation of `Handle::block_on`, lines 248 to 253 and 263 to 269) says that on a
  current-thread runtime only `Runtime::block_on` drives the I/O and timer drivers, so timers and
  I/O do not work under `Handle::block_on` unless another thread is in `Runtime::block_on`, and
  that `Handle::block_on` panics when it is called from an asynchronous context. Amended
  2026-11-24 with the change that built the record.
- `Runtime::shutdown_timeout` and `Runtime::shutdown_background` bound that wait only where the
  task is dropped on a worker of a multi-thread runtime: those workers are threads of the
  blocking pool (Tokio 1.53, `runtime/scheduler/multi_thread/worker.rs`, `spawn_blocking(move ||
  run(worker))`), and both calls stop waiting for that pool after their duration and leak what
  still runs (`runtime/runtime.rs`, § *Shutdown*: "spawned work that did not stop in time and
  threads running it are leaked"). The join then goes on there and the record is written while
  the process lives. A current-thread runtime drops its tasks on the thread that shuts it down,
  in `Runtime`'s `Drop` after either call, so there the join is not bounded by the duration.
- Where the system refuses the thread, the record is not dropped. It goes to the current
  runtime's blocking pool, and only where no runtime is current, or that pool is shutting down
  and drops the job unrun, is it written on the calling thread before the hand-off returns. A
  pool that cannot start a thread keeps the job in its queue and panics; the panic is
  contained, and a thread of the pool writes the record once one is free, or the runtime's
  shutdown drops the job and writes it on that thread (Tokio 1.53.1,
  `runtime/blocking/pool.rs`, `spawn_task`, lines 393 to 438, and `spawn_blocking`'s
  `NoThreads` panic, lines 324 to 326). On every path it is written once and a sink that
  panics is contained. A warning names the refused thread. On that path a sink that blocks holds
  a thread of the pool, or the calling thread, for as long as it blocks.
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
the run. Nothing is written from a thread that is panicking, and a sink that panics on this
record is contained (amended 2026-11-24 with the change that built the record). A run that ended
before the task was dropped is D20's: its record is written on its own thread, which the dropped
task joins; on `pod`, whichever side of D17's cell comes second hands the ended record to that
thread before it returns, in either order, so a runtime that drops the waiting task right after
cannot turn it into `unknown`. A process that exits or is killed writes nothing; a consumer that must account for that pairs `run_started` with `run_ended` in
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
  `run_ended` record, are `UNMAPPED`; the change that adds `limit_unread` (D27) names it in the
  same block and regenerates the projections in the same change;
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
`story:exec-observation-reports-process-count-hits`. The usage of § 3a moves no wire byte either:
`RunUsage` is a host type, and the wire's `ResourceUsage` keeps being published only for an exec
that asked for measurements (D34).

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

The amendment of 2026-11-25 (D27, D28) breaks no Rust consumer at compile time: `RunObservation`
and the variant `RunEnded` are `#[non_exhaustive]` since D26, so the new field `limit_unread`
compiles against every pattern with `..` and every `RunObservation::new`, and `LimitUnread` is a
new public item. What a consumer sees at run time:

| consumer | after the amendment |
|---|---|
| reads `limit` of an `oci` run | unchanged in what a value means; a memory-bound kill whose state holds no flag is `None`, as before |
| reads the new `limit_unread` | `Some(UnattributedKill)` on the `oci` runs of D27, `None` on every other observation |
| a sink that serializes every record | a `run_ended` record of such a run has one more member, `"limit_unread"`; a reader that refuses unknown members refuses that record |
| waits for the end of an `oci` run that a kill of unread origin ended | the end is published up to one second earlier (D28) |

The public-items list gains `LimitUnread`, its variant and the field, and the change's
`CHANGELOG.md` entry names the new record member.

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

### The amendment of 2026-11-25, the unattributed kill

| lane | case | expects |
|---|---|---|
| gate's test step | unit, in `confinement/oci.rs`: the function that decides the member, over a state with exit status 137 and no flag, where neither the monitor nor the embedder sent a kill | `limit` `None`, `limit_unread` `UnattributedKill`. This is the state the hosted runner showed; the case does not compile before the change |
| gate's test step | the same function over: the flag set; the deadline fired; the stall's kill; an embedder's `signal(Kill)`; exit status 143; exit status 1; no state read | `limit_unread` `None` in each, with the limit D7 names |
| gate's test step | unit, in `confinement/`: the record's JSON with the member, and without | `"limit_unread":"unattributed_kill"`, and the member omitted |
| gate's test step | `ess_confinement_local`: a run that exits 0 | `limit_unread` `None` on the observation and on `run_ended` |
| `oci` | `ess_confinement_oci`: a command that sends `SIGKILL` to itself, with a memory bound it stays under | `exit.signal` 9, `limit` `None`, `limit_unread` `UnattributedKill`, and one `run_ended` with the same member |
| `oci` | the same file, the existing memory case: a run that allocates past its memory bound | either `limit` `Memory` with `limit_unread` `None`, or `limit` `None` with `limit_unread` `UnattributedKill`; the case prints which of the two it read. Any other observation fails it |
| `oci` | the same file, the existing timeout case | `WallTime`, `limit_unread` `None` |
| delegated | `ess_confinement_linux`, the existing memory case | `Memory`, `limit_unread` `None` |

The self-kill case was probed once on a workstation on 2026-11-25 with the runtime's own client
and the lane's pinned image (Docker 29.8.2, `--init`): `sh -c 'kill -9 $$'` ended with exit
status 137 and no `OOMKilled`, and so did a child that killed its parent; `kill -9 1` from inside
ended nothing.

The memory case's two answers are one deterministic assertion, not a retry: a run of it passes
or fails on its one observation. How often each answer comes is a rate, and a rate is no test
([`AGENTS.md`](../../AGENTS.md), § *The gate*). It is measured once after the change, as the
first measurement was: one job on a standard hosted runner, from a branch that is never merged,
which runs the memory case's shape through the changed adapter and counts the runs that end
with a named limit, with the stated member, and with neither. The story's acceptance is a count
of zero for the last. The figures go to the planning store.

### Story 4, the usage (§ 3a)

The lane cases' run sleeps one second in eight processes started at once, allocates 64 MiB in
one buffer (`dd` with a block of 64 MiB, which the read of `/dev/zero` fills) and spins in a
subshell whose CPU time is bounded at one second by `ulimit -t 1`, so it spends at least one
second of CPU whatever the load of the host. Its bounds leave room for all of it: ten seconds of
wall and CPU time, 256 MiB, 32 processes.

| lane | case | expects |
|---|---|---|
| gate's test step | unit, in `process.rs`: the end-of-run read from a cgroup directory written as files | each member `Some` for a readable file, `None` for a missing one, the others kept; a measured end takes its terminal sample's three counters |
| gate's test step | unit, in `confinement/systemd.rs`: the end task's read from files | the same |
| gate's test step | unit, in `confinement/linux.rs`: the observation of an ended exec and of a running one | `usage` is `end_of_run`'s on the ended one, `None` on the running one |
| gate's test step | unit, in `confinement/`: the record's JSON with `usage`, with a member absent, and without | `"usage":{…}` with the absent member omitted, and the member omitted |
| gate's test step | `ess_confinement_local`: a run that exits 0 | `usage` `None` on the observation and on `run_ended` |
| delegated | `ess_confinement_linux`: the run above, with a sink | wall time at least 1 s, CPU time at least 1 s, peak memory at least 64 MiB, peak processes at least 8, on the observation and the same `usage` on `run_ended`. This is the story's acceptance |
| delegated | the same file: a run that sleeps past its timeout of one second, with a sink | `WallTime` and wall time at least 1 s, on the observation and on `run_ended` |
| `systemd` | `ess_confinement_systemd`, around `local`: the same two cases | the same |
| `systemd` | the same file, around `linux`: the first case | the same, relabelled `systemd` with `inner: linux` |
| `oci`, `pod`, `macos` | the existing cases that assert `process_limit_hits` absent | `usage` `None` on the observation and on `run_ended` |

Every change also regenerates
[`confinement-public-items.txt`](../../crates/substrate-host/tests/confinement-public-items.txt).

## 9. Per adapter, in one table

| adapter | `limit` can be | `memory_oom_kills` | `process_limit_hits` | `run_ended` written by |
|---|---|---|---|---|
| `linux` | all five | read | read | a task of its own per run with a sink |
| `systemd` around `linux` | all five | read | read | a task of the outer adapter |
| `systemd` around `local` | `Memory`, `CpuTime`, `WallTime`, `OutputStall` | read | read | the run's end task |
| `oci` | `Memory`, `WallTime`, `OutputStall` | `None` | `None` | its monitor |
| `pod` | `Memory`, `WallTime` | `Some(0)` or `Some(1)` | `None` | its monitor, or `start` where it hands the value after the publication (D17) |
| `local` | `OutputStall` | `None` | `None` | the run's end task |
| `macos` | `OutputStall` | `None` | `None` | the run's end task |

`limit_unread` (D27) is `Some(UnattributedKill)` on `oci` alone, and `None` on every other
adapter.

| adapter | `usage` (§ 3a) |
|---|---|
| `linux` | `Some`, the four members read by the driver before the cgroup is removed (D36) |
| `systemd` around `linux` | `linux`'s |
| `systemd` around `local` | `Some`, the four members read by the run's end task (D36) |
| `oci`, `pod`, `local`, `macos` | `None` |

## 10. Decisions

| # | decision |
|---|---|
| D1 | `RunObservation.limit: Option<RunLimit>`; five variants; `None` for running, for no limit and for not observed |
| D2 | the limit is named on an `Unknown` run where the adapter observed it |
| D3 | the process bound and the output byte bound are no variants |
| D4 | of several true limits the first of output stall, egress bytes, memory, CPU time, wall time is named, on every adapter |
| D5 | the aperture byte ceiling and the output stall are variants (Q1) |
| D6 | `linux` maps five refusal codes; every other code is `None` |
| D7 | per adapter: which limits, from which source; none from an exit or a signal alone (the `oci` note amended 2026-11-25) |
| D8 | `memory_oom_kills` on `linux` and `systemd`; `oci`, `local`, `macos` `None`; `pod` unchanged |
| D9 | `ExecObservation.end_of_run`, host-internal, read for every exec before the cgroup is removed; an unreadable counter is `None` |
| D10 | the process count is `pids.events` `max`; the peak is the usage story's |
| D11 | `RunObservation.process_limit_hits: Option<u64>` |
| D12 | a hit never ends a run and never sets `limit` |
| D13 | a running observation carries the count where it is read (Q3) |
| D14 | `linux` and `systemd` report it; `oci`, `pod`, `local`, `macos` `None` |
| D15 | `AuditRecord::RunEnded`, `"event": "run_ended"`: the ended observation plus the profile id |
| D16 | `audit_start` returns the one value that writes `run_ended` |
| D17 | `run_ended` follows `run_started`; the value reaches the end task after `audit_start` returned; on `pod` the value and the publication meet in one cell, and the side that comes second writes (amended 2026-11-24) |
| D18 | written for `Unknown` as well |
| D19 | no sink: nothing written, no task added |
| D20 | the end task writes after it published the end; the sink runs on a thread of its own per record, which the end task awaits and a dropped end task joins (Q4, Q5; amended 2026-11-24) |
| D21 | dropping an adapter neither ends a run nor loses its record |
| D22 | a runtime that ends first: `run_ended` with `state: unknown` (Q5) |
| D23 | a declared-insecure run's record carries its reason |
| D24 | no record for a denied execution or connection, none while the run runs |
| D25 | the specification gains one `UNMAPPED:` block and nothing else |
| D26 | `RunObservation` and `AuditRecord` become `#[non_exhaustive]`; `RunObservation::new` (Q2) |
| D27 | `RunObservation.limit_unread: Option<LimitUnread>`, on the record too; `oci` sets `UnattributedKill` for a `SIGKILL` neither it nor the embedder sent and the runtime's state does not explain; `limit` stays `None` (2026-11-25) |
| D28 | the `oci` monitor reads the runtime's state once; the one-second re-read is removed (2026-11-25) |
| D29 | `RunObservation.usage: Option<RunUsage>`, one `#[non_exhaustive]` struct of four `Option<u64>` members, so a later figure breaks no caller (Q6; 2026-11-26) |
| D30 | a figure that cannot be read leaves that member absent and keeps the others (invariant 3) |
| D31 | `wall_time_us`, `cpu_time_us`, `memory_peak_bytes`, `processes_peak`, named and counted as the wire's `ResourceUsage` |
| D32 | wall time runs from the start to the leader's exit, on every adapter that reports it |
| D33 | a measured exec reuses its terminal sample for the three counters; wall time is D32's for every exec (Q7) |
| D34 | reading the usage refuses nothing, names no limit and changes no state; `linux` still asks for no measurements |
| D35 | an `Unknown` run carries what was read before its cgroup went; D22's record carries none |
| D36 | per adapter: `linux` and `systemd` read the four; `oci`, `pod`, `local`, `macos` `None` (Q8) |
| D37 | `run_ended` carries the same `usage`, omitted when absent |
| D38 | `EndOfRun.usage: RunUsage`, host-internal, read with the end's other counters |

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

- A (chosen): after the end is published, on a thread of its own (D20, amended 2026-11-24; it
  named the blocking pool). `wait` never waits for a sink.
  Cost: the record may arrive after `wait` returned; a consumer that reads its sink right after
  `wait` must wait for the record.
- B: before the end is published, on the end task. After `wait` the record is in the sink. Cost:
  a sink that blocks holds `wait` and every `observe` of that run, and on a current-thread
  runtime every other task. The story rules this out.

**Q5. A runtime that shuts down while a run still runs.**

- A (chosen): the dropped value writes `run_ended` with `state: unknown`. Every `run_started` in
  a process that returns from its runtime has its `run_ended`. Cost: a sink is called from a
  destructor, on the thread that drops the runtime, and that drop waits for a `run_ended` still
  being written on its own thread (D20, amended 2026-11-24), so a sink that never returns holds
  it.
- B: nothing is written. Cost: a consumer cannot tell a run that is still running from one whose
  adapter went away; it must time out on its own.

Q6 to Q8 were added on 2026-11-26 with § 3a. The consumer gave no answer that day; A is the
default the wave took (`approval-record:wave-2026-11-26-substrate`).

**Q6. The shape of the usage.**

- A (chosen): one `#[non_exhaustive]` `RunUsage` with four `Option<u64>` members, as
  `RunObservation.usage` (D29). Cost: a consumer reads `observation.usage?.cpu_time_us`, one
  level deeper than the counts beside it.
- B: four `Option<u64>` fields on `RunObservation` and `RunEnded`, beside `memory_oom_kills`.
  Cost: every later figure is one more field of each; a consumer that passes the usage on copies
  each field.

**Q7. The usage of an exec that asked for measurements.**

- A (chosen): the end reuses the terminal `ResourceUsage` sample for the three counters (D33).
  Cost: its wall time is the driver's clock to the leader's exit and can be lower than the wire's
  `usage.wall_time_us` of the same exec.
- B: the end reads every exec's counters on its own, measured or not. Cost: the same counter is
  read twice at the end of a measured exec, and the observation and the wire record of one exec
  can carry two values of it.

**Q8. The adapters that read no counter today.**

- A (chosen): `oci`, `pod`, `local` and `macos` leave `usage` `None` (D36; the story's item 4).
  Cost: no usage on those adapters, not even a wall time their own clocks could give.
- B: `local` and `macos` report a wall time from their own clocks and `oci` and `pod` read their
  runtime's figures. Cost: four more reads to design and prove, two of them through a runtime
  or a cluster API this design has not read (§ 13).

## 12. Inferred, not observed

Nothing was built or run for this document. Beyond that, these claims rest on knowledge outside
this tree:

- That `memory.events` `oom_kill` and `pids.events` `max` only grow over the life of a cgroup.
- What `pids.events` `max` counts (D10). Read: the kernel's `cgroup-v2.rst`, which says that
  without `pids_localevents` the counter "represents any pids.max enforcemnt across cgroup's
  subtree" and that the option restores the v1-like count of local fork failures, and a host
  whose cgroups have `pids.events.local` and whose cgroup2 mount has no `pids_localevents`. Read
  too: `kernel/cgroup/pids.c` of Linux 6.12. Its `pids_event` counts `max` in the cgroup whose
  bound was exceeded and in every cgroup above it, so a cgroup counts no creation that a bound
  above it refused; under `CGRP_ROOT_PIDS_LOCAL_EVENTS` it counts instead in the cgroup of the
  process refused, whichever bound refused it, and `__pids_events_show` prints `max %lld\n`. The
  count on a kernel without `pids.events.local` is taken to be that second form, which the
  document calls v1-like. The source of such a kernel was not read, and neither was which
  kernel release added the file.
- That an OOM kill in a cgroup with `memory.oom.group` 1 ends every process in it. The host
  driver's test `an_exec_that_crosses_its_memory_bound_leaves_no_process_in_its_cgroup` holds it
  for `linux`; it was read, not run.
- What a container runtime's `OOMKilled` flag covers (the main process or any process of the
  container), and whether the runtime sets a group kill. D7 does not depend on the answer.
- When a container runtime sets `OOMKilled`, relative to the exit, on a runtime other than the
  two measured below. Read in the sources of Docker and of containerd's cgroup library on
  2026-11-24, not run: Docker sets the flag when the OOM event arrives, whatever the container's
  state, and the event's watcher sends none when the cgroup is removed before it reads
  `memory.events`. So a late flag and a missing one are both possible by the sources. Measured
  (next entry): a missing one, and no late one. D28 rests on that measurement; a runtime that
  sets the flag late would name no limit and state D27's member.
- The measurement D27 and D28 rest on, and what it does not cover. One job on a standard hosted
  runner on 2026-11-25 (run 45659093178: runner image ubuntu-26.04, kernel 7.0.0-1020-azure,
  4 CPUs, Docker 29.4.2 with the systemd cgroup driver on cgroup v2, runc 1.5.3) ran the `oci`
  lane's memory case's shape 610 times with the runtime's own client: the arguments the adapter
  passes to `create`, then `start --attach`, then reads of the container's state for 5 s. The
  kernel log of the job holds 610 OOM kill lines.

  | phase | runs | flag at the first read | flag at a later read | no flag |
  |---|---|---|---|---|
  | one at a time, no load | 60 | 56 | 0 | 4 |
  | one at a time, beside 8 busy loops | 150 | 113 | 0 | 37 |
  | four at a time, beside 8 busy loops | 200 | 146 | 0 | 54 |
  | as the second, with a reader on the host of the container's `memory.events` | 100 | 81 | 0 | 19 |
  | as the second, the growing process a child of the container's command | 100 | 86 | 0 | 14 |
  | all | 610 | 482 | 0 | 128 |

  Three more sources were read in the same job. The runtime's `oom` event: sent for 482
  containers, the same 482 that held the flag, so it is no second source. The reader on the host:
  it read `oom_kill` above zero in 66 of its 100 runs, and the flag or the reader named the kill
  in 90 of them, so it is not exact either. A command inside the container that outlives the
  kill of its child: it read `oom_kill` 1 in its own cgroup in 100 of 100 runs. On a workstation
  (Docker 29.8.2, the cgroupfs driver on cgroup v2) 330 of 330 containers of the case's shape
  held the flag at the first read on 2026-11-24, and so did 19 of 19 on 2026-11-25. Not
  covered: another runner image, another runtime (Podman was not run), a rootless daemon, and a
  rate over more than one job. The three failures of the gate's case that led here are
  recorded in the story.
- That a command which sends `SIGKILL` to itself ends an `oci` run with exit status 137 and no
  flag on a hosted runner. Probed once on a workstation (§ 8); the `oci` lane's case holds it.
- That the kubelet writes `status.reason` `DeadlineExceeded` on a Pod it ended for
  `activeDeadlineSeconds`, and how long after the deadline.
- That a container runtime and the kubelet report a main process killed by signal 9 as exit code
  137. `oci.rs` maps 129 to 192 to a signal, and, beside `OOMKilled`, any other status outside 0
  to 128 to signal 9; the `pod` rule of D7 takes a code above 128.
- That a task dropped by a runtime shutdown runs the destructors of what it owns on the thread
  that drops the runtime for a current-thread runtime and on a worker for a multi-thread one
  (read in Tokio 1.53, `Runtime`'s `Drop`; not run on the multi-thread flavour). What holds by
  the guard of D20: the runtime's drop returns after the record's thread wrote; a thread whose
  guard was dropped by a panicking thread, or that `shutdown_timeout` or `shutdown_background`
  left behind, writes only while the process lives; the runtime's drop returns only once the sink
  returned, so a sink that waits on that runtime holds it (measured on a current-thread runtime
  by the `limit_` case of D20; read, not run, for a multi-thread runtime's drop).
- That the digest in the header of a generated projection covers comments of the YAML (D25).
- That the Kubernetes API exposes no counter of refused process creations for a Pod. What was
  read: the adapter reads none.
- For § 3a, from the kernel's `cgroup-v2.rst` and not from its source: that `memory.peak` is the
  highest `memory.current` of the cgroup since it was created, that `pids.peak` is the highest
  `pids.current`, and that `cpu.stat` `usage_usec` counts the CPU time of every process that ran
  in the cgroup, ended ones included. Which kernel release added `pids.peak` was not read; a
  kernel without it leaves `processes_peak` absent (D30).
- That `ulimit -t 1` in the lane cases' shell sets `RLIMIT_CPU` and the kernel sends `SIGXCPU`
  once the process's CPU time reaches one second, so the spinning subshell spends at least one
  second of CPU and then ends.

## 13. Not decided here

- An exact source for a memory-bound kill on `oci`. The one the measurement found exact is a
  process inside the container that outlives the kill and reads its own cgroup's `oom_kill`
  (§ 12). It would be a helper the adapter starts inside every run, which is a change of this
  repository's architecture and needs a design document of its own. It is drafted as
  `story:an-oci-run-reads-its-memory-kill-from-inside-the-container` and not ordered: no
  consumer gap needs it today (`approval-record:requester-answers-d101-2026-11-25`).
- The same question for `pod`: whether a container status can lack `OOMKilled` after a
  memory-bound kill was not measured, and `pod` states no `limit_unread`.
- A run entity in the ESS model, with the ended observation as a command's input and the
  per-adapter rows as its outcomes. It would let the suite hold D7, D8 and D14. It is its own
  story.
- The usage figures (wall time, CPU time, peak memory, peak process count) are decided in § 3a
  for `story:run-observation-carries-usage`, on `linux` and `systemd`. Not decided: usage on
  `oci` (the runtime's stats, or its state's start and end times), on `pod` (the kubelet's
  figures, or the container status's `startedAt` and `finishedAt`), and on `local` and `macos`
  (their own clocks, `wait4`'s resource usage); usage while a run runs; block I/O and network
  figures (`story:block-io-and-bandwidth-limits`).
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
- A bound on how long the sink may hold its thread, on how many such threads a sink that never
  returns may leave behind, and on how long such a sink may hold a runtime's drop (D20). A sink
  that waits on the runtime it finds current is such a sink once nothing drives that runtime;
  the design states it as a rule for the sink and holds it as a limit, and detects nothing.
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
