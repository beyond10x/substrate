# Changelog

All notable changes to Substrate are documented in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project uses
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- The host execution AppArmor profile `substrate-host-exec-v2`
  (`deploy/apparmor/substrate-host-execution-v2`): `v1`'s rules with `noatime` and `nodiratime`
  added to the `remount options in (…) /**` set, no comment lines, and nothing else changed. Bubblewrap keeps a
  source mount's `noatime` when it remounts a bind read-only. `v1` refuses that remount, so on a
  node whose filesystems are mounted `noatime` the confinement probe failed and the daemon
  published no `exec.*` fact. A test checks every remount flag combination against both
  policies: `v2` refuses everything `v1` refuses except a remount that adds `noatime` or
  `nodiratime`.

### Changed

- `execution.appArmorProfile` in the chart, and `--apparmor-profile` on
  `substrate-container-profiles` and `substrate-container-exec`, default to
  `substrate-host-exec-v2`. They accept `substrate-host-exec-v1`, `substrate-host-exec-v2` and
  either followed by `-<suffix>`. `v1` stays byte for byte as released since `0.10.0` and is
  still installed and checked when named.
- **Upgrade: the default profile is now `v2`, and nodes must load it.** Every pool node needs
  `substrate-host-exec-v2` loaded before the daemon restarts under it: with
  `nodeProfiles.enabled` the installer DaemonSet loads it during `helm upgrade`; with it disabled,
  load it on each node by hand. The daemon image must carry the `v2` policy file
  (`/usr/share/substrate/apparmor/substrate-host-execution-v2`), as images from this release do.
  Setting `execution.appArmorProfile: substrate-host-exec-v1` keeps the old behaviour. The chart
  does not refuse the upgrade; its install notes say the same. The installer removes nothing, so
  a loaded `v1` stays loaded.
- `cargo xtask chart-lane cluster --node-profiles` names its profile
  `substrate-host-exec-v2-lane-<ulid>`.
- `cargo xtask chart-lane cluster` deletes its kind cluster only once `kind create` succeeded, so
  a create refused for a name another run holds deletes nothing. SIGINT or SIGTERM stops the
  running command and runs the same cleanup (the cluster's deletion and, with `--node-profiles`,
  the removal of the run's own profile) before it exits non-zero; a second signal exits at once.
  The run prints the exact removal commands when it creates each, and the refusal of a name
  already taken names `kind delete cluster --name <name>`.
- `cargo xtask chart-lane cluster --node-profiles` installs the chart and compares the seccomp
  profile as the recorded commit holds them, extracted with `git archive` like the chart-lab
  recipe, not from the working tree. A failed `helm uninstall` is a removal failure. A run that
  passed every scenario but is no record still exits 0 and ends its standard error with
  `not a release record`. It refuses a chart-lab recipe with a second `FROM` in any case, indented
  or tab-separated, and a lane source holding a carriage return, whose `SCENARIOS` the release
  workflow's awk would count differently.
- `helm template` and `helm lint` in both chart lanes run with an empty kubeconfig of their own
  and without the ambient `HELM_KUBE*` overrides.
- **Upgrade: the state store upgrade is one-way.** Opening a store adds the `backend_objects`
  table (design 34) and stamps it schema version 2. Substrate `0.15.x` and earlier refuse a
  store stamped 2 at open with `NewerSchema` and do not start on it. If you may roll back, back
  up the state directory before upgrading.

## [0.15.0] — 2026-10-29

### Added

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

### Security

- `pulldown-cmark` is 0.13.5, taking the fix for
  [RUSTSEC-2026-0414](https://rustsec.org/advisories/RUSTSEC-2026-0414.html); it is used only by
  `xtask`. `THIRD_PARTY_LICENSES.html` is regenerated for that revision.

### Changed

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

### Fixed

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

## [0.14.0] — 2026-10-27

Published by [release run 40558015370](https://github.com/beyond10x/substrate/actions/runs/40558015370)
for tag `0.14.0` at commit `0b9d50c6105cacc7f959eb41d5e65f252084a282`. All three images and the
binaries' `SHA256SUMS` were keyless-signed and verified against `release.yml@refs/tags/0.14.0` by
the release workflow, and each digest resolved anonymously:

- Image: `ghcr.io/beyond10x/b10x-substrate-daemon:0.14.0` at `sha256:a22fc8a1707ac750eb86115422e26ecfb2fde64fb3df588e9fced32ef2a52182`.
- Contract bundle: `ghcr.io/beyond10x/b10x-substrate-wire:0.23.0` at `sha256:c2a50222da154028011a794cc93724043943581ba4ee44051c38dd084b759ff5`, unchanged since 0.11.0 and signed again by this release.
- Disposable MCP image: `ghcr.io/beyond10x/b10x-substrate-mcp:0.14.0` at `sha256:c6844980db262412db962228bb263421e285d57226e7c8a3a28fbbb85d589d42`, development-only.
- Binaries: `substrate-daemon-0.14.0-x86_64-unknown-linux-gnu.tar.gz` and `b10x-substrate-0.14.0-x86_64-unknown-linux-gnu.tar.gz`, listed in `SHA256SUMS` (`sha256:521435c3007898ade2a021e13f799dd5789fddc44f81cfce051d186b6c67f480`) and signed in `SHA256SUMS.sigstore.json`.

### Added

- The confinement profile gains `workspace_access`, the wire's `WorkspaceAccess`, re-exported from
  `substrate_host::confinement`, default `ReadWrite` as before. `linux`, and `systemd` around it,
  serve `ReadOnly` and `Scoped` on a host whose probe proved `exec.workspace-scoped-write`: the
  workspace is bound read-only, and each named subtree is bound writable over it by descriptor, so a
  write elsewhere in the workspace fails inside the run and `Scoped` protects the workspace root's
  `.git` from writes in the run. A subtree that is `.git` or lies beneath one is refused
  `confinement.writable-subtree-git` (422), and so, at start, is a subtree that is or holds a
  submodule the workspace's `.gitmodules` registers, because the run could replace the submodule's
  `gitdir:` file and a host `git status` in the root would run Git in it; a malformed list is
  `exec.workspace-access-invalid` (422). Its limits: a nested `.git` inside a writable subtree stays
  writable when no `.gitmodules` registers it; `Scoped` does not protect host Git run inside a
  writable subtree from a repository the run creates there, as `ReadWrite` does not; and a hard link
  made on the host before the run stays writable through a subtree, while the run cannot create one
  itself (`EXDEV`). Every other adapter refuses a non-default access
  `exec.workspace-scoped-write-unserved` (501), and so does `linux` on a host without the fact.
  `sealed_paths` stays refused on `linux` (`confinement.fact-unserved`). The surface change is
  additive: a profile built with `..ConfinementProfile::default()` compiles unchanged, and only a
  struct literal that names every field must name the new one.
- The confinement profile gains `output_retention`, which sets `Head` (the default, unchanged),
  `Tail` or `HeadAndTail` for stdout and stderr separately, within the same byte bound per stream.
  `HeadAndTail` keeps the first half of the bound and the last half. `RunOutput` gains `stdout_kept`
  and `stderr_kept` (`KeptOutput`): the retention applied, the head and tail byte counts, and the
  bytes the stream produced when the adapter read it to its end. `local`, `linux`, `macos`,
  `systemd` and `oci` serve all three modes from one shared buffer. `pod` refuses `Tail` and
  `HeadAndTail` with `confinement.output-retention-unserved` (501), because the log API bounds a
  read only from its start. No bound changes, and a run that does not set it reports the same bytes
  as before.
- The GitHub release carries prebuilt x86_64 Linux binaries: `substrate-daemon-<version>-x86_64-unknown-linux-gnu.tar.gz` and `b10x-substrate-<version>-x86_64-unknown-linux-gnu.tar.gz`, each with `LICENSE`, `THIRD_PARTY_LICENSES.html` and `README.md`, plus `SHA256SUMS` over both and `SHA256SUMS.sigstore.json`, a keyless `cosign sign-blob` bundle by the release workflow's identity, verified with `cosign verify-blob` before the release is created. The binaries come from the Dockerfile's pinned `builder` stage (new target `release-binaries`, which now also builds the CLI). Before any image tag moves, the workflow refuses a binary that needs a glibc newer than 2.35 or a shared library other than OpenSSL 3, zlib, `libgcc_s` and glibc's own, that does not start in a digest-pinned Ubuntu 22.04 image, or an archived daemon that is not byte for byte the daemon of the image being published; a recovery run that reuses a published image archives that image's daemon. A recovery dispatch of a tag whose source predates the `release-binaries` stage publishes the release without binaries and says so in its notes and summary; a tag push without the stage is refused. README.md § *Release binaries* states the runtime needs, the verification commands and both signing identities (`@refs/tags/<version>` for a tag push, `@refs/heads/main` for a recovery dispatch).

### Changed

- The public site is built with Docusaurus on the shared Docs System, with the same pages, paths, grouping and heading IDs, and the start page unchanged. The Mermaid diagrams on the operations and system model pages render as diagrams, code blocks are highlighted, and the adapter matrices show `TRUE`, `UNKNOWN` (proved by the open-time probe on this host) and `FALSE` chips.
- `cargo xtask docs-provenance --site website/build --commit <full-sha>` replaces `cargo xtask build-docs`: it checks the built site, holds the click ratchet on it, and writes `.well-known/b10x-site.json`, `.well-known/b10x-routes.json` and `.nojekyll`. `cargo xtask check-docs` keeps the Markdown checks and the reading-load ratchet without Node, and checks the pages against `website/sidebars.ts` and the start page's links. `cargo xtask docs-baseline` takes `--site` to measure clicks.
- The *Release notes* page is generated from the newest released `CHANGELOG.md` section by `cargo xtask docs-releases`, and `check-docs` fails when the two differ.
- *Getting started* now installs the command line and runs one command under the `oci` or `linux` adapter without a daemon, then shows a named refusal. The daemon walkthrough moves, with its heading IDs, to *Run a bounded command*.

## [0.13.0] — 2026-10-27

Published by [release run 40507411588](https://github.com/beyond10x/substrate/actions/runs/40507411588),
the protected-`main` recovery dispatch for tag `0.13.0`, from tagged commit
`c2b95f2882ec1e425459b23ed9e48dae121ae6d3`. The tag-push
[run 40504625490](https://github.com/beyond10x/substrate/actions/runs/40504625490) pushed the
daemon image and failed at keyless signing before anything was announced; the recovery run checked
that image's source-revision label and shipped binary again and reused it. All three artifacts were
keyless-signed and verified against `release.yml@refs/heads/main` by the release workflow, and each
digest resolved anonymously:

- Image: `ghcr.io/beyond10x/b10x-substrate-daemon:0.13.0` at `sha256:81a5d4d5d47520483dd6020549d63bb82ec3b636885c4f6aadab02b5a6040f22`.
- Contract bundle: `ghcr.io/beyond10x/b10x-substrate-wire:0.23.0` at `sha256:c2a50222da154028011a794cc93724043943581ba4ee44051c38dd084b759ff5`, unchanged since 0.11.0 and signed again by this release.
- Disposable MCP image: `ghcr.io/beyond10x/b10x-substrate-mcp:0.13.0` at `sha256:ae0d64f50b7bae805b5bb4d3ceb24abd8bb4cbe32cbd4b9aeb216840c127f5ec`, development-only.

### Added

- The public site explains each confinement adapter (what it enforces, declares, refuses by name and needs, and when to choose it). It also gains an overview of the contracts with the ten families and eleven facts, release notes rendered from the newest released `CHANGELOG.md` section, and `confine run`; the status page moves to 0.12.0. The page list is grouped, marks the current page, and collapses on narrow screens; tables scroll inside focusable regions.
- `cargo xtask check-docs` holds the site's adapter matrices, family, fact and lane tables and CLI Commands table to the specification. It also holds the site to lower-is-better UX ratchets in `website/ux/baseline.json` (clicks per reader task, words per text block), which `cargo xtask docs-baseline` rewrites, and checks the `website/ux/findings/` records.
- The `macos` adapter resolves the selected developer tools' macOS SDK, `clang` and `ar` with `xcrun` outside the sandbox at open, with a cleared environment and `/` as its working directory, and a start gets `SDKROOT`, `CARGO_TARGET_AARCH64_APPLE_DARWIN_LINKER`, `CARGO_TARGET_X86_64_APPLE_DARWIN_LINKER`, `CC` and `AR` for each of them its exec subpaths reach, unless its environment names it, so a confined `cargo build` or `cargo test`, and a build script that compiles C, links without reaching the `/usr/bin/cc` shim, which runs `/bin/sh` and `xcodebuild`. A start over the developer tools is refused `exec.sandbox-developer-tools-unresolved` before anything runs, naming why, when the open could not resolve them, when the selected developer tools changed since the open, when the SDK the open resolved is gone, or when its exec subpaths hold the selected developer directory but not a value it needs; nothing is resolved again at start. The Seatbelt policy gains no rule. `tests/seatbelt_adversary.rs` runs the fixture's `cargo test` with no `RUSTC`, `SDKROOT` or linker set by the caller.

### Changed

- The gate workflow's `pod-lane` job runs `scripts/pod-lane.sh`: the `ess_confinement_pod` and `adversary_pod` suites against the `kind` cluster, each required by its own `test result:` line with at least one case passed, none failed, ignored or filtered out, and no case reporting itself absent; the script takes no arguments. `security_review_pod` reads no lane variable and stays in the workspace gate. The job adds a second ServiceAccount bound to the Pod and NetworkPolicy Role without `pods/log` for the no-log case, and names the kind node for the kubelet-stop case, which now waits for the adapter to remove the run's Pod and NetworkPolicy once the kubelet answers again.
- `tests/confinement_public_items.rs` now freezes every type the `confinement` module's public items reach, to a fixed point, beside the module's own items. From the host crate that adds `SecretSlot`, `EgressAperture`, `GitSourceBinding`, `DriverError` and `DriverErrorClass`. From `substrate-wire` it adds `CapabilitySnapshot`, `CapabilityFacts`, `HostDriverKind`, `EgressApertureFact` and the fact types `CapabilityFacts` holds (`StorageQuotaFacts`, `ScratchQuotaFacts`, `ReadOnlyRootFacts`, `ReadOnlyFileFacts`, `ResourceUsageFacts`, `MetricsStreamFacts`, `StreamBoundsFacts`, `AttachmentBoundsFacts`, `ExecutionCapsuleFacts`, `NamespaceFacts`, `CgroupLimitFacts`). The list also covers:
  - an `impl` of a module type in any file of the host crate;
  - the associated types and constants of trait impls on held types;
  - a wire type that shares a name with a module type;
  - a type reached through a renaming `pub use … as …`.

  `tests/confinement-public-items.txt` lists them. Adding, removing or reshaping any of them changes the list.
- `b10x-substrate confine run` names its private group `confine-<pid>-<start time>` (the start time is field 22 of `/proc/<pid>/stat`). Before opening, it removes each such group whose owner has exited (including an owner still unreaped), deepest first, if the group holds no process. It never touches a running owner's group and never kills. Each group it leaves holding a process, or cannot remove, is named on one stderr line, and so is an unreadable root. Under `--output json`, these go instead into the verb's one document, as `left_groups` (`path` and `reason`) in `result` or in `error.data`. The exit status does not change.
- The public site's command-line guide describes how `confine run` removes a killed run's group and `left_groups`; the profiles guide describes the `macos` developer-tool resolution and its refusal.

### Fixed

- An egress aperture's listening socket is shut down when an aperture list fails to install, before its forwarders are killed. Before, only the installing process's descriptor was closed, so a child another thread forked meanwhile could keep the socket accepting connections from inside the run. A list that installs completely closes the installing process's copy without a shutdown, so the forwarder holds the last one and a forwarder that dies mid-run leaves its aperture refusing connections. A descriptor received and then refused inside the install is shut down as well.
- `scripts/delegated-lane.sh` and `scripts/pod-lane.sh` read each suite's log through a here-string. Piped into `grep -q` under `pipefail`, a log longer than a pipe buffer after a match read as no match, so an `ABSENT` line could pass the lane.

## [0.12.0] — 2026-10-27

Published by [release run 40469131133](https://github.com/beyond10x/substrate/actions/runs/40469131133)
from tagged commit `65e1fca7185dfb0a5a116b70b9046b1de5722d68`. All three artifacts were
keyless-signed and verified against `release.yml@refs/tags/0.12.0` by the release workflow, and each
digest resolved anonymously:

- Image: `ghcr.io/beyond10x/b10x-substrate-daemon:0.12.0` at `sha256:eca4a8b5118bd012ed9c96d6ffa392d0983ed5375356504c6df56220e4f50802`.
- Contract bundle: `ghcr.io/beyond10x/b10x-substrate-wire:0.23.0` at `sha256:c2a50222da154028011a794cc93724043943581ba4ee44051c38dd084b759ff5`, unchanged since 0.11.0 and signed again by this release.
- Disposable MCP image: `ghcr.io/beyond10x/b10x-substrate-mcp:0.12.0` at `sha256:2d365d7041feaff960fab55bcb1405a44c7b790db7936a3f3bdf1b0e78683403`, development-only.

### Added

- The confinement contract is specified as the ESS domain `substrate.confinement` (ADR 0044): ten contract families, six adapters with their declared facts and named refusals, 49 authored scenarios named by adapter and lane, checked against the interpreted reference by `cargo xtask check-ess`.
- `substrate_host::confinement` implements that contract in Rust (ADR 0044): `ConfinementProfile`, the `ConfinementAdapter` trait, the `linux` adapter over the host driver, the declared-insecure `local` adapter, the six-adapter registry and refusing `macos`, `systemd`, `oci` and `pod` stubs, with the authored scenarios executed by `tests/ess_confinement_local.rs` (portable) and `tests/ess_confinement_linux.rs` (delegated).
- The `macos` confinement adapter (ADR 0037, ADR 0109 S1) runs a command under a Seatbelt profile through `confinement::open`, refuses `limits`, `network_egress`, `whole_tree_kill` and a read-only root mounted away from its host path by name, fails closed on the Seatbelt probe, and is proved on the `macos-15` job.
- The `systemd` confinement adapter (ADR 0109 decision 3) asks the user's systemd manager for one transient `Delegate=yes` scope holding the embedding process: around `local` each run gets its own bounded cgroup (pids, memory with no swap, timeout, cumulative CPU, `cgroup.kill`) and stays declared insecure; around `linux` the scope is the host driver's delegated cgroup root. Without a reachable user manager it is refused `confinement.adapter-unserved`; `tests/ess_confinement_systemd.rs` runs every `systemd` scenario of the suite and real runs under `scripts/delegated-lane.sh`.
- The `oci` confinement adapter runs each run in one Docker or podman container (ADR 0044 decision 8): the image by sha256 digest only, no network, every capability dropped, a read-only root, the profile's bounds as `--pids-limit`, `--memory`, `--memory-swap` and `--cpus`, and the container removed after `wait`, a kill or a timeout; `tests/ess_confinement_oci.rs` executes the `oci` scenarios against Docker where `SUBSTRATE_OCI_LANE=1`, which the CI gate job sets.
- The `pod` confinement adapter runs each start as one Kubernetes Pod beside a default-deny NetworkPolicy in the operator's namespace (design 28): restricted Pod posture, an image by digest, requests equal to limits, a 1 GiB ephemeral-storage and workspace bound unless the operator states another, `activeDeadlineSeconds` from the wall bound, the argv and environment passed literally (every `$` written `$$`), the Pod never force-deleted and its policy deleted only once the Pod is confirmed gone (otherwise the run reads `Unknown`), open refused without `pods/log` access, an unreadable log reported as an error and never as empty output, and every Pod field a request cannot choose refused by name before any API call; `RunObservation` gains `memory_oom_kills`, which `pod` reads from the container's terminated state and every other adapter leaves absent; `tests/ess_confinement_pod.rs` checks the rendered Pod against a golden manifest and runs the `pod` lane against a pinned `kind` cluster in the gate workflow's `pod-lane` job.
- The `macos` adapter's build policy (ADR 0109 S2) proves `sealed_paths`, `exec_subpaths` and `private_tmpdir` by a Seatbelt probe at open: sealed workspace paths and the directories above them stay unwritable, programs run beneath declared subpaths (the workspace, a declared root, system and developer directories), and each run gets its own `TMPDIR`, removed before `wait` returns; `tests/seatbelt_adversary.rs` runs `cargo test --offline` of a fixture crate with a vendored dependency and a build script under it on the `macos-15` job.
- The `macos` adapter refuses a relative exec subpath with a symlink component (`exec.sandbox-path-unrepresentable`) and any `DYLD_*` environment variable (`exec.environment-refused`) before anything runs; it makes a run's private `TMPDIR` removable before removing it and writes a `private_tmpdir_left` audit record when removal still fails; a policy with an exec subpath reads `/private/etc/ssl` and the Xcode licence record, so cargo and `xcrun` start under it. ADR 0037's gap table records the unscoped `sysctl-read`.
- The `confinement` module's consumer surface is proved and frozen: `tests/consumer_surface.rs` chooses adapters by `served_families` and facts only and runs a read-only toolchain root, a workspace writing `target/`, a working directory beneath it, a build into a declared writable install root and a `--version` run of the result (portable lane on `local`, which refuses each filesystem view as `confinement.family-unserved`; delegated lane on `linux`, where every need runs); `tests/confinement_public_items.rs` fails when a public item of the module is added, removed or changes signature without `tests/confinement-public-items.txt`; the public site gains *Choose a confinement profile*.
- `b10x-substrate confine run` runs a command through `confinement::open` without a daemon, with the sandbox repository's flags (`--dir`, `--ro`, `--net`, `--backend linux|oci`, `--image`, `--dry-run`, `--local <reason>`): every refusal exits 2 before anything runs, `--image` is admitted only with `--backend oci` and by sha256 digest (elsewhere it is `confinement.fact-unserved: image_digest_pinned`), `--local` says `adapter=local declared_insecure=<reason>` on stderr, and `tests/sandbox_parity.rs` proves the linux and oci renders of the verb's own command carry sandbox's bubblewrap and Docker flags apart from six it names. Every run gets sandbox's five baseline variables (`PATH=/usr/bin:/bin`, `HOME=/tmp`, `LANG` and `LC_ALL` `C.UTF-8`, `TZ=UTC`). `linux` reads its delegated cgroup v2 root from `SUBSTRATE_CONFINE_CGROUP_ROOT`, never the daemon's `SUBSTRATE_CGROUP_ROOT`, and opens its driver on a private group it makes beneath that root and removes at exit, so concurrent invocations and a daemon on the same root never reconcile each other's runs; a `--dir` whose name is outside `[A-Za-z0-9_-]+` is refused naming that rule; a relative `--ro` resolves `.` and `..` lexically; `--dry-run` meets the admission a run meets and says its descriptor numbers are local to the render; the private state directory is mode 0700; SIGINT and SIGTERM kill the run, remove the state directory and the private group, and exit 128 plus the signal. The host driver's open-time workspace probe no longer writes to the embedding process's stdout or stderr.

### Changed

- The specification and CI use ESS 0.70.0 (was 0.69.2); the suites, wire types and CLI package regenerate unchanged apart from the generator header.

## [0.11.0] — 2026-10-26

Published by [release run 40349695547](https://github.com/beyond10x/substrate/actions/runs/40349695547)
from tagged commit `2f886f2a2de2ff072a5d3ad755e333f7c8a7166b`. All three artifacts were
keyless-signed and verified against `release.yml@refs/tags/0.11.0` by the release workflow, and each
digest resolved anonymously:

- Image: `ghcr.io/beyond10x/b10x-substrate-daemon:0.11.0` at `sha256:ad4adf6fce067355d2d3df7dfd0511ccedb723fafb194f5b28f7f70020be440a`.
- Contract bundle: `ghcr.io/beyond10x/b10x-substrate-wire:0.23.0` at `sha256:c2a50222da154028011a794cc93724043943581ba4ee44051c38dd084b759ff5`, first published by this release.
- Disposable MCP image: `ghcr.io/beyond10x/b10x-substrate-mcp:0.11.0` at `sha256:70f9d5046e753618ddfb49972b0b159cc93301d46a9e1d247d86ddaa0f6512e3`, development-only.

### Added

- Development contract bundle `substrate-wire/0.23.0`, additive over `0.22.0` with no new route.
  It publishes the optional `read_only_roots` start list (at most 4 `{host_path, mount}` entries),
  the `exec.read-only-roots` fact `{"max_roots": 4}`, the applied `read_only_roots` record and
  three named refusals (`exec.read-only-root-invalid`, `-absent`, `-not-a-directory`, 422)
  (ADR 0010). The exec applied record now admits `secret_slots` and `read_only_files`, which
  earlier bundles wrongly rejected; those bundles stay frozen.
- Bundle `0.23.0`: the session start's `exec` object publishes `scratch`, `measurements` and
  `workspace_access` with the same schemas as exec start, and both starts publish the refusals
  these members have always raised (design 27). No refusal is new.
- Capability facts `events.stream-bounds` and `metrics.stream-bounds` (`per_subject` 4, `global`
  64, `lifetime_ms` 3600000, `refusal`) and `sessions.attachment-bounds` (`per_session` 1,
  `per_subject` 16, `global` 32, `lifetime_ms` 3600000, `refusal`
  `session.attachment-capacity`), published in bundle `0.23.0` (ADR 0042).
- A v1 directory page and a v2 tree name the entries they omit (a FIFO, socket, device or
  non-UTF-8 name) in the `x-b10x-omitted-entries` response header, sent only when at least one
  was omitted; bundle `0.23.0` publishes it as an `x-b10x-response-headers` annotation
  (ADR 0043). The v2 tree's `truncated` still answers true for an omission as well as for the
  limit; the header tells them apart. The Rust SDK adds `Workspace::tree_with_omissions` and
  `Workspace::read_directory_with_omissions`; `tree` and `read_directory` are unchanged.
- `cargo xtask quota-fixture run` builds the checked-in recipe `fixtures/quota-lab/` and runs
  the seven `real_quota_*` cases on an ext4 project-quota loop filesystem in a container, adding
  only `SYS_ADMIN` and never `--privileged`. It runs from a `git archive` snapshot of `HEAD` and
  prints a `Quota-lane:` record only when the checkout was clean before and after and `HEAD` did
  not move.
- README § Serving exec states what the published daemon image carries for exec (bubblewrap
  0.11.2 and socat) and the four things a deployment still supplies before exec is advertised.
- ADR 0041 records the one-hour session attachment lifetime, killed on expiry, disconnect or
  protocol error, with a one-shot attachment right.

### Changed

- Every workspace package and its exact internal version edges move from 0.10.0 to 0.11.0. Minor:
  `substrate-wire/0.23.0` adds read-only roots, session-start exec fields and stream bounds.
- The daemon and the Rust SDK advertise `substrate-wire/0.23.0`.
- A confined exec's `socket(2)` is an argument allowlist: `AF_INET`/`AF_INET6` with protocol 0,
  TCP or UDP, `AF_NETLINK` with `NETLINK_ROUTE`, and the existing `AF_UNIX` floor;
  `socketpair(2)` admits only `AF_UNIX` stream and seqpacket pairs; `ioctl(TIOCSETD)` and every
  x32 syscall are refused, each with `EACCES` (ADR 0039). A confined child can no longer make the
  host kernel autoload `sctp`, `n_hdlc` or `scsi_transport_iscsi` through these calls.
- Hosted admission reuses its connections to Identity through a bounded keep-alive pool (at most
  8 idle, closed after 30 s idle, never reused after an error, a refusal or a close-requesting
  response, retried once on a fresh connection only when the request was never sent). Every
  request still resolves online, so revocation applies to the next request (ADR 0040).
- An upgraded WebSocket (event stream, metrics stream or session attachment) hands its
  connection's transport slot to its stream at the upgrade, so streams inside their caps no
  longer block co-located callers from connecting (ADR 0042). One subject holds at most 16
  session attachments; the 17th is refused `session.attachment-capacity` (429).
- `GET /v1/metrics/stream` refuses `501 exec.metrics-unserved` before the upgrade on a host that
  does not publish `metrics.stream`.
- The daemon refuses `scratch: null`, a storage `max_bytes` that is not a multiple of 1024 (exec,
  session and workspace storage) and a measurement listed twice as `request.schema-invalid`
  (422), as the published schemas always did. A misaligned workspace quota was `429
  storage.quota-limit` before.
- A release refuses unless its annotated tag, or the `quota_evidence` dispatch input, carries
  `Quota-lane: <sha> <case-count> <RFC 3339 UTC>` for the tagged commit. Both lane records are
  refused when they span more than one line, lack exactly three fields, appear twice in the tag,
  or name a timestamp that is not a real UTC instant.
- The specification, its generated suites and wire types, and CI use ESS 0.69.2 (was 0.68.0);
  generated vocabulary variants carry their spelling's name, so the hand-written aliases are gone.

### Fixed

- An exec recorded `unknown` because its cgroup could not be proven empty is killed again on
  lease cleanup and once per maintenance round until its tree is proven gone, instead of keeping
  its processes until the daemon restarts.
- `session.retire` refuses a session in `unknown` until recovery has recorded its tree as
  physically gone.
- Removing an exec's cgroup removes nested cgroups deepest first (at most 32 levels and 256
  groups, never following a symlink), so an empty nested group no longer pins the exec's slot.
- The host driver checks the `exec.read-only-roots` fact before opening any caller-named root,
  and refuses `exec.sandbox-unavailable` without it.
- `cargo xtask check-secrets` counts only commits Gitleaks scans, so a delete-only, binary-only
  or mode-only commit no longer fails the gate; it runs without textconv drivers, replace objects
  or grafts, diffs merges with `--diff-merges=separate`, and refuses a shallow repository.

## [0.10.0] — 2026-10-22

Published by [release run 39866527468](https://github.com/beyond10x/substrate/actions/runs/39866527468)
from tagged commit `105cf718bc6a5314486c78f5fd93fabed290e30e`. All three artifacts were
keyless-signed and verified against `release.yml@refs/tags/0.10.0` by the release workflow, and each
digest resolved anonymously:

- Image: `ghcr.io/beyond10x/b10x-substrate-daemon:0.10.0` at `sha256:db404a84e6ba528dfeed5c3765d79c80d858cd0e6b5d8ed881aa7eb8bca80fd3`.
- Contract bundle: `ghcr.io/beyond10x/b10x-substrate-wire:0.22.0` at `sha256:adb6257379abb402ff1e906c0d86a240cb2682939925796fc37a9bc046d356cd`, first published by this release.
- Disposable MCP image: `ghcr.io/beyond10x/b10x-substrate-mcp:0.10.0` at `sha256:751cb27ad1799c0f67f3f10ca0842486c8117c80757f3185164b8e5f4d40c380`, development-only.

### Added

- Development contract bundle `substrate-wire/0.22.0` (ADR 0038), additive over `0.21.0` with
  its 44 routes unchanged. The daemon and SDK advertise it at inner `bundle.json` digest
  `5d551f08c3b680c144359b13ff99a7da2853003c4fcc9665a32b21ce85e146da`. `cargo xtask check-bundle
  0.22.0` holds its bound, reserved mounts, facts, refusals, vectors and compatibility text to the
  wire constants, and the gate checks it beside every earlier bundle. Its `writable-roots` register
  publishes the six host refusals beside the eight lexical and unserved ones, as `0.21.0` publishes
  the read-only-file host codes: `exec.writable-root-absent`, `-not-a-directory`, `-not-owned`,
  `-not-git`, `-own-workspace` and `-workspace-duplicate`, each 422 `refused`.
- An exec or session start may name `writable_roots`, at most `MAX_WRITABLE_ROOTS` (8) entries,
  each either `{host, mount}` naming an operator-declared host root or `{workspace, mount}` naming
  a Git checkout workspace. A start that names none serializes and hashes as before. The daemon and
  host refuse `exec.writable-roots-exceeded`, `exec.writable-root-name-invalid`,
  `exec.writable-root-mount-invalid`, `exec.writable-root-mount-reserved`,
  `exec.writable-root-mount-duplicate` and `exec.writable-root-mount-overlap` (422) before any
  capability is consulted, then `exec.writable-roots-host-unserved` and
  `exec.writable-roots-git-unserved` (501) with the root's name.
- Writable host roots (ADR 0038 kind 1). `substrate-daemon --writable-root <name>=<path>`
  (repeatable, `SUBSTRATE_WRITABLE_ROOT`) declares a host directory a start may name. Startup
  refuses, naming the root, a name outside the root-name set or declared twice, and a path that is
  not absolute and normal, does not exist, is not a directory, is reached through a symlink, is not
  owned by the daemon's uid, lies under or contains the workspace root or the state directory, or
  equals or contains the API socket, the daemon executable, the bubblewrap binary, a secret-slot
  source, the CA bundle, the TLS private key or certificate chain, the hosted Identity CA bundle,
  the static-bearer file or the delegated cgroup root, each compared as written and as resolved.
  A path that is not UTF-8 stops startup with `config.writable-root-invalid` (ADR 0038 decision 2).
  The host publishes `exec.writable-roots.host`, the declared names, only when roots are declared
  and a throwaway sandbox proved a root bound by descriptor writable beneath a read-only parent,
  with no descriptor left resolving to either; otherwise the fact is absent and every host root is
  `exec.writable-roots-host-unserved` (501). An undeclared name stays unserved by name. At each
  start the driver opens every named root once, `openat2` with `RESOLVE_NO_SYMLINKS`, checks on
  that descriptor that it is a directory this uid owns, and bubblewrap mounts the descriptor with
  `--bind-fd`, so a rename or symlink swap after admission cannot redirect the mount. A root that is
  gone is `exec.writable-root-absent`, one that is not a directory or is reached through a symlink
  is `exec.writable-root-not-a-directory`, and one this uid does not own is
  `exec.writable-root-not-owned` (422), each naming the root. The declared roots and their paths
  move the configuration generation; a daemon that declares none keeps its generation.
- Writable Git roots (ADR 0038 kind 2). A start may name `{workspace, mount}` for a Ready
  workspace of the same deployment and subject that was created from a Git source; the host binds
  that workspace's directory writable by descriptor at `mount`, `.git` included, so a program can
  commit in the checkout, and its storage quota still applies. The host publishes
  `exec.writable-roots.git` only when it serves `workspace.git` and the writable-bind probe passes;
  otherwise every Git root is `exec.writable-roots-git-unserved` (501). Before dispatch, and
  recorded in the operation ledger, the daemon refuses the start's own workspace
  (`exec.writable-root-own-workspace`), a workspace named twice
  (`exec.writable-root-workspace-duplicate`) and an empty or adopted workspace
  (`exec.writable-root-not-git`), each 422 naming the workspace; an unknown workspace or another
  subject's answers the same 404 `resource.not-found`, and one that is not Ready answers 409
  `workspace.not-ready`, both at `writable_roots`. The start locks its own workspace and every Git
  root in one order through dispatch. The transaction that accepts the start checks again that its
  own workspace and every Git root are Ready with no lapsed lease; one whose lease fell due since
  admission is frozen there and the start answers 409 `workspace.not-ready`, not retriable, never
  500 `state.store-failed`. The exec row records the Git roots it binds in the
  transaction that accepts it (a new `git_root_workspaces` column; an existing store fills it from
  each exec's applied record), so a workspace an Accepted, Running or Unknown exec binds is not
  destroyed under it (`workspace.execs-active`, 409), and workspace lease expiry kills every exec
  that binds the workspace, as it kills the execs in it, before it ends the workspace. Bundle
  `0.22.0` publishes these three codes with the three host-path refusals above.
- A `workdir` may lie at or beneath a writable root mount, and the mount runs the program there.
  The applied confinement record gains an optional `writable_roots` list (kind, name or workspace
  id, host path, mount), filled from what was bound.
- The clean-room runtime vectors declare one writable root: the portable lane refuses it unserved,
  and the delegated lane binds it, sees the write on the host with its parent and sibling
  unchanged, runs a working directory beneath its mount, and refuses it by name once its path is
  swapped for a symlink. A second daemon declares a Git source served over HTTPS on loopback: the
  portable lane refuses its Git workspace as a Git root unserved (111 cases in all), and the
  delegated lane (167 cases) has a child commit in the mounted checkout, reads the commit on the
  host and in the change-set route, and refuses each named Git-root case before dispatch.

### Changed

- Every workspace package and its exact internal version edges move from 0.9.0 to 0.10.0. Minor:
  `substrate-wire/0.22.0` adds `writable_roots` as new public surface.

## [0.9.0] — 2026-10-22

Published by [release run 39756033654](https://github.com/beyond10x/substrate/actions/runs/39756033654)
from tagged commit `9c5f38be3f22234367dfc068c7d34cd560e8f6fd`. All three artifacts were
keyless-signed and verified against `release.yml@refs/tags/0.9.0` by the release workflow, and each
digest resolved anonymously:

- Image: `ghcr.io/beyond10x/b10x-substrate-daemon:0.9.0` at `sha256:da28e2c86f3753e5a566e31ffa6652d05caecc87d9ece96d3929742dbe6752c3`.
- Contract bundle: `ghcr.io/beyond10x/b10x-substrate-wire:0.21.0` at `sha256:448423df3fd7669053636732a407af629305dac25e32a84482e15cb72da22b01`, first published by this release.
- Disposable MCP image: `ghcr.io/beyond10x/b10x-substrate-mcp:0.9.0` at `sha256:5ba10859d46cff8223c5b3fa6947509fc8e150406d5b916f95272ff9f2f3d830`, development-only.

### Added

- Development contract bundle `substrate-wire/0.21.0` (design 26), additive over `0.20.0` with
  its 44 routes unchanged. The daemon and SDK advertise it at inner `bundle.json` digest
  `044b3099d8e5a1108cc75cf3b03f1ea4d250ee4e34266cf493276fe4a9a0fe15`. `cargo xtask check-bundle
  0.21.0` holds its bounds, refusals, vectors and compatibility text to the wire constants, and
  the gate checks it beside every earlier bundle.
- An exec or session start may name `workdir`, the directory the program starts in. It must be an
  absolute, lexically normal path of at most `MAX_WORKDIR_BYTES` (4096) bytes at or beneath a
  mount the start places: `/workspace`, a declared read-only root, `/scratch` with scratch, or
  `/runtime` with a capsule. The daemon refuses `exec.workdir-invalid` and
  `exec.workdir-outside-mounts` (422) before dispatch. The Linux host publishes the
  `exec.workdir` fact only after a throwaway sandbox ran in a named directory and refused an absent
  one; without it a start that names a directory is `exec.workdir-unserved` (501). The host walks
  the directory beneath the object its mount binds with guarded `openat2` before anything runs,
  refuses an absent, non-directory or symlinked one as `exec.workdir-missing` (422), names a
  directory removed after that check by bubblewrap's exact `--chdir` diagnostic on the
  observation, and reports the directory on the applied record.
- A sandbox may name `apertures`, a list of two to `MAX_EGRESS_APERTURES` (4) distinct aperture
  names, instead of one `aperture`. Every name follows the single-aperture rules, answered at
  `sandbox.apertures`; a list of one is `exec.apertures-too-few`, more than four
  `exec.apertures-too-many`, a repeated name `exec.aperture-listed-twice`, and a list beside
  `aperture` or without network `aperture` `exec.aperture-mode-mismatch` (all 422). Each name must
  be declared (`exec.aperture-undeclared`, 501). The applied network gains a third form, an array
  whose elements carry the `listen` address; a single aperture keeps its `0.20.0` object.
- The Linux host installs every listed aperture in one sandbox, the n-th listening on
  `127.0.0.n`, and reports each in the applied array with its `listen` address. Before anything is
  installed it refuses `exec.aperture-host-listed-twice` (422) when two listed apertures declare
  one host name (ASCII case-insensitive, without a trailing `.`), or when any but the first
  declares `localhost`, which the generated `/etc/hosts` already gives to the first listen
  address. The generated mapping refuses the same set again as `exec.aperture-install-failed`, and
  a listed set that names nothing is refused as `exec.apertures-too-few` at the host as well as
  the wire, so it never runs without a network.
- The clean-room runtime vectors cover both: 99 portable cases (was 90) and 147 delegated cases
  (was 132), including programs started beneath the workspace and a declared root, absent or
  symlinked directories refused before dispatch, a run that reads from two apertures and reports
  both, and a list refused `exec.aperture-host-listed-twice` for `localhost` listed second.

### Changed

- The host checks a working directory beneath a declared read-only root through the descriptor
  admission pinned for that root, not by resolving its path again, so a rename or swap after
  admission cannot change which directory is checked.
- Declared read-only roots are opened once at admission and mounted by descriptor.
- Every workspace package and its exact internal version edges move from 0.8.0 to 0.9.0. Minor:
  `substrate-wire/0.21.0` adds `workdir` and `apertures` as new public surface.

### Fixed

- The host read bubblewrap's `--info-fd` report only up to the child pid and then closed the
  pipe; bubblewrap's next write could take `SIGPIPE`, and an exec with apertures was refused
  `exec.aperture-install-failed`. The host now reads the report to its closing brace.

## [0.8.0] — 2026-10-22

Published by [release run 39699794271](https://github.com/beyond10x/substrate/actions/runs/39699794271)
from tagged commit `3798b86faafcb995c20b15749b6e97513037b5e9`; its first attempt failed on a GHCR
`unknown blob` before any tag was written, and the re-run of that job published. All three
artifacts were keyless-signed and verified against `release.yml@refs/tags/0.8.0` by the release
workflow, and each digest resolved anonymously:

- Image: `ghcr.io/beyond10x/b10x-substrate-daemon:0.8.0` at `sha256:180a5716366d7811a878aa1aaff23c41c3a52b7756f851a9f3d6df99bc2f146e`.
- Contract bundle: `ghcr.io/beyond10x/b10x-substrate-wire:0.20.0` at `sha256:007a18088e2af6af70f02cbc951208f66d4310b00eaa6a2fda4991399670634c`, first published by this release.
- Disposable MCP image: `ghcr.io/beyond10x/b10x-substrate-mcp:0.8.0` at `sha256:506e1c09e5a05d1864bbb202e70f73a72e5fd6ec21464c9124c09307c882e5ef`, development-only.

### Added

- The clean-room runtime vectors drive `workspace.adopt` through the shipped daemon: an operator's
  directory without the `ws_` prefix is adopted, out-of-set root names are refused as
  `request.schema-invalid` before dispatch and replayed from the ledger, an exec addressing the
  directory by its root name is refused, and destroying it leaves the operator's files. On the
  delegated lane an exec runs confined in the adopted directory with its output and exit observed,
  and the isolation cases run against the adopted workspace as well as a created one; on the
  portable lane the adopted exec is `exec.sandbox-unavailable`. A no-nested-user-namespace case
  joins the isolation cases.
- The clean-room route inventory reads the `0.20.0` operation registry and probes the seven
  host-process routes through the shipped daemon. Without a delegated cgroup `process.host` is not
  published, a start, stop, stop by owner and lease renewal answer `process.host-unserved` (501),
  the list is empty and an unknown record is `process.not-found`; with one, `process.host` is
  published, a start without a profile is `process.profile-required` and nothing runs. The runtime
  vectors grow to 90 portable and 132 delegated cases.
- Development contract bundle `substrate-wire/0.20.0`, the additive successor of `0.19.0` (ADR 0036
  S1 to S7 and S11): the seven host-process routes under `/v1/processes` with the `process` scope,
  the optional `process.host` capability fact, the closed `process` record and log tail, their
  bounds, the seventeen named refusals and the shared `lease.expired` (409) on lease renewal, each
  held to its `substrate_wire` constant by `cargo xtask check-bundle 0.20.0`. The daemon and Rust
  SDK advertise it.
- `substrate_wire`: the host-process wire types generated from `spec/host-processes/wire.yaml`,
  their bounds, refusals, `PROCESS_SHARED_REFUSALS` (`LEASE_EXPIRED`), `PROCESS_HOST_FACT` and
  `PROCESS_ID_PREFIX`.
- The daemon serves the host-process routes (ADR 0036 S1 to S7 and S11) on a local Unix socket
  only: `proc_` ids, the `process` scope, `process.host` published only with a delegated cgroup,
  one cgroup per process with whole-tree stop, renewable leases and hard deadlines on a supervisor
  clock, bounded owner-private logs, and restart re-attachment by cgroup membership. The daemon
  holds the supervisor through the `ProcessSupervisor` and `ProcessLogs` port traits. A stop that
  lands while a process starts never leaves it running unsupervised, a failed re-attachment is
  retried, and every start operation is concluded after a restart. Restart reconciliation tells
  this run's records from an earlier run's by a per-run id stored on each record, so a wall-clock
  step back across a restart neither orphans a running process nor abandons a dispatched start.
- `b10x-substrate-host` gains `seatbelt`, the macOS confinement primitive of
  [ADR 0037](adr/0037-macos-confinement-is-the-platform-sandbox.md). A `SeatbeltPolicy` (one
  writable workspace, read-only roots, an exec allowlist, no network) renders a `(deny default)`
  SBPL profile; on macOS a command runs under it through `/usr/bin/sandbox-exec` with a cleared
  environment, null stdin and its own process group. A path SBPL cannot carry as a plain literal
  is refused as `exec.sandbox-path-unrepresentable`, a program outside the allowlist as
  `exec.sandbox-program-unlisted`, and a missing `sandbox-exec` as `exec.sandbox-unavailable`,
  each before anything runs. The profile allows reading the root directory listing because the
  dynamic loader aborts without it. `HostDriver::open` still refuses on macOS; no wire change.
- The macOS workflow runs `crates/substrate-host/tests/seatbelt_adversary.rs` on `macos-15`: seven
  attacks on the profile, each paired with a control that succeeds where the profile allows it, and
  the step fails unless all seven ran.

### Changed

- The specification pins ESS 0.67.0, and its suites and generated wire types are regenerated with
  it. The generated `RootName` decoder now checks the root-name alphabet and length; the adopt
  input bypasses that check so an out-of-set name is still refused by the adopt admission with its
  operation recorded.
- `scripts/delegated-lane.sh` also runs the delegated cases of the daemon's host-process tests.

### Fixed

- Exec binds a workspace by a descriptor opened beneath the workspaces root without following
  symlinks (`--bind-fd`), so an adopted directory swapped for a symlink after adoption never mounts
  the link's target: a swapped symlink or file is `workspace.path-escape` and a removed entry
  `resource.not-found`, before any mount. Scoped access walks its writable subtrees beneath that
  descriptor, and exec pins the workspace beneath the driver's own root descriptor.

## [0.7.18] — 2026-10-21

Published by [release run 39580183130](https://github.com/beyond10x/substrate/actions/runs/39580183130)
from tagged commit `17e0e96946b9588c150298ac0f951211681b2043`. All three artifacts were
keyless-signed and verified against `release.yml@refs/tags/0.7.18` by the release workflow, and
each tag resolved anonymously to its released digest:

- Image: `ghcr.io/beyond10x/b10x-substrate-daemon:0.7.18` at `sha256:b532d3c6aca309f606d551c02ea5b977292bfbee641337fece57145a51be41d8`.
- Contract bundle: `ghcr.io/beyond10x/b10x-substrate-wire:0.19.0` at `sha256:5c392e33f56a24ac18ab50e4c86d5bfce4bbe1dbe1c122e102a2f7f2b00a8e91`, the byte-identical bundle first published by 0.7.17, signed again with this release's identity.
- Disposable MCP image: `ghcr.io/beyond10x/b10x-substrate-mcp:0.7.18` at `sha256:9d5b43505dd6712c494789e8d3aba8bb047e54ae6252cf0c1c803db3e3108302`, development-only.

The [release notes](https://github.com/beyond10x/substrate/releases/tag/0.7.18) provide exact
signature verification commands and the successful 107-case delegated confinement record. Rust
consumers pin the Git revision `17e0e96946b9588c150298ac0f951211681b2043` or the tag.

### Changed

- `b10x-substrate-host` compiles on macOS and every other non-Linux target. Its Linux modules are
  gated on `target_os = "linux"`; elsewhere `HostDriver::open` refuses with
  `host.platform-unserved` (class `unserved`, address `host.platform`) and
  `HostProcessSupervisor::open` with `process.host-unserved`, both before touching the host. Linux
  behaviour and the public paths of `EgressAperture`, `ExecObservation`, `PipeFrame` and
  `PipeStream` are unchanged. 0.7.17 failed with 83 errors on `aarch64-apple-darwin`.
- CI: a macOS job checks, lints and tests `b10x-substrate-host` on every pull request.

### Security

- `rustls-webpki` 0.103.15 → 0.103.16 for RUSTSEC-2026-0382 (certificate verification may accept
  distrusted intermediates).

## [0.7.17] — 2026-10-17

Published by [release run 39047185868](https://github.com/beyond10x/substrate/actions/runs/39047185868)
from tagged commit `00fa634cb24eb307946bfe3bbe40142f4b7a469a`. All three artifacts were
keyless-signed and verified against `release.yml@refs/tags/0.7.17` by the release workflow, and
each tag resolved anonymously to its released digest:

- Image: `ghcr.io/beyond10x/b10x-substrate-daemon:0.7.17` at `sha256:ce42397ecc80a2ee1c98d25f8341f9ebfb8ca6ef4f678d3b8552be05de5f8137`.
- Contract bundle: `ghcr.io/beyond10x/b10x-substrate-wire:0.19.0` at `sha256:5c392e33f56a24ac18ab50e4c86d5bfce4bbe1dbe1c122e102a2f7f2b00a8e91`, annotated `development`, first published by this release.
- Disposable MCP image: `ghcr.io/beyond10x/b10x-substrate-mcp:0.7.17` at `sha256:cda26e25cf3f09d6675c71242f540fee283ba38b008b95f35f3d02da95ace0d2`, development-only.

The [release notes](https://github.com/beyond10x/substrate/releases/tag/0.7.17) provide exact
signature verification commands and the successful 107-case delegated confinement record. The
`b10x-substrate` client installs from the tag with
`cargo install --git https://github.com/beyond10x/substrate --tag 0.7.17 b10x-substrate-cli --locked`.

### Added

- `workspace.adopt` at `POST /v1/workspaces/adopt` (ADR 0035): serves a directory the operator
  placed under the workspace root, named by `root_name` (ASCII alphanumerics, `_` and `-`, at most
  255 bytes, no leading `-`). The row is Ready, owned by the caller, and gets a minted `ws_…` id;
  the wire `workspace-id` pattern is unchanged. An adopted workspace has no lease expiry and no
  project quota, and `workspace.destroy` forgets its row and leaves the directory. Refusals use
  existing codes and are recorded before any driver dispatch.
- Development contract bundle `substrate-wire/0.19.0`, the additive successor of `0.18.0`: the
  `workspace.adopt` operation and route, and the read-only `origin` (`created` or `adopted`) and
  `root_name` fields on the workspace resource. The daemon and Rust SDK advertise it.
- `spec/host-processes`: the ESS domain of the host-process family (ADR 0036) with its scenarios
  and generated suite, and the store and host groundwork for it in `substrate-store` and
  `substrate-host`: process records with owner, reason, lease and wall-clock deadline, one cgroup
  per process with whole-tree stop, bounded per-process logs and observed listening ports. No
  daemon route serves host processes yet.

### Changed

- ADR 0034 (a workspace addressed by its root name) is superseded by ADR 0035; its widening of
  `workspace-id` never shipped.
- `cargo xtask check-secrets` expects Gitleaks to count every reachable commit that emits a patch,
  roots included, and names each commit that carries none instead of counting it.

## [0.7.16] — 2026-10-16

Published by [release run 38931900566](https://github.com/beyond10x/substrate/actions/runs/38931900566)
from tagged commit `6245807636b432e3f61fb1aa662af8a81c37ea8a`. All three artifacts were
keyless-signed and verified against `release.yml@refs/tags/0.7.16` by the release workflow, and
each tag resolved anonymously to its released digest:

- Image: `ghcr.io/beyond10x/b10x-substrate-daemon:0.7.16` at `sha256:9a5fa7ab955db5773fc9b6ce746b38a61d74304014f1a5f94aa761ec6199fdcd`.
- Contract bundle: `ghcr.io/beyond10x/b10x-substrate-wire:0.18.0` at `sha256:0fc268003bcb02645759cb916f0790359598943c07fa3647320821f2375450ae`, annotated `development`, the byte-identical bundle 0.7.13 published, signed again with this release's identity.
- Disposable MCP image: `ghcr.io/beyond10x/b10x-substrate-mcp:0.7.16` at `sha256:3116fc5638841719c74d6c1765beb2017c218cda3dfa1dd8ed6e51582aaca017`, development-only.

The [release notes](https://github.com/beyond10x/substrate/releases/tag/0.7.16) provide exact
signature verification commands and the successful 106-case delegated confinement record. The
`b10x-substrate` client installs from the tag with
`cargo install --git https://github.com/beyond10x/substrate --tag 0.7.16 b10x-substrate-cli --locked`.

### Added

- `spec/client`: the client CLI's presentation domain and `ess-cli/2` binding for the
  `b10x-substrate` binary, with its generated contract package in `spec/generated/cli`.
  `cargo xtask check-ess` validates both and fails on drift in the package. It types the
  commands, arguments and refusal the CLI presents; observations stay under the released bundle's
  authority, and no conformance suite runs for it.
- `b10x-substrate`, a command-line client of the daemon socket (`crates/b10x-substrate-cli`,
  `publish = false`) over `b10x-substrate-sdk` only: machine facts, workspace create, show, renew
  and destroy, exec run and show, session show and signal, operation show, metrics and events.
  `--output json` prints the binding's envelope with a daemon refusal's named code in
  `error.data.code`; `exec run` passes output through and exits with the command's status. A
  test holds the clap command line to `spec/generated/cli/binding.json`, and the story's five
  scenarios run against the shipped daemon. No route, wire field or bundle changes.
  Install it from a checkout with `cargo install --path crates/b10x-substrate-cli --locked`.

### Changed

- The executable specification is pinned to ESS 0.60.0. The committed operations suite is
  unchanged; the generated capture type differs only in its generator header.

## [0.7.15] — 2026-10-14

Published by [release run 38646569421](https://github.com/beyond10x/substrate/actions/runs/38646569421)
from tagged commit `abc5ea2c9ebd490420b4205d0913ad721c41b3a1`. All three artifacts were
keyless-signed and verified against `release.yml@refs/tags/0.7.15` by the release workflow, and
each tag resolved anonymously to its released digest:

- Image: `ghcr.io/beyond10x/b10x-substrate-daemon:0.7.15` at `sha256:2367f049ddc022a452b2aa04e8afcc8b8d36fb1472267578d5a596fbce4f6073`.
- Contract bundle: `ghcr.io/beyond10x/b10x-substrate-wire:0.18.0` at `sha256:0fc268003bcb02645759cb916f0790359598943c07fa3647320821f2375450ae`, annotated `development`, the byte-identical bundle 0.7.13 published, signed again with this release's identity.
- Disposable MCP image: `ghcr.io/beyond10x/b10x-substrate-mcp:0.7.15` at `sha256:8760c096fbd4a825560f207a4bdac018b3b3a3e92c97c67a3a4435fefb29e472`, development-only.

The [release notes](https://github.com/beyond10x/substrate/releases/tag/0.7.15) provide exact
signature verification commands and the successful 106-case delegated confinement record.

### Security

- A workspace file read, a digested read and the current-content observation of a
  compare-and-set replace, edit or patch no longer block on a FIFO a confined process created at
  the requested path: the file is opened non-blocking and refused as a non-regular file, as
  before. One `mkfifo` and one read request could otherwise park a daemon worker indefinitely.

### Changed

- Containment after a failed WebSocket upgrade no longer runs in a detached task. The daemon's
  containment step performs it, and that step stops with the lease sweeper on every shutdown path,
  including the early return of TLS identity setup.

### Fixed

- A claimed pipe session whose containment kill the driver refused or did not answer is retried
  until the kill is proven, the exec is terminal or its lease has expired; before, only lease
  expiry ended it. Retries run in their own containment step, so a kill the driver never answers
  holds up no other maintenance. Each round takes up to 64 pending sessions, least recently
  attempted first; an exec that ends `unknown` leaves the step with a warning rather than being
  reported as terminated; a repeated failure is logged on its 1st, 2nd, 4th, 8th attempt and
  so on.
- A directory page and the v2 workspace tree no longer refuse with `workspace.path-escape` when a
  directory holds a FIFO, a socket or a name that is not UTF-8. Such entries are omitted, as `.git`
  is, and the tree answers `truncated: true` when it omits one. A confined process could
  previously make its own workspace unlistable with one `mkfifo`.

## [0.7.14] — 2026-10-14

Published by [release run 38597672533](https://github.com/beyond10x/substrate/actions/runs/38597672533)
from tagged commit `e74d6647e4f0cefc81292a1e9d0a723cefadeb8b`. All three artifacts were
keyless-signed and verified against `release.yml@refs/tags/0.7.14` by the release workflow, and
each tag resolved anonymously to its released digest:

- Image: `ghcr.io/beyond10x/b10x-substrate-daemon:0.7.14` at `sha256:79f93a03f80e7e03a2d412ad3db1ce447a5ec30748be1ccc64940171a9ad2c62`.
- Contract bundle: `ghcr.io/beyond10x/b10x-substrate-wire:0.18.0` at `sha256:0fc268003bcb02645759cb916f0790359598943c07fa3647320821f2375450ae`, annotated `development`, the byte-identical bundle 0.7.13 published, signed again with this release's identity.
- Disposable MCP image: `ghcr.io/beyond10x/b10x-substrate-mcp:0.7.14` at `sha256:80c2c2679a9cc228bd438da312a0d9a4c7a49454a4d902761acdd8384e94422e`, development-only.

The [release notes](https://github.com/beyond10x/substrate/releases/tag/0.7.14) provide exact
signature verification commands and the successful 106-case delegated confinement record.

### Changed

- Exec admission rechecks the bubblewrap binary by its device, inode, size, nanosecond
  modification time and change time against the startup probe, and reads and hashes it only when
  one of them differs. A differing hash still refuses `exec.capability-stale`, metadata that cannot
  be read refuses the recheck, and a binary changed within two seconds before the probe is hashed
  at every recheck. The startup probe still hashes the whole binary.
- The event store no longer keeps `events_subject_sequence`, an index on the same columns as the
  `events` primary key; a new store does not create it and an existing store drops it on open.
- `scripts/delegated-lane.sh` takes the daemon binary's path from cargo's own build output, so the
  lane runs wherever cargo places its build directory.

### Fixed

- A workspace line patch applies its edits in line order, so every order of the same
  non-overlapping edit list writes the same bytes. An edit list that was not ascending could land
  edits on the wrong lines, or fail with `host.blocking-failed` from an out-of-range splice.
- Design documents 10 and 10a cite the sandbox argv, the egress refusal and the spawn barrier at
  the functions and lines where they are.

### Security

- `rustls` moves from 0.23.45 to 0.23.46, which fixes RUSTSEC-2026-0346, RUSTSEC-2026-0347,
  RUSTSEC-2026-0348 and RUSTSEC-2026-0349.

## [0.7.13] — 2026-10-11

Published by [release run 38280358137](https://github.com/beyond10x/substrate/actions/runs/38280358137)
from tagged commit `44e2505e31d32733fc997b84f76eeb42cce1dfc6`. All three artifacts were
keyless-signed and verified against `release.yml@refs/tags/0.7.13` by the release workflow, and
each tag resolved anonymously to its released digest:

- Image: `ghcr.io/beyond10x/b10x-substrate-daemon:0.7.13` at `sha256:0de7bbdee8dd44b10ccbc3715b15b5551603033e566b29db9331f9f889932be0`.
- Contract bundle: `ghcr.io/beyond10x/b10x-substrate-wire:0.18.0` at `sha256:0fc268003bcb02645759cb916f0790359598943c07fa3647320821f2375450ae`, annotated `development`, published for the first time.
- Disposable MCP image: `ghcr.io/beyond10x/b10x-substrate-mcp:0.7.13` at `sha256:94de49281fe1916de21ff510270c53fc51d2d1193d04138fbba0ecf600c98f7b`, development-only.

The [release notes](https://github.com/beyond10x/substrate/releases/tag/0.7.13) provide exact
signature verification commands and the successful 106-case delegated confinement record.

### Added

- Development contract bundle `substrate-wire/0.18.0` (ADR 0032), preserving every earlier bundle's
  bytes. `exec.resource-usage.block_io` is a published boolean, and `io_read_bytes` and
  `io_write_bytes` are optional in a usage record. The daemon and the Rust SDK advertise
  `substrate-wire/0.18.0` with its inner manifest digest.
- A single host file can be admitted read-only (ADR 0033). The optional `read_only_files` start
  field, beside `read_only_roots`, binds each declared regular file read-only at its mount through
  a descriptor pinned at admission, which bubblewrap mounts and closes before the program starts;
  none of its host siblings becomes visible. Up to 16 files are
  admitted, the bound the new `exec.read-only-files` capability fact publishes. The applied
  confinement observation lists every bound file. A list over the bound, a non-canonical path (one
  carrying a NUL, over 4000 bytes, with a component over 255 bytes or over 64 components deep), a
  mount that collides with an owned mount, a declared root or another file, a duplicate mount, an
  absent path, a symlink in any component and a non-regular file are each refused by name before
  dispatch; a host that has not proved the mechanism refuses `exec.read-only-files-unserved`. The
  Rust SDK gains `read_only_file` on its exec and session builders. Bundle `0.18.0`, not yet
  released, carries the field, fact, observation and refusal set, and its advertised digest moves
  accordingly. The runtime vectors grow to 76 portable and 106 delegated cases.

### Changed

- A host whose delegation root lacks the `io` controller, such as a systemd-delegated user scope,
  now publishes `exec.resource-usage` with `block_io: false` and serves execution usage instead of
  refusing it `exec.metrics-unserved`. Its usage records and metrics samples omit both block I/O
  counters rather than reporting zero. `ResourceUsage::io_read_bytes` and `io_write_bytes` are now
  `Option<u64>`; a consumer that required them must tolerate their absence.
- The executable specification is pinned to ESS 0.59.0. The committed suite is regenerated in the
  `ess-conformance/34` format, and the store's conformance runner resolves the instance values of
  its `expect_event_values` steps.

### Fixed

- The MCP stdio journey checks the usage record's `wall_time_us`, the field it has always carried,
  instead of a `wall_ms` field it never had; the check ran for the first time on a host without `io`.
- A `read_only_roots` host path or mount carrying a NUL byte is refused by the lexical check as
  `exec.read-only-root-invalid`, rather than by whatever filesystem or spawn step met it later.

### Security

- A read-only or scoped workspace (ADR 0023) no longer hands its pinned root descriptor to the
  confined program. Bubblewrap was given `/proc/self/fd/<n>` as a path, so the descriptor stayed
  open in the program, which could reopen it for writing on the host's writable mount and write
  anywhere in a workspace it was granted read-only, or outside its writable subtrees. This was
  present in every release since `0.4.0`. The workspace descriptors are now bound with
  `--ro-bind-fd` and `--bind-fd`, which close them before the program starts, and the scoped-write
  probe withholds `exec.workspace-scoped-write` unless it finds no such descriptor in the program.
- A secret slot declared at a driver-owned descriptor number could replace the seccomp program or
  the launch barrier (or, with an egress aperture, the info pipe) that bubblewrap consumes: a slot
  on the seccomp descriptor was installed as the filter, so the program ran without Substrate's
  profile, and a slot on the barrier released bubblewrap before the cgroup attach. Driver
  descriptors now sit above the slot range, close-on-exec in the daemon and inherited only by the
  one bubblewrap child, and a start in which any of them lies in the slot range is refused before
  the spawn.

## [0.7.12] — 2026-10-10

Published by [release run 38003936799](https://github.com/beyond10x/substrate/actions/runs/38003936799)
from tagged commit `7d268e59262aaa03bb836d9605a77a887b010816`. All three artifacts were
keyless-signed and verified against `release.yml@refs/tags/0.7.12` by the release workflow, and
each tag resolved anonymously to its released digest:

- Image: `ghcr.io/beyond10x/b10x-substrate-daemon:0.7.12` at `sha256:e7f34d46c4a57f14bf3b4043ba48b752cbab106aab983188dfff7e9f3676fb9b`.
- Contract bundle: `ghcr.io/beyond10x/b10x-substrate-wire:0.17.0` at `sha256:90f047b346a9f35460644521775637dc046228159a426608f6ec5cb71292d8ac`, annotated `development`, byte-identical to 0.7.11's.
- Disposable MCP image: `ghcr.io/beyond10x/b10x-substrate-mcp:0.7.12` at `sha256:d82bc5eb62567b383d482e871cd7759d2aa2843d1bc88b3a1d0fd518de3ccd67`, development-only.

The [release notes](https://github.com/beyond10x/substrate/releases/tag/0.7.12) provide exact
signature verification commands and the successful 97-case delegated confinement record.

### Added

- Running Git in a linked worktree: declaring the repository's Git common directory as a read-only root at its own host path lets `git status` and `git log` run inside an exec, while writes to the common directory stay refused; documented under *Confinement and refusal*, proved by delegated host cases.
- The exec CPU ceiling is configurable: `HostConfig::exec_cpu_cores` and the daemon's `--exec-cpu-cores N` (default 1, today's one-core clamp) cap an exec's derived `cpu.max` at N cores; 0 or more than the usable CPUs is refused at startup as `config.exec-cpu-cores-invalid`.

### Changed

- Egress-aperture run state no longer goes into the workspace root: `HostConfig::aperture_root` (default `<workspace_root>/.substrate-apertures`) is created only when an aperture is declared, and the daemon keeps it beside its state database as `<state>.apertures`.

## [0.7.11] — 2026-10-08

Published by [release run 37821762900](https://github.com/beyond10x/substrate/actions/runs/37821762900)
from tagged commit `7772fd7867b86883d1fd60767e2f07e4dd30324b`. All three artifacts were
keyless-signed and verified against `release.yml@refs/tags/0.7.11` and anonymously retrieved
before announcement:

- Image: `ghcr.io/beyond10x/b10x-substrate-daemon:0.7.11` at `sha256:09e322c2510be8f612a32995d133c245984f20eb43139a6031593439061ce2ad`.
- Contract bundle: `ghcr.io/beyond10x/b10x-substrate-wire:0.17.0` at `sha256:90f047b346a9f35460644521775637dc046228159a426608f6ec5cb71292d8ac`, annotated `development`, byte-identical to 0.7.10's.
- Disposable MCP image: `ghcr.io/beyond10x/b10x-substrate-mcp:0.7.11` at `sha256:5c020f033450821147d9a697badf9525244c9e4d7bbfb14133aaf72061907e01`, development-only.

The [release notes](https://github.com/beyond10x/substrate/releases/tag/0.7.11) provide exact
signature verification commands and the successful 97-case delegated confinement record.

### Added

- The documentation build publishes `.well-known/b10x-routes.json`: every route of the project site
  with its rendered anchor IDs, at the built commit. The organization Website uses it to redirect
  the former `/docs/substrate/` pages to `/substrate/docs/`.

## [0.7.10] — 2026-10-03

Published by [release recovery run 37077554387](https://github.com/beyond10x/substrate/actions/runs/37077554387)
from tagged commit `65304edf6ebdf4a95f9c2c6138b0c20ea47d157e`. All three artifacts were
keyless-signed and verified against `release.yml@refs/heads/main`, the protected-main recovery
workflow identity, and anonymously retrieved before announcement:

- Image: `ghcr.io/beyond10x/b10x-substrate-daemon:0.7.10` at `sha256:90469e101c828c7e88fbf1e98c63ec82b9b010e022cbe8432bfc0619c38d1511`.
- Contract bundle: `ghcr.io/beyond10x/b10x-substrate-wire:0.17.0` at `sha256:90f047b346a9f35460644521775637dc046228159a426608f6ec5cb71292d8ac`, annotated `development`.
- Disposable MCP image: `ghcr.io/beyond10x/b10x-substrate-mcp:0.7.10` at `sha256:d84ac8303aa465653ad9caddc18400c06b11b364b2a30b5780c9fb9fc80c0696`, development-only.

The [release notes](https://github.com/beyond10x/substrate/releases/tag/0.7.10) provide exact
signature verification commands and the successful 97-case delegated confinement record.
The initial publication attempt encountered a GitHub OIDC token timeout; recovery retained the
same immutable tag and source revision.

### Fixed

- Live PTY and pipe output now waits up to one second for a bounded queue slot, so an
  active consumer can drain a terminal redraw burst without immediate cancellation.
  A stalled consumer still receives `session.output-backpressure`; queue capacity,
  byte ceilings, cancellation cleanup and unrecorded capture guarantees are unchanged.

## [0.7.9] — 2026-10-02

Published by [release run 37053941591](https://github.com/beyond10x/substrate/actions/runs/37053941591).
All three artifacts were keyless-signed, verified against the exact tagged workflow identity,
and anonymously retrieved before announcement:

- Image: `ghcr.io/beyond10x/b10x-substrate-daemon:0.7.9` at `sha256:42246a57457c9b548d0270f51d78c9fd4f9c4d8ba81ad33c99d19ddffa99bf6a`.
- Contract bundle: `ghcr.io/beyond10x/b10x-substrate-wire:0.17.0` at `sha256:90f047b346a9f35460644521775637dc046228159a426608f6ec5cb71292d8ac`, annotated `development`.
- Disposable MCP image: `ghcr.io/beyond10x/b10x-substrate-mcp:0.7.9` at `sha256:c1f5cb19369ea32e38ab87e0e844f9de7eacf1758292df52397b0fc82b842682`, development-only.

The [release notes](https://github.com/beyond10x/substrate/releases/tag/0.7.9) provide exact
`cosign verify` commands and the delegated confinement record for the tagged commit.

### Added

- Policy-controlled unrecorded PTY and pipe sessions through the public Rust SDK.
  Recording remains the default; deployment policy must explicitly admit unrecorded
  capture. Live output retains finite byte, frame, queue and attachment bounds.
- Content-free byte accounting, named capture refusals and explicit no-replay output
  queries. The store rejects unrecorded payload writes and capture-mode changes before SQL.
- Development contract bundle `substrate-wire/0.17.0`, preserving earlier bundle bytes.
- A standalone project documentation site, AEP v5 planning store and initial ESS ledger
  and capture-selection projections.

### Verification

- Production canaries cover PTY and pipes through success, child failure, cancellation,
  expiry, output/backpressure limits, disconnect, persistence failure and restart. Live
  database, WAL and diagnostic-log scans include recording-enabled controls.
- Live resource-measurement assertions require a host advertising the complete counter
  capability; the development workstation lacks delegated `io.stat`. See the
  [implementation evidence](.engineering/reports/issue-112-unrecorded-streaming.md).

## [0.7.8] — 2026-09-25

### Security

- `rustls` is 0.23.45, taking the fix for
  [RUSTSEC-2026-0285](https://rustsec.org/advisories/RUSTSEC-2026-0285.html); `THIRD_PARTY_LICENSES.html`
  is regenerated for that revision.

### Added

- A release requires a **recorded delegated confinement-lane run for the tagged commit**.
  `release.yml` reads a `Confinement-lane: <source-sha> <case-count> <RFC 3339 UTC>` line from the
  annotated tag's own message, or from the `confinement_evidence` recovery-dispatch input, and
  refuses a record that names another commit, a case count other than the `DELEGATED_CASES` the
  tagged source declares, or a timestamp that is not an RFC 3339 UTC instant. It records a local
  run; it does not execute the lane in CI.
- [ADR 0006](adr/0006-substrate-publishes-its-own-contract-bundle.md) records why Substrate
  publishes and keyless-signs its own development contract bundle.
- `cargo xtask quota-lane` runs the seven `#[ignore]`d `real_quota_*` cases in
  `crates/substrate-host/src/git/quota_tests.rs`, selecting each with `--ignored --exact` and
  requiring the harness's own `1 passed` for it. With `SUBSTRATE_TEST_QUOTA_ROOT` or
  `SUBSTRATE_TEST_PROJECT_QUOTA_IDS` missing, malformed, offering fewer than 128 identities, or
  backed by a mount carrying neither `prjquota` nor `pquota`, it **refuses by name and exits 1**,
  listing each missing prerequisite and every case that did not run. `--inventory` prints that
  statement and runs nothing.
- `cargo xtask check-packages` asserts the two dependency invariants — no Flux crate and no
  dependency resolved from a `beyond10x` Git repository — across every dependency table of every
  workspace manifest, with refusal tests.

### Changed

- `AGENTS.md`, `README.md`, `ROADMAP.md` and `STATUS.md` describe 0.7.7, the repository rules that
  exist — including ruleset `main-requires-the-full-gate` (id 23425552), which requires the `Full
  gate` check on `main` — and the embedded seam.
- `README.md` § *Status* points at
  [ADR 0006](adr/0006-substrate-publishes-its-own-contract-bundle.md) § *Addendum* for why bytes
  annotated `development` are signed, digest-pinned and write-once at all.

### Fixed

- `b10x-substrate-host` compiles for `aarch64-unknown-linux-gnu`
  ([#109](https://github.com/beyond10x/substrate/issues/109)). The loopback readiness probe in
  `crates/substrate-host/src/egress.rs` wrote the interface name into `ifreq::ifr_name` with
  `u8::cast_signed`, which assumes `c_char` is `i8`; on aarch64 Linux it is `u8`. The bytes are now
  converted with `c_char::from_ne_bytes`, which holds for either signedness.

- The real project-quota cases had no owner: neither `scripts/gate.sh` nor
  `scripts/delegated-lane.sh` passed `--ignored`, no workflow or `xtask` verb did either, and they
  are outside `DELEGATED_CASES`, so the release confinement-lane record never covered them.
  Provisioning the filesystem would have changed nothing, because no script would have run them
  even then — the only recorded execution is a hand-typed `docker exec` against a quota-lab image
  on 2026-09-05. `cargo xtask quota-lane` is that invocation made repeatable, and
  `scripts/delegated-lane.sh` now closes with `cargo xtask quota-lane --inventory`, which names the
  cases it did not run so a green delegated lane cannot be read as covering them.
- `scripts/delegated-lane.sh` resolves the shipped daemon under `CARGO_TARGET_DIR` rather than a
  hardcoded `${PWD}/target/debug/substrate-daemon`, and refuses by name when the build left no
  executable there. With an external build directory set — which a machine that shares one build
  directory across a repository's worktrees does — the two `b10x-substrate-sdk` delegated cases
  failed with `Startup("No such file or directory (os error 2)")`, which reads as a daemon defect
  rather than a missing build artefact.

## [0.7.7] — 2026-09-10

### Fixed

- Leave Git metadata untouched during host startup, Git-only observations and workspace
  destruction when no Git sources are configured.

### Changed

- The documentation website consumes `@beyond10x/docs-system` at the exact revision
  `86cd6c6efd02184c51a37e80012ffe9d3f77d40a`, release 0.7.0, in `website/package.json` and
  `website/package-lock.json`. The published documentation bundle and Pages delivery workflows keep
  their own catalog-owned pins and are unchanged.

## [0.7.6] — 2026-09-06

Published by [release run 34051681656](https://github.com/beyond10x/substrate/actions/runs/34051681656).
The workflow verified keyless signatures before announcement:

- daemon: `sha256:2ffe9021c9f498cda8d08e5b7438f0e3ca2bc371bdb1bf467e18ab7403073170`
- disposable MCP: `sha256:61b08d32b1c2e365c7466113c87334ae99b269716f3a469626134abf1ea580a2`
- development wire bundle 0.16.0: `sha256:4c4e57a1b2427cb004a05cb475c1193e979777c5c79d9a9505ba5facbe10daf7`

### Added

- Opt-in container execution bootstrap with a private cgroup delegation, protected enclosing
  limits, permanent non-root transition and an init process that reaps sandbox descendants.
- Versioned AppArmor and seccomp profiles with a separate immutable node-profile installer.
- Packaged non-setuid bubblewrap and shell tools for the existing confined host execution floor,
  plus a public-SDK container check for file bytes, PTY input/resize, observed resource usage and
  complete process-tree cleanup. Ordinary and quota daemon entrypoints remain available.

## [0.7.5] — 2026-09-05

Published by [release run 33994751338](https://github.com/beyond10x/substrate/actions/runs/33994751338).
The workflow verified keyless signatures before announcement:

- daemon: `sha256:e69032ec391a7e2cea57ff46465e2c2af2b413795174a173ac8c6d76d4f11e0c`
- disposable MCP: `sha256:ba0bfd9ea73abb3111dc95fd5a6ff998723db5a19e14c581e783de080431cd5f`
- development wire bundle 0.16.0: `sha256:4c4e57a1b2427cb004a05cb475c1193e979777c5c79d9a9505ba5facbe10daf7`

### Fixed

- Return and persist fresh kernel storage usage from workspace observations. Preserve admitted
  quota limits, lifecycle, labels and lease authority, and reject older usage observations.

## [0.7.4] — 2026-09-05

Published by [release run 33991484686](https://github.com/beyond10x/substrate/actions/runs/33991484686).
The workflow verified keyless signatures before announcement:

- daemon: `sha256:7f3213a766f3b734a306e374cbcfb37b48a4b577348d9a3142c9c21bed76308f`
- disposable MCP: `sha256:72d160135447d965b0abf6ee5632c8b85fb3a390168f2a1c9abb77d0d06b4a07`
- development wire bundle 0.16.0: `sha256:4c4e57a1b2427cb004a05cb475c1193e979777c5c79d9a9505ba5facbe10daf7`

### Fixed

- Attach hard byte and inode quotas before Git initialization or network materialization. Keep
  quota identity and allocator ownership through atomic installation, report kernel storage usage,
  and release allocations only after failed, cancelled or destroyed workspaces are absent with
  zero accounted usage. Enforced-filesystem regressions cover quota exhaustion, concurrent
  workspaces, restart recovery, installation conflicts and identity reuse.
- Provide an explicit, byte-identical `substrate-daemon-quota` image executable with only
  `cap_sys_admin=ep` so non-root quota startup retains the delegated permission. The ordinary
  entrypoint remains unprivileged. Release validation checks final-image metadata and the actual
  daemon and worker capability sets before publication or recovery reuse.

### Changed

- Connect the public handbook around one workspace and bounded command, with subsystem diagrams,
  explicit model/event coverage and contract derivation, corrected Git and release availability,
  and runnable examples with checked responses, distinct operation IDs and bounded polling.

## [0.7.3] — 2026-09-05

### Changed

- Require Git Smart HTTP protocol v2 for Connector-authorized materialization, request only the
  selected branch and exact commit, and retain the configured 50-commit shallow history. Discovery,
  ref lookup and fetch reuse one HTTPS connection without persisting source credentials.
- Combine durable file synchronization and storage accounting in one bounded traversal before
  atomic workspace installation.
- Reuse bounded HTTPS connections in the Rust SDK. The additive `RemoteEndpoint` factory shares
  TLS configuration across callers while obtaining and verifying each request's current authority;
  terminal exporter binding and the development wire bundle remain unchanged.

## [0.7.2] — 2026-09-05

### Fixed

- Ship the zlib runtime library required by the exact daemon and disposable-MCP binaries in both
  distroless image targets. The release image now starts instead of failing at the dynamic loader
  with a missing `libz.so.1`.

## [0.7.1] — 2026-09-05

### Added

- Configured, segment-bounded HTTPS Git sources now materialize one normal working tree at the
  provider-selected branch's exact commit. Connector authority is transient and redacted; checkout
  is bounded, fsynced and atomically installed; `.git` remains usable by confined terminals but is
  hidden from file and tree APIs.
- The Rust SDK now exposes bounded baseline-file and path-sorted change-set observations against a
  host-private immutable materialization commit.
- Development contract bundle `0.16.0` succeeds `0.15.0` additively with two Git observation routes,
  the closed Git source request shape and conditional `workspace.git` capability, at inner
  `bundle.json` SHA-256 `cee5845cf425885bdae3be6f59cb9e39ce342df065a01ae65eaae24ad2f29b41`.

## [0.7.0] — 2026-09-04

Daemon image: `ghcr.io/beyond10x/b10x-substrate-daemon:0.7.0` at
`sha256:62ad44f814a4cae4f95b826aaad69f5205f1d16a46e98700b96efe1b3a8a5f51`, keyless-signed;
verify with `cosign verify`.

Disposable MCP image: `ghcr.io/beyond10x/b10x-substrate-mcp:0.7.0` at
`sha256:56265329568f4f3eebb0559f35dc61b6fa6501d0facf626362fac6573989797a`, development-only and
keyless-signed; verify with `cosign verify`.

Contract bundle: `ghcr.io/beyond10x/b10x-substrate-wire:0.15.0` at
`sha256:ba95171e3a05d7917e4083759107132ad6fb707003e791e15b47d9fb20424ac8`, annotated
`development`, keyless-signed; verify with `cosign verify`. Publication, digest pinning and a
verified signature do not make the development contract stable. **The digest is the one `0.5.0` and
`0.6.0` published**: two waves of confinement work changed no byte of a released bundle, which is
invariant 6 holding, and the workflow re-signed the identical layout with this release's own
workflow identity rather than replacing the tag.

Second security wave over the 2026-09-03 whole-repository review. Every remaining severity D and H
finding is closed, and two live sandbox escapes were found and closed along the way.

### Security

- **A confined process can no longer reach another sandbox or the host through an unconfined socket
  family.** The four-family shortlist is replaced by a survey of all 46 families across seven socket
  types, measured from inside a real admitted exec, asking two questions of each: can data cross to
  a mutually-isolated sibling, and does opening it mutate host-global state. The denied set is
  three to ten — `AF_UNIX`, `AF_VSOCK`, `AF_QIPCRTR`, `AF_ALG`, `AF_RDS`, `AF_PPPOX`, `AF_KCM`,
  `AF_SMC`, `AF_MCTP`, `AF_PACKET`.
  - `AF_VSOCK` was review finding 7, filed *inferred, not tested*. It was measured: a socket opened
    inside a confined exec with the network namespace holding, and reached `connect()`.
  - `AF_QIPCRTR` carried a datagram between two mutually-isolated sandboxes, each with a fresh
    network and user namespace, while `AF_UNIX`, `AF_INET` and `AF_INET6` all failed across the same
    boundary.
  - `AF_ALG` let a confined process name an algorithm and have the host load the implementation into
    its single global module table, where it persists after the sandbox exits and a sibling sandbox
    observes it through the non-namespaced `/proc/modules`.
  - `AF_INET`, `AF_INET6` and `AF_NETLINK` autoload too and are recorded as accepted residuals:
    refusing them removes the egress aperture and name resolution.
- **An exec that crosses its memory bound leaves no process in its cgroup.** The exec cgroup set
  `pids.max`, `memory.max`, `memory.swap.max` and `cpu.max` but not `memory.oom.group`, so an OOM
  killed one process and the rest of the tree continued (review finding 8).
- **An upgraded WebSocket stays inside the transport connection budget.** The per-uid and global
  permit was released the moment an upgrade succeeded, so the socket ran in a task the budget no
  longer counted, at all three listeners (review finding 4).

### Changed

- Workspace lease cleanup reads exec ids and states without their output blobs. It previously loaded
  every exec row of the workspace including `stdout` and `stderr` — 2048 execs at 2 MiB each is
  4 GiB in one sweep (review finding 5).
- A transport slot is now held for an upgraded stream's lifetime — one hour — rather than the
  transport's five minutes. This is the intent of finding 4's fix and it is recorded rather than
  hidden; `story:transport-admission-and-stream-lifetime-disagree` carries the design question it
  opens.
- The seccomp jump chain is generated from the family table rather than hand-written, because an
  off-by-one in hand-counted BPF offsets does not fail closed.
- `docs/design/08-phase-3-closure-invariants.md` no longer states that the local HTTP transport
  disables keep-alive. It has set it true since `a9f234b`, with no record of the change.

## [0.6.0] — 2026-09-04

Daemon image: `ghcr.io/beyond10x/b10x-substrate-daemon:0.6.0` at
`sha256:55a080c8b65c977ba6031d15ad7a336f49b4175266ea40b8bf10151ad822a711`, keyless-signed;
verify with `cosign verify`.

Disposable MCP image: `ghcr.io/beyond10x/b10x-substrate-mcp:0.6.0` at
`sha256:c7a189a389d5760780f3329a0b3d8470e5eaf1247cd6fc6ecfdad170e10c0fbc`, development-only and
keyless-signed; verify with `cosign verify`.

Contract bundle: `ghcr.io/beyond10x/b10x-substrate-wire:0.15.0` at
`sha256:ba95171e3a05d7917e4083759107132ad6fb707003e791e15b47d9fb20424ac8`, annotated
`development`, keyless-signed; verify with `cosign verify`. Publication, digest pinning and a
verified signature do not make the development contract stable. **The digest is the one `0.5.0`
published**: this release changed no byte of a released bundle, which is invariant 6 holding, and
the workflow re-signed the identical layout with this release's own workflow identity rather than
replacing the tag.

Security wave over the 2026-09-03 whole-repository review. Three review findings closed, and three
pre-existing defects on the same routes that no story had named.

`0.5.1` was prepared and never tagged; its change is listed below and ships inside this release.

### Security

- Confined processes can no longer nest user namespaces. Every bubblewrap argv the host builds
  carries `--disable-userns`, spliced from one constant and asserted on the argv as built rather
  than on the source text. The probe additionally passes `--assert-userns-disabled`, so a
  bubblewrap that cannot prove a non-nestable user namespace leaves the exec facts **absent**
  rather than serving a quieter sandbox.
- `GET /v1/metrics/stream` carries a permit, a lifetime and frame bounds. It was previously
  unbounded: one authenticated uid could open as many streams as the kernel allowed, each writing
  to SQLite once a second for up to the 24 h exec timeout.
- A claimed session whose WebSocket upgrade never completes is terminated rather than left running
  unattached until its lease expires. An abortive client close reaches this path; an ordinary close
  was already handled.
- An attachment permit that cannot prove its containment kill no longer forgets a global slot for
  the life of the process. Thirty-two such disconnects previously left the daemon serving no
  attachments at all while reporting a retriable `exhausted` capacity.

### Changed

- Metrics streams are capped at 4 per subject and 64 per deployment; one more is refused `429` with
  `metrics.stream-capacity`. A client control-frame flood is closed `1008` and a data frame `1003`,
  matching the event stream. Two bounds still end the connection without naming themselves — the
  1 h lifetime and an oversized client frame — and the deployment guide now says so.
- The metrics stream samples once per second, as its released capability predicate
  `sample_interval_ms: 1000` and both guides have always advertised. It previously delivered a
  sample every two seconds when the client was silent, and every second when it was not.

### Fixed

- Atomic workspace file replacements preserve whether an existing regular file is executable,
  preventing editor saves and guarded patches from silently stripping executable mode. *(prepared
  as 0.5.1)*

## [0.5.1] — 2026-09-03

### Changed

- Atomic workspace file replacements preserve whether an existing regular file is executable,
  preventing editor saves and guarded patches from silently stripping executable mode.

## [0.5.0] — 2026-09-01

Daemon image: `ghcr.io/beyond10x/b10x-substrate-daemon:0.5.0` at
`sha256:5dc8a1a6b61c9b652817c0ae54a4504c23bf781a6fed3cb7617e535bf7c9e786`, keyless-signed;
verify with `cosign verify`.

Disposable MCP image: `ghcr.io/beyond10x/b10x-substrate-mcp:0.5.0` at
`sha256:3fc28533df606b1db8d5583c3f4288551393ecf15c293c7815bfe8f599976316`, development-only and
keyless-signed; verify with `cosign verify`.

Contract bundle: `ghcr.io/beyond10x/b10x-substrate-wire:0.15.0` at
`sha256:ba95171e3a05d7917e4083759107132ad6fb707003e791e15b47d9fb20424ac8`, annotated
`development`, keyless-signed; verify with `cosign verify`. Publication, digest pinning and a
verified signature do not make the development contract stable.

### Added

- **Hosted raw-pipe attachment authority is one-use and TLS-channel-bound.** A scoped hosted client
  mints at most four 60-second authorities for a ready session, binds each to an Ed25519 public key,
  and proves the accepting TLS 1.3 exporter in the WebSocket upgrade. Substrate stores only the
  bearer verifier and atomically redeems it with the single attachment claim; absent, expired,
  replayed and unbound attempts are named refusals. Unix attachments retain kernel-credential
  authority, while development TCP serves neither start, mint nor attach.
- **Production HTTPS/WSS requests use current Identity authority.** The TLS listener resolves one
  five-minute opaque access credential over direct, explicitly rooted HTTPS for exact audience
  `urn:b10x:substrate`, validates the closed authority document, derives a tenant-bound subject and
  enforces `observe`, `workspaces` or `exec` before handler dispatch. Missing, invalid,
  under-scoped and unavailable authority fail closed by four named refusals; revocation applies on
  the next request because no authority is cached.
- **A disposable local MCP test adapter drives Substrate through the public SDK.** The private
  `substrate-mcp` binary owns one fresh linked daemon, exposes a bounded closed stdio tool/resource
  surface, preserves caller operation ids and exact refusals, and performs ordered exec/workspace/
  daemon cleanup. Portable, delegated and real Codex clean-room journeys prove the surface; a new
  repository gate keeps the crate private, SDK-only and free of remote or unbounded MCP transports.
- **The public Rust SDK covers the promoted development contract.** It now exposes guarded v2 file
  operations, bounded directory/tree and exec-output pages, reconciliation snapshots, PTY start and
  resize, metrics pull and stream, complete session observations, exact optional capability facts,
  serializable observations, public request bounds and caller-supplied operation ids for every
  mutation.
- **The Rust SDK connects to production HTTPS/WSS without ambient trust.** Remote mode requires an
  exact HTTPS origin, a bounded PEM root bundle, an expected DNS identity and an asynchronous
  short-lived Identity token provider. HTTP, event and metrics streams, and proof-bound session
  attachments share TLS 1.3 and exact contract verification. One named authentication failure can
  refresh authority without replacing an operation id; ambiguous mutation recovery retains the
  original request bytes, and every session reconnect mints a fresh one-use channel authority.

### Changed

- **Rust crates are source-distributed and cannot be published accidentally.** All workspace
  packages now set `publish = false`; `cargo xtask check-packages` refuses any publishable member
  while retaining fixed runtime package names, exact internal edges, SPDX metadata, READMEs and
  public documentation targets. Consumers use a path or exact Git revision. Releases remain signed
  GitHub/GHCR artifacts and require no crates.io credential or docs.rs claim (ADR 0030).
- **Session HTTP routes are now `/v1/sessions`, with no legacy alias.** The daemon and Rust SDK
  claim development contract `substrate-wire/0.15.0` and inner `bundle.json` digest
  `c0a6f82601debdca988f6c3cf93b89ebb7d086b8c9f74b4b7c9fb17d664357b3` together. Its direct
  predecessor is 0.14.0; it replaces exactly eight `/v1/pipe-sessions` route addresses, preserves
  all 26 non-session addresses and every operation id, and retains the hosted attachment-authority
  semantics added in 0.14.0. This is a deliberate pre-1.0 break. Its separately signed OCI
  distribution is 0.15.0 at
  `sha256:ba95171e3a05d7917e4083759107132ad6fb707003e791e15b47d9fb20424ac8`. The clean-room shipped-
  binary lane checks the pair on every HTTP response and WebSocket route probe, while SDK fixtures
  reject missing, older, unknown and wrong-digest claims before serving an operation. The bundle
  remains development, not stable.

### Fixed

- **Orderly daemon shutdown quiesces every owned process tree before the runtime exits.** Unix and
  TCP connection tasks are owned and drained, the driver shutdown contract is mandatory, and the
  delegated SDK lane proves that both an active PTY workload and its exact exec cgroup are absent
  before managed-daemon shutdown returns. Kill or wait errors retain the managed child and temporary
  root for retry or drop cleanup.
- WebSocket upgrade refusals retain their typed refusal body and contract headers instead of
  collapsing into a transport string.

- **Release recovery inspects the remote OCI manifest, not its descriptor.** Registry descriptors
  carry digest, media type and size while the required development/version annotations live in the
  manifest content. A failed eligible tag can be resumed explicitly from the protected-main
  workflow without moving the tag or publishing from an unreviewed workflow ref.
- Corrected the `0.4.0` entry's frozen `0.12.0` lineage: its compatibility block names `0.10.0`,
  preserves 31 routes and adds two. The gate now additionally proves that it preserves the complete
  `0.11.0` route declarations and retains that bundle's quota and metrics behavior before the
  daemon advertises it (`atlas/architecture/adr/0019-substrate-contract-release-and-conformance.md`).

## [0.4.2] — 2026-09-01

Image: `ghcr.io/beyond10x/b10x-substrate-daemon:0.4.2` at
`sha256:1aac0c63c1f1e7dae2dff8f1f20a06b4d7f5461b61bb172b4a8a3f137cd2f6d1`, keyless-signed;
verify with `cosign verify`.

Contract bundle: `ghcr.io/beyond10x/b10x-substrate-wire:0.12.0` at
`sha256:dd901e848c821aca7d55f7b8cf5ee893e1d99a1428b348e32e7ed1045a375319`, annotated
`development`, keyless-signed; verify with `cosign verify`. Publication, digest pinning and a
verified signature do not make the development contract stable.

### Added

- **Production network control is server-authenticated TLS.** ADR 0024 fixes a distinct TLS 1.3
  HTTPS/WSS listener, explicit owner-safe key material, atomic certificate rotation, no forwarded-
  address trust and no client-certificate principal mapping. Hosted request identity remains a
  separate coordinated trust-envelope decision.

### Fixed

- **Contract-bundle publication proves the consumer path.** GHCR login uses the Actions actor, and
  post-publish bundle resolution runs without credentials with a bounded visibility-propagation
  retry. This lets a release reuse the exact write-once `0.12.0` bundle after proving its public
  digest instead of depending on the publisher credential's read behavior.

## [0.4.1] — 2026-09-01 (unpublished)

The annotated tag reached bundle upload, but its workflow refused before signing, daemon-image
publication or GitHub release announcement. The tag and the uploaded write-once bundle are retained;
`0.4.2` is the recovery release and never moves or overwrites either one.

### Added

- **Write-once OCI publication for the current development contract bundle.** An eligible release
  packages the explicitly pinned bundle with `cargo xtask package-bundle`, copies that exact
  manifest to `ghcr.io/beyond10x/b10x-substrate-wire:<bundle-version>`, checks the remote digest and
  `development` annotation, then keyless-signs and verifies it before the GitHub release is
  announced. Existing canonical tags are refused before publication and again immediately before
  upload. Protected `main` receives the observed digest through the workflow-emitted bot pull
  request stanza; no bundle digest is claimed before a release actually runs.

## [0.4.0] — 2026-09-01

### Added

- **Exact workspace write authority.** Exec and raw-pipe starts can retain the historical
  whole-workspace read/write mode, make the workspace read-only, or name a canonical set of
  writable subtrees. The driver refuses absence, aliases, overlaps and symlinks, proves the mount
  ordering before advertising it, and records the exact applied mode. The Rust SDK exposes this
  policy together with apertures, scratch, read-only roots, secret slots, capsules and exact
  resource measurements.
- **Contract bundle [`contracts/substrate-wire/0.12.0`](contracts/substrate-wire/0.12.0)**, an
  encoded successor to `0.10.0`, preserving 31 routes and carrying the two metrics routes already
  introduced by `0.11.0`, plus the request, capability and applied-confinement workspace-access
  shapes and an executable scoped-write vector. A separate promotion gate proves the complete
  `0.11.0` inventory and behavior survive the frozen non-adjacent compatibility block.

## [0.3.1] — 2026-09-01

Image: `ghcr.io/beyond10x/b10x-substrate-daemon:0.3.1` at
`sha256:48068ba167487fa84fe77e0acf7f8e3f556060f4d220a074d9b9f60f9af71ebf`, keyless-signed; verify
with `cosign verify`.

### Changed

- Updated `ed25519-dalek` to 3.0.0, `jsonschema` to 0.52.1, `tokio-tungstenite` to 0.30.0 and
  `ulid` to 3.0.0, refreshed every compatible transitive dependency and regenerated the
  deterministic third-party licence inventory. Dependabot now groups future Rust updates into one
  gateable rollup.

## [0.3.0] — 2026-08-31

Image: `ghcr.io/beyond10x/b10x-substrate-daemon:0.3.0` at
`sha256:9217023c2b21837e30d00e5d01cb4ac53095bb32df7b836fa2a514b2931a9902`, keyless-signed; verify
with `cosign verify`.

### Added

- **A public builder-oriented Rust SDK.** `b10x-substrate-sdk` verifies the daemon-advertised
  `substrate-wire/0.4.0` identity, exposes typed workspace, file, exec, raw-pipe, event, refusal and
  operation APIs, requires explicit execution bounds, and reconciles ambiguous mutations through
  the durable ledger under one operation id.
- **Managed daemon ownership without collapsing the process boundary.** The SDK can supervise an
  external binary or opt into a linked current-executable re-exec; both are separate authenticated
  child processes with readiness verification, parent-liveness shutdown, bounded diagnostics,
  graceful termination, force-kill fallback and reaping.
- **A closed Rust registry boundary.** Five `b10x-substrate-*` runtime packages carry exact 0.3.0
  internal edges, package READMEs and Apache-2.0 SPDX metadata. `cargo xtask check-packages` refuses any
  additional publishable workspace member and assembles every approved archive without adding a
  crates.io credential to GitHub.
- **Hard writable-storage ceilings and exact resource observations.** Workspaces and per-exec
  `/scratch` can request byte-and-inode project quotas when the operator delegates an exclusive
  range and the filesystem proves enforcement. Execs explicitly opt into exact live and terminal
  cgroup observations; no mean-memory estimate is manufactured.
- **Contract bundle [`contracts/substrate-wire/0.11.0`](contracts/substrate-wire/0.11.0)**, 260 files,
  an additive successor to `0.10.0`: `adds_routes: 2`, `preserves_routes: 31`. It adds
  `GET /v1/metrics`, a latest-wins WebSocket stream, quota requests, capability facts and usage
  schemas while leaving every earlier bundle byte untouched.
- **Practical public guides.** The Pages site now shows non-agent use cases and a terminal journey
  that runs an ordinary binary with process, memory, CPU, duration, disk, output and metrics bounds.

### Changed

- Updated the locked Rust dependency set to `base64` 0.23.1, `hyper` 1.11.1, `nix` 0.31.3,
  `rusqlite` 0.40.2 and `sha2` 0.11.0, with the deterministic third-party licence inventory
  regenerated from the resulting graph.

## [0.2.4] — 2026-08-31

Image: `ghcr.io/beyond10x/b10x-substrate-daemon:0.2.4` at `sha256:18b4bf966ffab38eb76801204dadfb413e898e5110272514accdf8d6e1ec083b`, keyless-signed; verify with `cosign verify`.

### Changed

- **Substrate is Apache-2.0.** Atlas ADR 0010 grants Apache-2.0 across all beyond10x-owned
  Substrate history without rewriting a frozen contract byte. The repository, public site and
  daemon image carry the licence and public security-reporting path; third-party material retains
  its own licence.
- **The public distribution is fail-closed.** GitHub secret and dependency protections, private
  vulnerability reporting, action SHA pinning, locked audit-clean website dependencies and
  deterministic third-party notices join the existing full-history and RustSec gates.

### Added

- **Contract bundle [`contracts/substrate-wire/0.9.0`](contracts/substrate-wire/0.9.0)**, an
  additive successor to `0.8.0`: `adds_routes: 5`, `preserves_routes: 26`. One operation registry
  now declares every served API major, the five existing v2 workspace byte-plane routes carry v2
  schemas and envelopes, and the preserved v1 file routes declare the catch-all path the daemon
  actually serves. Accepted as [ADR 0018](adr/0018-one-registry-declares-every-served-api-major.md).
- **Confinement and observation hardening.** Declared host roots cannot expose host IPC; delegated
  context is authenticated before replay lookup; reads stay bounded even when metadata is stale;
  aperture setup preserves its failing stage and errno; seccomp denies Unix sockets and
  `io_uring_setup`; aperture state is reconciled fail-closed; and live pipe-output backpressure is
  terminal rather than silently lossy. Accepted as [ADRs 0015–0017](adr/README.md).
- **Repository and release gates are fail-closed.** The gate now scans every reachable Git object
  for secrets, checks RustSec advisories and forbidden HTTP/2 dependencies, keeps bot credential
  validation in Rust, and checks all nine contract bundles. The release workflow tests the exact
  daemon binary extracted from the image before the one permitted push, refuses an existing tag
  image or release, and publishes its GitHub release only after signature verification and an
  anonymous image pull. Docker ignores local build and website dependency trees, reducing the
  measured review build context from 6.49 GB to 17 MB.
- **An egress aperture can carry a declared byte ceiling, and crossing it refuses the run by name.**
  `--egress-aperture <name>=<host>:<port>/tcp[/max=<size>]` bounds one run's
  `to_destination + from_destination` over that aperture; `<size>` is a decimal byte count with an
  optional binary suffix (`1048576`, `512KiB`, `64MiB`, `2GiB`) and never a decimal-power unit, and
  an unrecognised term is a startup error rather than an ignored one. The relay stops relaying at
  the ceiling, so the overshoot is at most one 16 KiB relay buffer per live relay; the parent's
  existing 1 ms supervision loop reads the same counters, ends the run and names the refusal.
  Accepted as [ADR 0014](adr/0014-apertures-carry-a-declared-byte-ceiling.md).
- **An exec observation can name the bound that ended it.** One optional `refusal` member carrying
  `class`, `code` and `message` beside the state that is already `cancelled`. The declared aperture
  byte ceiling — `exhausted` / `exec.aperture-byte-limit` — is its only user; a timeout and a CPU
  budget are unchanged.
- **A ceiling is deployment vocabulary, never request data.** A request carrying one is refused
  `exec.aperture-ceiling-in-request` at `sandbox.network.aperture`, told apart from
  `exec.aperture-destination-in-request` so a rejected escalation does not read as a schema typo.
- **Published and observed.** The `exec.egress-apertures` capability fact and the applied-aperture
  observation each gain an optional `max_bytes`, so `/v1/machine` answers how much this daemon could
  ever pass and a run states the ceiling it actually ran under beside the bytes that crossed.
- **Contract bundle [`contracts/substrate-wire/0.8.0`](contracts/substrate-wire/0.8.0)**, 228 files,
  an additive successor to `0.7.0`: `adds_routes: 0`, `preserves_routes: 26`. Every earlier bundle
  directory keeps its bytes, and `cargo xtask check-bundle 0.8.0` is in
  [`scripts/gate.sh`](scripts/gate.sh).

- **A pty is a second session mode, not a second session resource.** `POST /v1/pipe-sessions` takes
  `mode: "pty"` beside the `pipes` it already served, with a required initial `window` of 1–1000
  cells on each axis. The route family, the operation ids, the `ses_…` identity, the lease, the
  single attachment and the whole-tree cleanup are the ones a raw-pipe session already had; a
  `resize` frame joins the closed client vocabulary, bounded by the same cell rule and rated on the
  control window that already existed. There is no `close-input` on a terminal — a client ends
  input by sending the terminal's own end-of-file character as ordinary input bytes — and no
  `truncated`: reaching the declared output bound ends the session and names
  `session.output-limit` through the refusal field [ADR 0014](adr/0014-apertures-carry-a-declared-byte-ceiling.md)
  added. Decided in [ADR 0019](adr/0019-pty-is-a-second-session-mode.md).
- **`sessions.pty` is published only after a verified allocation.** A startup probe allocates a
  pair, makes it controlling inside a throwaway sandbox *after* bubblewrap's own `setsid`, and
  round-trips a window through the child's `TIOCGWINSZ` before and after a resize. Absent, every
  `mode: "pty"` request is refused `session.pty-unserved` (501, `unserved`) and **never** served as
  pipes; an allocation failure at start is `session.pty-exhausted` (429, `exhausted`, retriable).
  `--new-session` is not dropped to make a terminal work: that would weaken the confinement floor of
  every non-pty exec to serve one feature.
- **Session refusals are closed and actionable.** The bundle register names all 32 refusal codes,
  their arrival surface, class, status and retry fact; protocol-error frames use a closed typed
  vocabulary. The attachment-capacity path now reads the same per-code retry table as the register,
  so its HTTP response cannot contradict the published `retriable: false` decision.
- **Contract bundle [`contracts/substrate-wire/0.10.0`](contracts/substrate-wire/0.10.0)**, 251 files, an
  additive successor to `0.9.0`: `adds_routes: 0`, `preserves_routes: 31`. It adds the `mode` and
  `window` start fields, the `sessions.pty` fact, the served-modes capability document and
  `schemas/pty-channel-frame.json`. Every earlier bundle directory keeps its bytes, and
  `cargo xtask check-bundle 0.10.0` is in [`scripts/gate.sh`](scripts/gate.sh).

### Unchanged

- An aperture declared without the term behaves byte for byte as it did: no ceiling in the relay,
  no `max_bytes` on the fact or the observation, and no `refusal` on the run.
- A session start that names no `mode` is a raw-pipe session, byte for byte as before: the field
  defaults to `pipes`, so no existing client can be handed a terminal, and a `0.4.0` daemon refuses
  `mode: "pty"` as schema-invalid rather than quietly serving pipes.

## [0.2.3] — 2026-08-30

Image: `ghcr.io/beyond10x/b10x-substrate-daemon:0.2.3` at `sha256:ab10158266b579d705ce8422c7d2a6e783cde950d30e100f61ca6befc4d0beda`, keyless-signed; verify with `cosign verify`.

Three accepted ADRs implemented, and the gate's last Python removed. Additive throughout: no route
is removed or changed, and every earlier contract bundle keeps its bytes.

### Added

- **A tagged `main` publishes a signed, digest-pinned daemon image.**
  [`.github/workflows/release.yml`](.github/workflows/release.yml) triggers only on an annotated
  bare-version tag, refuses to build unless [`.github/workflows/gate.yml`](.github/workflows/gate.yml)
  has concluded `success` for that exact commit, builds the `Dockerfile` with `SOURCE_SHA` set to the
  tag's commit, pushes `ghcr.io/beyond10x/b10x-substrate-daemon:<version>`, signs it keylessly and
  runs `cosign verify` against the workflow identity **before** anything is announced. Only then does
  it create the GitHub release and write the digest under that version's heading here. A pre-release
  tag such as `0.2.2-rc.0` publishes nothing. `permissions` are `contents: read` at workflow level,
  with `packages: write` and `id-token: write` on the release job alone; every action is SHA-pinned;
  the release and the changelog commit are made by the b10x-bot App. No image is published yet — the
  workflow has never run. It needs no repository secret: the image push, the GitHub release and the
  changelog commit all use the run's own `GITHUB_TOKEN`, which cannot leave this repository.
  A signed daemon image does not make any wire contract bundle stable; the bundles under
  `contracts/substrate-wire/` remain development bundles.

- **An operation's ledger row carries the declared grant it ran under.** A start may present a
  signed delegated-context document; substrate verifies it and records `grant_ref` and
  `platform_principal` on the operation's ledger row and its `operation.*` events, so a reader can
  answer *which declared grant authorised this, on behalf of which platform principal* from
  substrate's own durable record. Accepted as
  [ADR 0011](adr/0011-delegated-context-and-grant-attribution.md).
- **Caller-written identity is never attribution.** Identity-shaped strings a caller may legitimately
  write — workspace labels are free-form and echoed verbatim — reach the resource and never the two
  attribution columns. Writing one into the request envelope is refused `request.schema-invalid`,
  not ignored quietly: the request union stays closed around `op`, `input` and the one signed
  member. Substrate verifies signature, issuer, exact audience, time window and binding to the
  authenticated subject, and it never evaluates the grant — connectors decides, substrate records.
- **The field is optional everywhere and costs a `0.6.0` client nothing.** `delegated_context` is a
  sibling of `op` and `input`, never a member of `input`, so it stays outside the canonical request
  hash: replaying the same `op` with a *fresh* context is the same operation and returns the
  original outcome. Absent, the serialized bytes are exactly what a `0.6.0` client sent.
- **Which service signs is configuration, not code.** A deployment declares
  `--delegated-context-key <kid>=<issuer>=<base64url>`; substrate holds a verifying key and never a
  signing key. Identity and connectors are both named relying parties (ADR 0011).
- **Contract bundle `contracts/substrate-wire/0.7.0`**, an additive successor to `0.6.0` carrying
  the optional `delegated_context` request member, the `grant_ref` and `platform_principal`
  ledger/event fields and the eight named refusals — `delegated-context.absent`, `.malformed`,
  `.unknown-key`, `.signature-invalid`, `.audience-mismatch`, `.subject-mismatch`, `.expired` and
  `.grant-conflict`. 224 files; every earlier bundle directory unchanged.

- **Destination-bound egress apertures.** A confined run can now reach exactly one
  operator-declared destination and nothing else. `--egress-aperture <name>=<host>:<port>/tcp`
  (repeatable) declares one; a start selects it **by name** — `sandbox.network: "aperture"` plus
  `sandbox.aperture: "<name>"` — and may never carry a destination, at any depth, in any field.
  Accepted as [ADR 0013](adr/0013-egress-apertures-are-declared-by-the-operator.md); the mechanism
  is the one [design 10a](docs/design/10a-egress-mechanism-spike.md) proved.
- **The mechanism keeps the kernel floor literally intact.** The sandbox still runs under
  `--unshare-net` with loopback and no other interface, no route and no resolver. What an aperture
  adds is one listening socket **inside that namespace**, created by a short-lived helper that joins
  the namespace bubblewrap created (through `ioctl(netns_fd, NS_GET_USERNS)`, and against the
  `child-pid` bubblewrap reports on `--info-fd` — not the bubblewrap pid, which lives in the host
  namespace) and handed back over `SCM_RIGHTS`. A per-run forwarder in the run's own cgroup dials
  the pinned address from the host namespace, one relay process per connection, and dies with the
  run. Everything else is still `ENETUNREACH`.
- **The capability fact `exec.egress-apertures`**, the declared names and their pinned destinations,
  published only after the mechanism was exercised in a throwaway sandbox at startup — never after
  reading configuration, and never by dialling a declared destination, which would make readiness
  somebody else's uptime. Absent otherwise: a start naming an aperture is then `unserved`.
- **The applied aperture is an observation**, not an echo: `applied.network` becomes
  `{mode, name, destination, mechanism, bytes}` on the aperture branch, where `destination` is the
  address the forwarder dialled and `bytes` is counted in the forwarder, the only thing that sees
  them. A run with no aperture still reports `"none"`.
- **DNS stays outside the aperture.** The daemon resolves each declared host once, at declaration,
  and pins the address for the process's lifetime. A run with an aperture is given a generated
  read-only `/etc/hosts` holding exactly the declared name mapped to loopback — the whole of `/etc`
  it ever gets — so a child uses the URL the operator declared and the forwarder is what answers.
- **`--ca-bundle <path>`**, optional. TLS crosses the aperture byte for byte, but a sandbox has no
  trust anchor because it has no `/etc`; where one is configured, each run gets a private read-only
  **snapshot** of it, so rotating the source cannot change what a running child already trusts.
  Absent and unverifiable, never present and unverified.
- **Contract bundle `contracts/substrate-wire/0.6.0`**, an additive successor to `0.5.0`
  (`preserves_routes: 26`, `adds_routes: 0`) carrying the `aperture` start field on exec and
  pipe-session start, the `exec.egress-apertures` fact, the applied-network object and seven
  conformance vectors. `cargo xtask check-bundle 0.6.0` is in the gate.

### Changed

- `network: "aperture"` **with a declared name** is now served where the mechanism verified. Without
  a name it answers exactly as before — `exec.network-unserved`, the refusal
  `contracts/substrate-wire/0.4.0/vectors/http/egress-unserved.json` froze — so no earlier vector
  changes shape.
- `AppliedNetwork` gains a variant, which breaks exhaustive matches in Rust consumers.
- The capability snapshot's configuration generation now covers declared apertures **whole**, name
  and destination. Unlike a secret slot, what is behind an aperture is exactly what a client is
  told, so changing it must invalidate every snapshot.

- **Sealed secret slots.** A confined process can now be handed an operator-declared credential
  without the value existing in any byte substrate can emit. `--secret-slot <name>=<path>` declares
  one; a start names `secret_slots: [{"slot": …, "fd": …}]` and the driver copies the declared bytes
  into an anonymous `memfd`, seals it with exactly `F_SEAL_WRITE|F_SEAL_SHRINK|F_SEAL_GROW|
  F_SEAL_SEAL`, verifies the read-back, places it at the declared descriptor with `dup2` in
  `pre_exec`, and closes its own copy the moment the child exists. The child finds the mapping — names
  and descriptors only — in `SUBSTRATE_SECRET_SLOTS`. Accepted as
  [ADR 0012](adr/0012-secret-slots-are-sealed-memfds.md).
- **The capability fact `secrets.slots`**, the sorted list of declared slot names, published only
  from a probe that proved sealing in-process and descriptor pass-through through a real bubblewrap
  child. Absent otherwise: a start naming a slot is then `unserved`, never delivered by weaker means.
- **The delegated clean-room lane proves the slot guarantee on the wire**, against the shipped
  binary rather than in process. A confined child reads its slot from the declared descriptor and
  returns a SHA-256 of the bytes; it reports `F_GET_SEALS` as the declared set, a write refused with
  `EPERM`, and a descriptor table of exactly `{0,1,2}` plus its slot; the value is found in no
  captured argv, shaped environment, stdout, stderr, event page, ledger row, applied record, refusal
  body or daemon diagnostic; and `/proc/<daemon>/fd` holds no slot `memfd` while the child is still
  running and has not yet read. `bash scripts/delegated-lane.sh` now counts 48 cases and the
  portable lane 31, where it asserts `exec.secret-slots-unserved` and
  `exec.secret-slot-descriptor-invalid`.
- **Contract bundle `contracts/substrate-wire/0.5.0`**, an additive successor to `0.4.0`
  (`preserves_routes: 26`, `adds_routes: 0`) carrying `secret_slots` on exec and pipe-session start,
  the `secrets.slots` fact, the applied-confinement slot record and six conformance vectors.
- **`cargo xtask check-bundle <version>`**, in the gate. It verifies a released bundle as the fixed
  point of its authored source, its manifest, its compatibility counts against both route
  inventories, and every JSON document's schema classification.

### Changed

- The `pre_exec` descriptor closure is one `close_range` per gap in the retained set rather than two
  fixed windows. With no slot declared the retained set is `{0,1,2,barrier}` and the gaps are exactly
  the two former windows, so behaviour without slots is unchanged.
- The capability snapshot's configuration generation now covers declared slot **names**. Adding or
  removing a slot moves the snapshot; rotating the material behind one moves nothing observable, so
  an operator can rotate a credential without invalidating an admitted operation.
- **The `secrets.slots` probe now observes the two properties it publishes a fact about.** Design 11
  § 5 requires a bubblewrap child reporting the probe descriptor at its declared number *with the
  same seals and nothing else above 2*; the probe checked neither, accepting any child whose stdout
  was the sentinel followed by `sealed` — so a descriptor sealed `F_SEAL_WRITE` alone, or one handed
  over beside leaked descriptors, published the capability. The child now reports the inode behind
  its declared number, the memfd's link, and every descriptor it holds; the parent compares the
  inode against the memory it staged, the seal word `F_GET_SEALS` reads off that inode against
  `F_SEAL_WRITE|F_SEAL_SHRINK|F_SEAL_GROW|F_SEAL_SEAL`, and the descriptor set against `{0,1,2}`
  plus the declared number. Any disagreement withholds `secrets.slots` (invariant 3), and nothing is
  compared against a substring of the child's output any more.

## [0.2.2] — 2026-08-30

One behavioural change, in what a workspace root may be called.

### Changed

- **A workspace root may be a directory the operator already owns.** A root name no longer has to
  carry the `ws_` prefix, only to be a single path component; containment was always `openat2`
  beneath the pinned root descriptor with symlinks refused, never the prefix. A harness can now run
  against your actual checkout — `harness`, `engineering-protocols` — instead of only a `ws_`-named
  scratch copy. Workspaces the server creates are still minted as `ws_…`.

## [0.2.1] — 2026-08-29

Documentation and distribution release. This release changes no runtime behavior, route, schema,
wire identifier, capability, or contract-bundle byte.

### Added

- A self-contained public Docusaurus site with a project-specific landing page, eight reader-facing
  documentation pages, responsive themes, a Substrate mark, and strict broken-link and
  broken-anchor gates.
- A GitHub Pages workflow that installs from the npm lockfile, type-checks, builds, uploads only
  `website/build`, and deploys the resulting artifact from `main`.
- A public-website working agreement that requires the Atlas `website-docs` skill and keeps
  internal designs, ADRs, plans, reviews, work logs, contributor status material, and source links
  out of the reader-facing site.

### Changed

- The repository is public under Atlas ADR 0003 after a full-history and working-tree credential
  scan found no leaks. Public visibility does not change the proprietary licence and does not make
  the development contract bundles stable.
- The workspace crate version is `0.2.1`.

## [0.2.0] — 2026-08-24

First release from the standalone repository. Substrate was extracted from the b10x monorepo
at `e01ea676` with full history on 2026-08-23; everything before the extraction is recorded in the
monorepo's own ledgers, and the version continues from the manifest it arrived with.

### Added

- **Declared host roots are mounted read-only** (ADR 0010). A root the operator names is bound
  `--ro-bind` into the confined tree, so a run can read a declared host tree and can never write
  through it.

### Changed

- The shared cargo cache mounts are `sharing=locked`, so parallel confined builds cannot corrupt
  one cache.
- This repository is the canonical Substrate home, with its own gate (`bash scripts/gate.sh`); the
  surface speaks as b10x and a fence keeps it that way, and links that escape the repository are
  pinned to the extraction baseline.
