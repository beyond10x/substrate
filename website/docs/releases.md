---
title: Release notes
description: What changed in the newest Substrate release, generated from CHANGELOG.md.
---

# What changed in the newest release

This page ends with the newest released section of `CHANGELOG.md`, and `cargo xtask check-docs`
fails when the two differ. Earlier releases are in the
[full changelog](https://github.com/beyond10x/substrate/blob/main/CHANGELOG.md). The
[confinement adapters](./concepts/adapters.md) page explains each adapter the notes name.

The newest release is 0.29.0, published on 26 November 2026. The
[status page](./status.md#newest-release) summarizes it.

A release that moves the contract bundle needs its SDK clients upgraded with the daemon. See the
[deployment rules](./guides/deployment.md#rules-that-do-not-change-by-posture).

<!-- Generated from CHANGELOG.md by `cargo xtask docs-releases`. Edit CHANGELOG.md, not the text below. -->

## 0.29.0 (2026-11-26)

### Added in 0.29.0

- Static musl release archives: every release attaches
  `substrate-daemon-<version>-x86_64-unknown-linux-musl.tar.gz` and
  `b10x-substrate-<version>-x86_64-unknown-linux-musl.tar.gz` beside the two `gnu` archives, each
  holding one statically linked executable that runs on any x86_64 Linux, whatever its C
  library. They are built from the Dockerfile's `release-binaries-musl` stage of the same pinned
  image, with OpenSSL compiled in from source; `SHA256SUMS` and its keyless signature cover all
  four archives. Each musl archive also carries `musl-COPYRIGHT`, the licence and copyright
  notice of the musl C library linked into its binary, committed unchanged as
  `licenses/musl-COPYRIGHT`. The musl build selects the packages and targets the `gnu` build
  selects, so both resolve the same dependency features. Before any image tag moves, `release.yml` refuses a musl binary with a program
  interpreter or a dynamic section naming a shared library, or one that does not answer
  `--version` with the release's version in a digest-pinned Alpine 3.24.4 image; a recovery of
  a tag whose source has no musl stage publishes the `gnu` archives alone and says so. The job
  `musl-binaries` of `release-lanes.yml` runs the same build and checks on every pull request
  that changes what they build. The `gnu` archives, their names, their checks and the images are
  unchanged. The binaries keep musl's own allocator until a recorded measurement shows mimalloc
  is not slower on their allocation-heavy path. Held by the musl cases of the `xtask` test
  binary `release_workflow`, which run the release's steps offline against fake `readelf`,
  `docker` and `gh`.
- `substrate_host` builds for `x86_64-unknown-linux-musl`: its three `RENAME_NOREPLACE` renames
  make `renameat2` as the system call (on glibc `rename::tests` compares it with nix's wrapper),
  and its `ioctl` requests and control-message lengths take the type each C library declares.
- `substrate_host::confinement::LimitUnread` and `RunObservation.limit_unread`, and the member
  `limit_unread` of the `run_ended` audit record (design 36 D27). An ended `oci` run whose main
  process ended by `SIGKILL` that neither the adapter nor the embedder sent, and whose runtime
  state holds no `OOMKilled`, names no limit and states `unattributed_kill`. Every other run on
  every adapter states nothing, and a run never has both a limit and the statement. The member
  is no limit: a consumer that counts limit hits counts `limit`. No Rust consumer breaks at
  compile time (`RunObservation` and `RunEnded` are `#[non_exhaustive]`); a reader of the
  serialized record that refuses unknown members refuses a record with the new member. Held by
  `a_sigkill_no_source_explains_names_no_limit_and_is_stated` and the monitor's cases in
  `confinement::oci::tests`, the record's JSON case in `confinement::tests`, and the `oci` lane
  case `ESS confinement oci unattributed kill`, a command that sends `SIGKILL` to itself.
- `scripts/wait-checks.sh` waits for a pull request's checks or a commit's check runs and exits
  by what they came to: 0 when every check succeeded or was skipped or neutral, 1 when one
  ended any other way (a startup failure and a stale check included), 3 when one is still
  pending at its timeout, which is wall time, or none is reported, and 2 when `gh` fails on
  three asks in a row. It is POSIX sh over `gh` and `jq` and only reads. `--from` decides once
  on a list `gh` printed, and the module `wait_checks` of the `xtask` test binary `bot_scripts`
  holds each status on lists recorded from this repository.
- `substrate_host::confinement::RunUsage` and `RunObservation.usage`, and the member `usage` of
  the `run_ended` audit record (design 36 § 3a, D29 to D38). An ended run on `linux`, on
  `systemd` around `linux` and on `systemd` around `local` carries `wall_time_us` (from the
  start to the leader's exit), `cpu_time_us`, `memory_peak_bytes` and `processes_peak`, named
  and counted as the wire's `ResourceUsage`, also beside the limit that ended it and on an
  `Unknown` run. A figure that cannot be read is absent and the others are kept; reading them
  refuses nothing and changes no state. `oci`, `pod`, `local` and `macos` leave `usage` absent.
  An exec that asked for `measurements` takes the three counters from its terminal sample, and
  no wire byte changes: an exec that asked for none still has no `resource.usage`. No Rust
  consumer breaks at compile time (`RunObservation`, `RunEnded` and `RunUsage` are
  `#[non_exhaustive]`; build a `RunUsage` from `RunUsage::default()`); a reader of the
  serialized record that refuses unknown members refuses a record with `usage`. Held by the
  unit cases of `process.rs`, `confinement::systemd`, `confinement::linux` and
  `confinement::tests`, and by the lane cases `ESS confinement linux usage`,
  `ESS confinement systemd local usage` and `ESS confinement systemd linux usage`: a run that
  sleeps one second in eight processes, allocates 64 MiB and spends one second of CPU carries at
  least each figure, and at most what its bounds let it reach, on its observation and on its
  `run_ended`.

### Changed in 0.29.0

- `b10x-substrate-host` makes `memfd_create` and `renameat2` as system calls on every Linux C
  library, so a binary that embeds it links against glibc older than 2.27 and against musl. A
  unit test of the crate refuses a source file that calls either C library wrapper outside
  `memfd.rs` and `rename.rs`; on glibc the tests compare the `renameat2` system call with nix's
  wrapper over twelve cases.
- The `oci` adapter reads a container's state once after the runtime client returns, and
  publishes the run's end once that read and the removal are done (design 36 D28). Measured on
  a standard hosted runner on 2026-11-25, 610 memory-bound kills: the flag was at the first read
  in 482 and at no later read within 5 s in the other 128. The `oci` lane's memory case passes
  with either of the two ends design 36 allows, `memory`, or no limit and `unattributed_kill`,
  and prints which it read; any other end fails it.
- Every Linux workflow job runs on a runner image named by its version: `ubuntu-26.04` for the
  gate, the change classification, the chart and namespace lanes, the pages build and the
  release, and `ubuntu-24.04` for the release lanes, as before. The Ubuntu release a job runs on
  moves only with a commit; the image's weekly updates still arrive. `every_job_runs_on_a_runner_image_named_by_its_version` in
  `xtask/tests/release_workflow.rs` holds the table. The documentation site's job runs in the
  website repository's reusable workflow, pinned by commit, which names its own runner. The
  host crate's executable-list lane cases list the directory that holds each tool's file, with
  every symbolic link followed, so a tool kept as a link out of `/usr/bin` (Ubuntu 26.04's
  `ln`, `env`, `cat`, `ls` and `mkdir`) is admitted by the directory it lies in. A case in
  `ess_confinement_linux` holds that rule on a staged directory of links, on any host, without
  privilege or a mount. The staged-mount cases still run on a workstation only: a hosted runner
  grants a user namespace no mount.
