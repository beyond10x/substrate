---
title: Release notes
description: What changed in the newest Substrate release, generated from CHANGELOG.md.
---

# What changed in the newest release

This page ends with the newest released section of `CHANGELOG.md`, and `cargo xtask check-docs`
fails when the two differ. Earlier releases are in the
[full changelog](https://github.com/beyond10x/substrate/blob/main/CHANGELOG.md). The
[confinement adapters](./concepts/adapters.md) page explains each adapter the notes name.

The newest release is 0.27.0, published on 24 November 2026. The
[status page](./status.md#newest-release) summarizes it.

A release that moves the contract bundle needs its SDK clients upgraded with the daemon. See the
[deployment rules](./guides/deployment.md#rules-that-do-not-change-by-posture).

<!-- Generated from CHANGELOG.md by `cargo xtask docs-releases`. Edit CHANGELOG.md, not the text below. -->

## 0.27.0 (2026-11-24)

### Added in 0.27.0

- The wire crate states the bytes the operation ledger reserves for one success result,
  `OPERATION_RESULT_MAX_JSON_BYTES` (524,217), the bounds of the result members a request does
  not set (`MAX_TIMESTAMP_CHARS`, `MAX_RESOURCE_ID_CHARS`, `MAX_OPERATION_ID_CHARS`, the label,
  lease, applied `cgroup`, aperture, usage, refusal and exit-signal bounds), and a
  `widest_json_bytes` on `Exec`, `PipeSession`, `ProcessRecord`, `Workspace`, `FileObservation`,
  `FileAbsence`, `ExecAbsence`, `SessionAbsence` and `WorkspaceAbsence`: the bytes of compact
  JSON of the widest form that result is stored in (ADR 0053 decisions 4, 6 and 7). No answer of
  the daemon changes. Held by the wire crate's `result_width_tests` against the schemas of
  bundle `0.27.0`, and by `the_default_reserve_is_the_published_result_bound` in the store.
- Contract bundle `substrate-wire/0.28.0` (predecessor `0.27.0`, adds no route, preserves its
  45) publishes ADR 0053's result bound: the required fact `operation.result-max-json-bytes`
  (524,217) with the client's counting rule in its description, the refusal
  `operation.result-limit` (429, `exhausted`) on the inputs of the exec, session and
  host-process starts and the v1 file write and delete, the error `operation.result-unrecorded`
  (500, `failed`), the coverage requirement `security.bounds.result`, and published lengths on
  the five minted identifiers (32), an exec refusal's `code` (128) and the process record's
  `argv` items, `cwd` and `env_names` items. Six vectors publish them, each executed against
  the daemon: the five refusals on a machine that states the least reserve a store opens with
  (383 bytes), each run as written on a daemon whose ledger reserves that figure and whose
  `GET /v1/machine` answers it, each request's widest result 144 to 25,240 bytes over it,
  answered exactly, recorded refused and nothing dispatched or changed on the host; and the row that ended `operation.result-unrecorded`, which names its
  operation as every such row the daemon writes does, read exactly. And
  `cargo xtask check-bundle 0.28.0` holds each value to its wire constant
  (`xtask/src/bundle/result_bound.rs`). The daemon, the SDK and the conformance runner advertise
  `substrate-wire/0.28.0` (inner `bundle.json` sha256
  `2cf0b0bd5a96600f4fbc02d2f98c3c76d839e3926399535487bc41f29e219e78`). A client that
  validates a capability snapshot against a bundle's schema needs the schema of `0.28.0`.
- A start of an exec, a session or a host process, and a v1 file write or delete, whose widest
  success result exceeds what the operation ledger reserves for one is refused
  `operation.result-limit` (429, `exhausted`, not retriable, address `result`) before any
  driver is called: the row is recorded refused, no resource row and no lease is written, and
  the same operation id replays the refusal. A request another published code refuses keeps
  that code. Every capability snapshot states the reserve as `operation.result-max-json-bytes`,
  the figure the daemon decides against: `OPERATION_RESULT_MAX_JSON_BYTES` (524,217) for the
  ledger as shipped (ADR 0053 decisions 1 to 5 and 10). The daemon publishes the driver's
  snapshot unchanged, so its digest covers that fact. No application is constructed over a
  snapshot that states another figure than its ledger reserves: `App::new`,
  `App::with_authority` and `App::with_delegated_context` return `ResultBoundMismatch` naming
  both figures, and the daemon does not start with it (ADR 0053, amendment of 2026-11-24).
- `substrate_host::confinement::RunLimit` and `RunObservation.limit` (design 36, story 1): an
  ended or unknown run names the one limit the adapter imposed to end it, `memory`, `cpu_time`,
  `wall_time`, `egress_bytes` or `output_stall`, and a running run and a run that ended on its
  own or on the embedder's signal name none. Where several were reached, the first in the order
  output stall, egress bytes, memory, CPU time, wall time is named. No adapter derives a limit
  from an exit code or a signal alone: `linux` maps the driver's refusal code, `systemd` its own
  CPU and deadline watch and the run cgroup's OOM count, `oci` the container's OOM flag beside a
  signal (a status of 129 to 192 is read as one, and, beside the flag, any status outside 0 to
  128, 193 and above or negative, as `SIGKILL`), its deadline and its stall, `pod` the
  container's `OOMKilled` beside a signal or an exit code over 128 and its deadline, and `local`
  and `macos` their stall only. Held by unit cases of each adapter's mapping and by lane cases
  that end a run on each bound (`ESS confinement linux limit …`, `ESS confinement systemd …
  limit…`, which `scripts/delegated-lane.sh` requires by name, and the `oci` and `pod` lanes).
- A run ended by a bound is told from one that ended by itself:
  - `systemd` around `local` records a CPU or deadline limit, and kills, only while the run's
    leader has not exited and the embedder has not killed the run, so a run whose leader exited
    first names no limit, also when a bound passes while the adapter still drains the run's tree.
  - `output_stall` is named only where the stall ended the run: on `local`, `macos` and
    `systemd`, where it came before the leader's exit and before the embedder's `KILL`; on `oci`,
    where it came before the runtime client returned and its kill was asked before the
    embedder's `signal(Kill)`; on `linux`, where it came before the child
    exited on its own, whether or not the host driver had taken the exit yet. A program that
    exits on its own and whose unread output then stalls names no limit; `output_stalled` and
    `OutputEvent::Stalled` still report the stall, and the driver's record still carries
    `session.output-backpressure`. The driver marks such a stall in
    `EndOfRun::output_stall_after_exit`, and a `linux` exec so marked names the limit that ended
    it, egress bytes before memory, or none where it ended by itself: the aperture ceiling from
    the new `EndOfRun::egress_ceiling_reached`, memory from the OOM count, which the driver reads
    from the cgroup by itself, also where the run's other counters could not be read.
  - Held by unit cases on a cgroup of files, a held runtime thread, a stand-in pid and a child
    left unreaped, and by `a_local_run_that_exits_0_before_its_unread_output_stalls_names_no_limit`.
- `RunObservation.memory_oom_kills` is reported by `linux` and `systemd`: the run cgroup's OOM
  kill count, read before the cgroup is removed, and absent when it cannot be read. The host
  driver's `ExecObservation` carries it in the new `end_of_run` member (`EndOfRun`), read for
  every exec whether or not it asked for measurements.
- `AuditRecord::RunEnded`, written as `"event": "run_ended"` (design 36, story 3): every run an
  admitted profile with an audit sink starts puts exactly one record of its end into that sink,
  after its `run_started`, whether or not anyone observes or waits on the run, and also when the
  adapter was dropped first. It names the run, the profile, the adapter and its inner adapter,
  the reason a `local` profile is declared insecure, the state (`ended`, or `unknown` where the
  end could not be established), the exit, the limit that ended the run and the run cgroup's OOM
  kill count; an absent member is omitted. The record is written once the run's end is published,
  on a thread of its own named `substrate-run-ended`, so a sink may receive it after `wait`
  returned, and a sink that blocks or panics on it delays neither `wait`, nor another run, nor
  another record, whatever the size of the runtime's blocking pool, on a current-thread runtime
  and with no runtime; a sink that panics is contained where the consumer is built with
  `panic = "unwind"`. The thread enters the runtime current when the run's end was handed to it,
  so a sink that reaches the runtime on `run_started` reaches it on `run_ended`. The run's end
  task waits on a channel until the sink returned; a runtime that drops the task before that
  joins the record's thread, so once a runtime's drop returned, the `run_ended` of every run that
  ended under it is in the sink, and a sink that never returns holds that drop. A sink is a
  synchronous call: it may hand work to the runtime it finds current (a spawn, a channel send)
  and must not wait on that runtime (`Handle::block_on`, a blocking wait for a task, a timer or
  I/O of it), because once nothing drives that runtime (on a current-thread runtime once its
  `block_on` returned, or once its drop began) nothing drives that wait, the sink does not return
  and the drop does not return. On `pod`, `start` and the run's monitor meet in one cell: the
  side that comes second, the publication of the run's end or `start` handing over its value,
  hands the ended record to its thread before it returns, so in either order a runtime dropped
  right after cannot record that run `unknown`. Where the system refuses the thread, the record
  goes to the runtime's blocking pool, or, with no runtime or where that pool is shutting down,
  is written on the calling thread; on every path it is written once and a panicking sink is
  contained. An end task dropped before the run ended, such as at runtime shutdown, writes
  `state: unknown` on the dropping thread, and nothing from a thread that is panicking. A
  profile without a sink starts no task and no thread for it. `observe`, `wait` and every other
  call write no record. Held by thirteen unit cases of the record, its one-shot value, its
  thread and the end task's guard, five `pod` unit cases (the monitor's deadline, the value
  handed before and after the publication with a runtime dropped right after, a publication
  without a value, and a value whose monitor never published), fourteen cases of
  `ess_confinement_local` (among them a sink that blocks on a one-thread blocking pool, a sink
  that panics, a sink that reaches the runtime, a sink that spawns and sends while its runtime
  drops, a runtime dropped while the run runs, a runtime dropped after the run ended under a
  sink that takes 500 ms, the limit the observation names,
  `limit_a_sink_that_waits_on_its_runtime_holds_that_runtimes_drop`, which holds that a sink
  waiting 200 ms on its current-thread runtime's timer still holds that runtime's drop 1 s after
  it began, and its counterpart, where the runtime still drives that timer and the drop returns)
  and lane cases on `linux`, `systemd` around both inner adapters, `oci`,
  `pod` and `macos` (`ESS confinement … run ended`, the delegated ones required by name in
  `scripts/delegated-lane.sh`). The `oci` case runs in the gate's test step; its start and its
  record share what is left of 50 s after the adapter opened.
- `RunObservation.process_limit_hits` and the same member of `run_ended` (design 36, story 2):
  the number of process creations a process bound refused, the `max` counter of `pids.events` in
  the run's cgroup. Where the cgroup has no `pids.events.local`, the kernel counts there every
  creation refused for a process of the cgroup, also by the bound of a cgroup above it, so
  `systemd` runs whose run cgroups share a bound above them each count what that bound refused;
  where it has that file and the cgroup2 mount has no `pids_localevents`, it counts the hits of
  the cgroup's own bound and of any bound beneath it. `linux` and `systemd` report it: on an
  ended run the count read once the leader exited and before the cgroup is removed, and on
  `observe` of a running run the count at that moment, which between the end's read and the end
  of the tree's kill can be higher than the ended count; `0` says the counter was read and
  nothing was refused. A `pids.events` whose `max` line is not `max`, one space and a decimal
  number gives an absent count. `oci`, `pod`,
  `local` and `macos` leave it absent, and so does a read that failed, which never fails
  `observe`. A hit ends no run and names no limit: a run that hits its process bound and then its
  timeout names `wall_time` and a count above zero. The host driver reads the counter for every
  exec, measured or not, into the new `EndOfRun::process_limit_hits`. Held by unit cases of the
  one parser of `pids.events`, which the driver's read and the `systemd` read share (the same 23
  texts through each read), of the driver's read, of the record, of the `linux` observation that
  carries the driver's ended count and of a count that names no limit, by
  `ess_confinement_local` (absent on the observation and on the record), and by lane cases on
  `linux` and `systemd` around `local` (`ESS confinement … process limit hits …`), on `oci`,
  `pod` and `macos` (absent on the ended observation and on the record), and by
  `consumer_surface`'s run held to
  an allow-list and a deny-list that hits its process bound and then its timeout, which
  `scripts/delegated-lane.sh` requires by name.

### Changed in 0.27.0

- **Breaking (`b10x-substrate-daemon` public API, ADR 0053):** `App::new`,
  `App::with_authority` and `App::with_delegated_context` return
  `Result<Arc<App>, ResultBoundMismatch>` where they returned `Arc<App>`. The error is
  returned only when the driver's capability snapshot states another
  `operation.result-max-json-bytes` than the ledger reserves for a success result; it holds
  both figures (`published`, `reserved`). A caller takes the value with `?` or handles the
  error.
- **Breaking (`b10x-substrate-host` public API, design 36):** `RunObservation` is
  `#[non_exhaustive]` and has a constructor, `RunObservation::new(run, adapter, state)`, which
  leaves every optional member absent. A struct expression `RunObservation { … }` outside
  `b10x-substrate-host` no longer compiles: write `RunObservation::new(…)` and assign the fields
  it needs, which stay public. A pattern that names every field needs `..`; one that has `..` and
  every field read compile unchanged. `RunLimit` is `#[non_exhaustive]` too, so a `match` over it
  needs a wildcard arm. So is `AuditRecord`: a `match` over it outside `b10x-substrate-host` that
  names every variant needs a wildcard arm (`_ => …`); a `match` that has one, an `if let` or
  `matches!` on one variant, and building a variant compile unchanged. A struct expression of the
  host driver's `ExecObservation` names `end_of_run`.
- `RunState` and `RunExit` implement `Serialize`: a state as its snake-case name, an exit as an
  object whose absent `code` or `signal` is omitted.
- The host driver records an exec's end by these rules where two ends are ready at once: a child
  that exited before its deadline is never recorded timed out (the exit is taken first, and the
  `oci` adapter takes its runtime client's return first in the same way), and a run past both its
  CPU budget and its deadline is recorded `exec.cpu-limit`, not `exec.timeout`. Held by two cases
  of `crates/substrate-host/src/process.rs` that make both ends ready before the supervision is
  polled again.
- A completion whose result the operation ledger cannot record ends its row terminal with the
  error `operation.result-unrecorded` (500, `failed`, not retriable, address `result`): the row
  names the resource, the resource's own record is stored whole, nothing is cut, the run is not
  ended, and the same operation id replays the error. A store opens only with a terminal
  headroom of at least 454 bytes (`StoreConfig::minimum_terminal_headroom_bytes`: the longest
  `terminal_at`, the status charge, that error's widest outcome and the widest resource id), and
  accepts an operation only with an id of at most `MAX_OPERATION_ID_CHARS` characters of the
  wire's operation-id alphabet and a resource of at most `MAX_RESOURCE_ID_CHARS` (ADR 0053
  decision 8).

### Fixed in 0.27.0

- A signal to an exec or a session of the host driver whose run has ended, and whose cgroup the
  driver has removed, answers with the run's terminal observation and records no delivered
  signal and no cancellation for a step that reached no process. A cgroup that is there and takes
  no kill still fails the signal `exec.cgroup-write-failed`. Held by three unit cases of
  `crates/substrate-host/src/process.rs` on a paused clock.
- The release workflow reads the hosted lane records from every attempt of the
  `release-lanes.yml` run and takes each lane's record from the newest attempt in which a job
  that concluded `success` annotated it. Before, it read the latest attempt only, so a run
  repaired by re-running one failed lane job was refused for the lanes whose records are on the
  first attempt's check runs.
- `b10x-substrate workspace renew` and `b10x-substrate session signal`, run again with the same
  `--operation-id`, exit 0 and print the stored result, also once the workspace was destroyed or
  the session retired: the daemon replays the stored outcome of the operation. Both commands
  send the mutation by id through two new SDK calls, `Client::renew_workspace_lease` and
  `Client::signal_pipe_session`, as `workspace destroy` does since 0.25.0, and read nothing
  first. A first run that names a workspace or session that does not exist is a refused
  operation recorded under its operation id.
- `b10x-substrate exec run`, run again with the same `--operation-id`, prints the same output
  and exits with the same status, also once its workspace was destroyed. It starts by workspace
  id through the new SDK call `Client::command_in_workspace` and reads nothing first. The stored
  outcome of the start holds the exec's observation; the output is read from the exec's record,
  which outlives the workspace. Once that exec was retired, the retry is refused
  `resource.not-found`, prints no output and exits non-zero. The replay holds while the daemon
  publishes the capability snapshot the first run named, which the start's input carries: after
  a daemon restart the retry is refused `operation.request-conflict` under the same operation
  id, starts nothing, and `b10x-substrate operation show <id>` prints the stored outcome. A
  first run that names a workspace that does not exist is a refused operation recorded under
  its operation id; a `--workspace` word that is not one path segment is refused before
  anything is sent, as `workspace renew` and `session signal` refuse it.
- A confined exec or session start on the host driver holds whatever the processes its
  confinement backend forks do during the start, one that ends before the start returns
  included. The forked child joins the exec's cgroup itself before it executes the backend, so
  every process the backend forks is born inside it and the driver moves none. The backend's own
  start is charged to the run's memory and process bounds; joining and then executing the
  backend was measured at 262144 to 565248 bytes. A child that cannot join does not execute: a
  join the kernel refuses is `exec.spawn-failed`, and a `cgroup.procs` the driver cannot open is
  `exec.cgroup-write-failed`. The driver raises no `exec.process-tree-failed`. No route or
  schema changes.
- The work and the memory of a Git create follow its byte limit, the requested storage limit,
  else 2 GiB (design 21, § *The baseline store*). Five counts are held to a multiple of that
  limit: the bytes that reading the commit, each tree and each file object of the baseline
  whole inflates and makes, once for each place the baseline names the object (64 times the
  limit); the bytes the entries of a pack inflate to while the pack arrives (64 times the
  limit); the bytes of the table of entries the fetch keeps while a pack arrives, 128 for each
  entry, which is its item of 64 bytes and as much again for what the indexing keeps for it
  beside the table, and the lists beside them at what they reserve (the limit); the bytes of
  the objects
  the fetch holds until the entries stored behind them have been made, each at its own length
  (the limit); and the bytes the fetch inflates and makes when it computes the ids (64 times
  the limit). The fetch computes the ids on one thread, so the same pack under the same limit
  gets the same answer every time. A create over one of the counts is refused
  `workspace.git-storage-limit`, and the workspace is proved absent. The first count is held
  where the baseline is checked, before any file object is read. The other four are held in
  `gix-pack`, which the repository carries as `vendor/gix-pack-0.74.2`: the published crate
  with those four counts, selected by `[patch.crates-io]`. The copy also fills the queue of
  its id pass from the table and sorts the entries by id where they lie, so neither keeps a
  list beside the table; the index it writes of a pack is the same bytes. `cargo xtask check-packages` refuses
  a `[patch.crates-io]` entry that `Cargo.lock` does not resolve to, and a lock file that holds
  a second package of the entry's name from any source: the registry, a `sparse+` registry, a
  Git source or another path. Cases run against a source served
  on loopback, with a pack within every other count and over one, at the byte or the entry
  where the count turns, and state a bound on the memory of the test process. Over the indexing
  of a pack within every count that makes the limit wait, the peak resident memory grew by
  3.03 to 3.23 times the limit for a pack that deflates and, on the second indexing of a
  process, by 3.75 to 3.95 times for a pack that does not deflate, whose mapped file is itself
  nearly the limit; six cases hold that growth under four times. Over the first indexing of a
  process, which also pays about 2 MiB once for the pages of its program, the pack that does
  not deflate grew the peak by 3.98 to 4.18 times the limit; that figure is printed and not
  held. A create of
  1,000 files with 50 commits of history is served. No route, schema or refusal code changes.
- `cargo xtask check-packages` holds a rule for replacing a package: only `[patch.crates-io]`
  in the root `Cargo.toml` carries a patch, and the check refuses by name, with the file, the
  key or entry, and what to do: another key under `[patch]` (an index URL in any spelling, a
  registry name, a Git URL) and `[replace]` in `Cargo.toml`; an entry `Cargo.lock` lists under
  `[[patch.unused]]`; two entries that patch one package name; and an entry with neither a
  `path` nor a `git` source. Each entry of `[patch.crates-io]` is held to the lock file, a Git
  entry to its reference. A cargo configuration beneath the root holds only the top-level keys
  `alias` and `profile`, and an alias is a string or an array of strings with no `--config` or
  `--config=<value>` token; a `.cargo` directory holding both `config` and `config.toml`, a
  link included, is refused. The `.cargo` directory and both file names are taken without
  regard to ASCII letter case, as cargo reads them on a file system that compares names
  without it (macOS), and a directory holding more than one such file across the spellings is
  refused, naming every file. The rule holds in every directory the walk enters, a fixture that
  is a workspace of its own included: the macOS tests that build a fixture offline from its own
  `vendor` write the source replacement into the copy they run. Outside the check are
  configuration outside the repository (a file above the root, `CARGO_HOME`, `--config` on a
  command line or in a script or workflow, `CARGO_*` variables), files beneath a directory
  named `target` or `.git`, and what `profile` sets. Held by 28 cases of
  `xtask/src/packages.rs`: 27 on throwaway workspaces, and one that holds that this tree's
  fixture directory has no `.cargo`. On this tree the check reads the two entries and the one
  cargo configuration, `.cargo/config.toml`, and passes.
