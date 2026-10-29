---
title: Release notes
description: What changed in the newest Substrate release, generated from CHANGELOG.md.
---

# What changed in the newest release

This page ends with the newest released section of `CHANGELOG.md`, and `cargo xtask check-docs`
fails when the two differ. Earlier releases are in the
[full changelog](https://github.com/beyond10x/substrate/blob/main/CHANGELOG.md). The
[confinement adapters](./concepts/adapters.md) page explains each adapter the notes name.

<!-- Generated from CHANGELOG.md by `cargo xtask docs-releases`. Edit CHANGELOG.md, not the text below. -->

## 0.15.0 (2026-10-29)

### Added in 0.15.0

- Host processes publish their lifecycle and stream it (ADR 0036 S12, contract bundle `0.24.0`).
  Each transition (`process.started`, `process.lease-renewed`, `process.lease-expired`,
  `process.timed-out`, `process.stopped`, `process.exited`, `process.vanished`) is an event of the
  existing event log with `resource_kind: process` and the operation that caused it. The new
  WebSocket `GET /v1/processes/{process_id}/events`, on the same socket and posture as the other
  process routes, sends one record's events and its log lines in one order, each item with a
  resumable cursor, a `gap` item for log bytes the bound discarded before the reader reached them,
  and one `ended` item after the record's end, after which it closes; a stream of an ended record
  replays what the event log and the log still hold and closes; a stream opened without a cursor
  is never refused for what retention removed. No line is sent before `process.started` while the
  record is starting. Lines are read from the log file, never from the program's pipes, so a
  slow reader never blocks the program, and they never enter the event log; an idle stream reads
  the store at most once a second. A ninth concurrent stream of one record, a subject's 513th over
  all its records, live or ended, or a deployment's 513th is `process.events-readers-exceeded`
  (429, retriable). The cursor names its record; one of another record, or outside the published
  pattern, is `event.cursor-invalid`, and every cursor the daemon cannot honour is refused before
  the upgrade with the event log's codes. The Rust SDK gains `Client::process_events` and
  `ProcessEventStream`.
- An operational listener for probes and scraping (ADR 0045). It adds no wire route and leaves the
  contract bundle unchanged.
  - **Flags:** `--ops-listen <address:port>` (`SUBSTRATE_OPS_LISTEN`) turns it on; it is off by
    default. Startup refuses a zero port, an address a control listener uses (in any spelling,
    IPv4-mapped included) or an `--ops-require-fact` name that is not a capability fact, with
    `ops.listener-config-invalid`.
  - **Transport:** plaintext and unauthenticated. It never reads an `Authorization` header or a
    body and never upgrades. It allows at most 8 connections, at most 4 from one source address, a
    2-second request-head deadline and one exchange per connection. It answers `GET` on `/livez`, `/readyz` and `/metrics`; anything
    else gets `404` with an empty body.
  - **`/livez`:** `200 ok` while both maintenance tasks run.
  - **`/readyz`:** `200`, or `503` otherwise, with one `ok` or `failed` line each for four checks:
    - `store`: one read-only statement answers within 1 s;
    - `schema`: the database's schema version matches the binary's. The store now stamps it at
      open as SQLite `user_version` 1. A database stamped higher, by a newer binary, is refused
      at open and keeps its stamp, so the daemon does not start on it;
    - `maintenance`: each task completed a round within 5 s;
    - `driver`: the startup snapshot publishes every `--ops-require-fact <fact>`
      (`SUBSTRATE_OPS_REQUIRE_FACT`, repeatable).

    It never calls Identity and answers `503` once shutdown begins, before any listener closes.
  - **`/metrics`:** a fixed OpenMetrics catalogue. Labels are closed sets only, and a scrape holds
    at most 579 samples, 471 without the refusal counter. Every histogram has 11 buckets from
    5 ms to 10 s. The metrics:
    - `substrate_build_info{version,contract_bundle}`, `substrate_ready{check}`;
    - `substrate_http_requests_total{listener,family,outcome}`,
      `substrate_http_request_duration_seconds{listener,family}`;
    - `substrate_refusals_total{code}`, one count per failure answer; a code the wire crate does
      not name, or a failure answer with no code (a `405`, the development listener's `401`), is
      counted as `other`;
    - `substrate_dispatch_total{family,outcome}`, `substrate_dispatch_duration_seconds{family}`;
    - `substrate_connections_open{listener}`, `substrate_connections_limit{listener}`,
      `substrate_connections_rejected_total{listener}`;
    - `substrate_output_stalls_total`;
    - `substrate_maintenance_rounds_total{task}`,
      `substrate_maintenance_round_duration_seconds{task}`,
      `substrate_maintenance_last_round_timestamp_seconds{task}`;
    - `substrate_hosted_admissions_total{result}`,
      `substrate_hosted_resolution_duration_seconds`;
    - `process_cpu_seconds_total`, `process_resident_memory_bytes`, `process_open_fds`,
      `process_max_fds`, `process_start_time_seconds`.
- A Helm chart for the node-bound Kubernetes serving profile (ADR 0046), `deploy/helm/substrate`,
  named `b10x-substrate`. It adds no wire route and leaves the contract bundle unchanged.
  - **Shape:** a StatefulSet behind one headless Service, one daemon per node of a labelled and
    tainted pool, and one retained `ReadWriteOnce` ledger claim per ordinal. `--deployment` is the
    pod name. The probes use the operational listener.
  - **Trust:** TLS and Identity admission only. Init containers copy the certificate, key and
    Identity trust roots into memory-backed volumes, owner-only. The certificate names each pod's
    host and never the Service; the chart publishes those names in `<release>-certificate-names`.
    The ServiceAccount mounts no token and has no Role.
  - **Network:** a default-deny NetworkPolicy. Egress goes only to DNS and to the declared Identity,
    Git source and aperture ranges.
  - **Refusals:** a closed `values.schema.json` and template checks refuse a privileged container,
    host namespaces and host paths, the development TCP listener, a cgroup root, host roots,
    project quotas without execution or a quota-capable class, pod annotations that select an
    AppArmor or seccomp profile, and egress to any prefix length 0 unless admitted. Every value
    the templates interpolate is quoted, so a value rendered with the schema skipped cannot add a
    key.
  - **Execution:** `execution.enabled` starts design 25's bootstrap with its five capabilities and
    the node's Localhost seccomp and AppArmor profiles; `nodeProfiles.enabled` installs those with
    a DaemonSet in a separate namespace.
  - **Lane:** `cargo xtask chart-lane static` renders and lints the chart and holds every rendering
    to ADR 0046. `cargo xtask chart-lane cluster` runs install, rolling upgrade, egress, Pod
    Security and uninstall scenarios on kind, and reports the four execution-enabled scenarios it
    cannot run there absent, by name. `.github/workflows/chart-lane.yml` runs both on every push
    to `main` and on pull requests that touch the chart or what the daemon image is built from.
  - **Release:** `release.yml` refuses a tag without a successful chart-lane run for its commit,
    counting only a push to `main` or a dispatch in this repository, and refuses an existing chart
    tag before it publishes anything. It packages the chart reproducibly with
    `cargo xtask package-chart`, pinning the daemon digest it just pushed, before the registry
    login, pushes it once to `oci://ghcr.io/beyond10x/charts/b10x-substrate`, signs it keylessly,
    attests its build provenance, verifies both and reads it back anonymously before announcing.
    A recovery reuses an existing chart only when its chart layer is the local package byte for
    byte. The release notes list the scenarios the chart lane reported absent.
  - The gate preflight now also counts only a push to `main` or a dispatch in this repository.
- A confinement adapter run can read a piped standard input (design 32). A profile asks for it
  with `StdinMode::Piped { limit_bytes }`, 1 to `MAX_STDIN_BYTES` (16 MiB, the daemon session's
  input bound), and requires the new fact `stdin_pipe`. The embedder writes with
  `ConfinementAdapter::write_stdin`, 1 to `STDIN_CHUNK_BYTES` (64 KiB) a call, one write in flight
  a run, and ends input with `close_stdin`; `run_with_stdin` writes a whole buffer, closes, waits
  and returns the output with the bytes the pipe took, and kills its run if it is dropped first.
  A cancelled write closes the pipe, so the child reads a prefix and then end of file, and from
  the run's observed end a write is `session.input-closed` while a pending one fails
  `session.write-failed`. The default, `StdinMode::Null`, is `/dev/null` as before. The trait's two
  new methods answer `session.not-pipe` by default, so an implementor outside the crate compiles
  unchanged. `local`, `linux`, `macos` and `systemd` around either prove `stdin_pipe`; `oci`
  proves it only on a runtime its lane pinned (Docker 29.7.2), and `pod` refuses `Piped` naming
  the fact.
- A confinement adapter run can deliver its output live while it runs (design 32). A profile asks
  for it with `OutputDelivery::Live { queued_chunks }`, 1 to `MAX_QUEUED_CHUNKS` (16), and
  requires the new fact `output_live`; the default, `OutputDelivery::Recorded`, is the recorded
  output as before. `ConfinementAdapter::read_output` returns the next `OutputEvent`: a `Chunk`
  with its stream and its offset in that stream's record, `Bounded` for a stream that reached its
  output bound, `Stalled` once, then `None` once both streams have ended, which a child that closes them
  while it keeps running reaches before the run ends. Live output needs `Head`
  retention on both streams: anything else is `confinement.live-output-retention-invalid` (422),
  and a queue outside 1 to 16 is `session.limit-unserved` (429), both at admission. When the queue
  stays full for one second the run is killed and `Stalled` follows the queued chunks;
  `output_stalled` reports it. A zero read timeout is `session.timeout-invalid`, a read that
  waited its timeout `session.read-timeout`, and a run whose output is recorded only
  `session.not-pipe`, the trait's default, so an implementor outside the crate compiles
  unchanged. `run_with_stdin` refuses a live profile `confinement.live-output-unread` before it
  starts anything. `local`, `macos`, `oci` and `systemd` around `local` prove `output_live`;
  `linux` and `systemd` around it prove it with the enforced isolation floor; `pod` refuses
  `Live` naming the fact.
- The host execution AppArmor profile name is a deployment parameter (design 33). The chart
  value `execution.appArmorProfile` (default `substrate-host-exec-v1`) and `--apparmor-profile`
  on `substrate-container-profiles` and `substrate-container-exec` accept `substrate-host-exec-v1`
  or `substrate-host-exec-v1-<suffix>`. The installer renders the unchanged policy under that
  name, renaming its declaration and its two self-peers, installs and loads it from
  `<name>.apparmor`, and refuses any source that is not those exact bytes. The bootstrap
  requires `<name> (enforce)`. The schema and the templates refuse any other name. With the
  value unset, the policy, the installer, the bootstrap check and `helm template` output are
  byte-identical to before.

### Security in 0.15.0

- `pulldown-cmark` is 0.13.5, taking the fix for
  [RUSTSEC-2026-0414](https://rustsec.org/advisories/RUSTSEC-2026-0414.html); it is used only by
  `xtask`. `THIRD_PARTY_LICENSES.html` is regenerated for that revision.

### Changed in 0.15.0

- The specification and CI use ESS 0.72.0 (was 0.70.0); the suites, wire types and CLI package regenerate unchanged apart from the generator header.
- The specification and CI use ESS 0.74.0 (was 0.72.0). The host-process suite gains the synthesized `AppendStreamEvent/unknown-process` scenario (92 synthesized, 27 refusals); the other suites, wire types and CLI package regenerate unchanged apart from the generator header.
- The specification and CI use ESS 0.75.0 (was 0.74.0); the suites, wire types and CLI package regenerate unchanged apart from the generator header.
- A `session.not-attachable` refusal now names which condition failed (state not `ready`, attachment not `available`, lease not `active`) and states the three observed values, for example "The pty session is not attachable: its state is not ready, its attachment is not available. Observed state exited, attachment consumed, lease active." Code, status, class and `retriable` are unchanged; only the free-text `error.message` differs, and no bundle changes. Under the hosted posture the attachment-authority mint, where a hosted client meets this refusal first, says the same.
- **Breaking (`substrate-host` public API, design 32):** the confinement `Fact` enum gains `StdinPipe`, so an exhaustive `match` on it breaks and a consumer on an older revision cannot deserialize an `AdapterFacts` or an audit record that names `stdin_pipe`.
- **Breaking (`substrate-host` public API, design 32):** `FACT_ORDER` changes type from `[Fact; 11]` to `[Fact; 12]`; code that names the array type or relies on its length breaks.
- **Breaking (`substrate-host` public API, design 32):** `ConfinementProfile` gains `stdin`, so a struct literal without `..ConfinementProfile::default()` and an exhaustive destructuring pattern break.
- **Breaking (`substrate-host` public API, design 32):** the confinement `Fact` enum gains `OutputLive`, so an exhaustive `match` on it breaks and a consumer on an older revision cannot deserialize an `AdapterFacts` or an audit record that names `output_live`.
- **Breaking (`substrate-host` public API, design 32):** `FACT_ORDER` changes type from `[Fact; 12]` to `[Fact; 13]`; code that names the array type or relies on its length breaks.
- **Breaking (`substrate-host` public API, design 32):** `ConfinementProfile` gains `output_delivery`, so a struct literal without `..ConfinementProfile::default()` and an exhaustive destructuring pattern break.

### Fixed in 0.15.0

- A raw-pipe or pty session whose child exited after the driver answered the start and before the daemon recorded it no longer reads `ready` with attachment `available` over the dead child. The start keeps the recorded exit: the session reads `exited` with attachment `consumed`, its exec reads `exited`, and the start's result is that session.
- The lease sweeper no longer expires a lease issued while it runs: a just-started raw-pipe or pty session or exec is no longer killed, and a just-leased workspace no longer frozen. The lease sweeper judged each lease by a clock it had read before its store calls; a start that issued its lease from a later reading in between looked like a clock regression, which expires conservatively, so the sweeper killed the child and a freshly started session refused its first attach with `session.not-attachable` (state `expired`, attachment `consumed`, lease `expired`). The claim now reads the clock inside its own transaction. Seen once in 1,500 sequential pty start-and-attach rounds on a loaded host.
- Two concurrent renewals of one live session or exec lease no longer refuse the slower one `lease.expired` (409). A renewal judged the stored lease by a clock it had read before its store calls, so a renewal that committed a later issue in between made that reading look like a clock regression. Session and exec renewals now read the clock inside their own transaction and issue the renewed lease from that reading; workspace renewals read it under the workspace lock and were not affected.
- `pod`: a run read `Unknown` because its Pod's removal was not confirmed no longer gives up after two removal rounds; its task deletes and reads the Pod again, the pause doubling from 500 ms to 30 s, until the API confirms it gone, then removes its NetworkPolicy (design 28). Until then the run stays counted against the 128 tracked runs, so at that cap a start is refused `exec.tracking-capacity` rather than adding another removal task.
- `pod`: open removes a NetworkPolicy carrying the adapter's owner label, named for the run that label names, whose run's Pod no longer exists and which the API server created at least 90 s earlier (twice the 30 s API timeout plus 30 s), the policy a process that ended before its run's cleanup left behind. It reads the list in pages of 100, at most 64, each policy's metadata alone (`PartialObjectMetadataList`); a longer list, or a sweep the API refuses, does not open (`confinement.adapter-unserved`).
- `pod`: a policy open keeps, younger than 90 s or with its run's Pod present, no longer stays until the next open: the adapter checks it again every 2 s on a thread of its own, with one list of its owned Pods' metadata alone per round (a page of whole Pods of ten admitted runs, each up to 256 KiB of argv and 256 KiB of environment, would pass the 4 MiB page bound and fail every round), and deletes it once its Pod is gone and either the API showed that Pod before or the policy is 90 s old. The thread ends when nothing is kept or the adapter and its runs' tasks are gone.
- `pod`: after `Unknown` the task reads the Pod every 2 s between its backed-off deletions, so a policy follows its Pod within 2 s of the kubelet removing it, not at the next deletion up to 30 s later.
- `pod`: `start` reads the run's NetworkPolicy back once the Pod is stored, and the Pod carries an `ownerReference` to that policy (`controller: false`), so a Pod whose policy is gone is removed by the garbage collector; a policy not read back removes the Pod and refuses the start `confinement.pod-api-failed`.
- `pod`: a Pod that reads 404 no longer releases its run's NetworkPolicy by itself. A forced deletion, PodGC's after a node stopped answering or an operator's, removes a Pod while its container may still run. The run's task, the open sweep, the deferred sweep and an abandoned start now delete the policy only once the API shows the container terminated in a Pod whose `restartPolicy` reads `Never`, or the Pod deleted without ever being bound to a node: a read showed it, or a watch of the Pod from the last `resourceVersion` read, at first what the start's creates answered, replays it before the deletion, as the kubelet writes it before it removes a Pod. A run whose task is still asking counts against the tracked-run cap. A watch answered 410, one that shows nothing within 5 s, a deletion of a container not shown terminated, or no `resourceVersion` to watch from keeps the policy and reports the run `Unknown`; a `Failed` phase alone does not count, because PodGC sets it before a forced deletion. **Required RBAC change:** the adapter's Role needs `watch` on `pods` (now `create`, `get`, `list`, `watch`, `delete` on `pods` and `networkpolicies`, and `get` on `pods/log`); a deployment that pre-creates the Role must add it, or every run whose Pod goes before its container is read terminated keeps its policy and reads `Unknown`.
- The event log reads an `ev2` cursor by its published `event-cursor` pattern: one outside it
  (a scope without the `scope_` form, generation 0, more than 512 characters) is
  `event.cursor-invalid` (409), where it was `event.source-mismatch`. A cursor inside the pattern
  whose number does not fit 64 bits reads as the largest one: another generation is
  `event.source-mismatch`, a seq or offset beyond the stream `event.cursor-invalid`. A
  process-event cursor whose seq is 2^63 or more is `event.cursor-invalid`, where it was 500
  `state.store-failed`, and an empty process-event cursor is `event.cursor-invalid`, where it was
  422 `request.schema-invalid`.
