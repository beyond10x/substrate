---
title: Release notes
description: What changed in the newest Substrate release, generated from CHANGELOG.md.
---

# What changed in the newest release

This page ends with the newest released section of `CHANGELOG.md`, and `cargo xtask check-docs`
fails when the two differ. Earlier releases are in the
[full changelog](https://github.com/beyond10x/substrate/blob/main/CHANGELOG.md). The
[confinement adapters](./concepts/adapters.md) page explains each adapter the notes name.

What 0.16.1 first publishes is listed under 0.16.0 in the full changelog and summarized on the
[status page](./status.md#new-in-0161), including the namespace driver's limits.

<!-- Generated from CHANGELOG.md by `cargo xtask docs-releases`. Edit CHANGELOG.md, not the text below. -->

## 0.17.0 (2026-10-30)

Shared clean-room conformance arrives: design 31's remote runner certifies a deployed endpoint from
outside it, and the Kubernetes namespace lane runs it as step 4. The namespace driver still refuses
every exec start `exec.sandbox-unavailable`; admitting one needs design 30's neutral confinement
families, which wait on an operator decision.

### Added in 0.17.0

- `b10x-substrate-conformance`, design 31's clean-room runner: it certifies one deployed endpoint
  over HTTPS and WSS from outside it, pinned to `substrate-wire/0.25.0`, and writes `report.json`
  with `report.json.sha256` under a closed schema (exit 0 passed, 1 failed, 2 invalid input, 3
  harness failure, 4 incomplete). Build it with
  `cargo build --release --locked -p b10x-substrate-conformance`; `cargo xtask check-packages`
  holds that this build links no daemon, host driver or store. Guide: *Remote conformance*. No
  release artifact (design 31 Q3).
- The runner checks its input before the first request: `out-uncreatable` (the `--out` directory
  is created at the start and removed again when the run stops without a report),
  `token-command-unusable` and `restart-hook-unusable` (the program is not an absolute path to an
  executable file) are exit 2 with no report. Its token command and restart hook start with only
  the standard streams open.
- Exit 3 (harness failure, no report) names `deadline-exceeded` (the handshake, the connect and the
  final machine read run under `--deadline`), `token-fd-poisoned` (an exchange on `--token-fd`
  timed out or answered an over-long line; every later fetch fails) and `report-outside-schema`
  (the report is checked against its own schema before it is written). A chunk size or
  `content-length` beyond the response bound is a malformed answer.
- Cases fail by name instead of reading without bound: `events-page-shrank`,
  `status-unrepresentable`, `session-output-exceeded` (64 KiB of session output). An endpoint whose
  first `/v1/machine` body does not decode is exit 1 with a report: the failed case
  `machine.envelope-decodes` (`envelope-undecodable`) and `capability_snapshot: null`.
- The namespace lane's step 4 runs the runner against the `--driver kubernetes` daemon with a
  crash restart hook, and the chart lane's hosted scenarios gain "clean-room runner" against
  ordinal 0, with the report uploaded as the `clean-room-report` artifact. Step 4 counts a daemon
  restart only when the exit was the SIGKILL its restart hook sent; any other daemon exit stops the
  runner and fails the step (`daemon-exit-not-caused-by-hook`), and no exit at all is
  `hook-caused-no-exit`. Both lanes refuse a report or runner output that holds the credential,
  its body or the token prefix, and delete the report, its digest and the runner's log before
  failing, so the `clean-room-report` artifact never carries them.
- The operational listener of a daemon running `--driver kubernetes` serves
  `substrate_kubernetes_collisions_total`, a counter with no labels: the objects the driver met
  under a name it would use and did not create, each left untouched (design 34 § 4, the one
  addition it makes to ADR 0045's catalogue). Each object is counted once, keyed on its kind, name
  and uid, however often a provisional loop or a read meets it again; the driver remembers 4096
  such objects, and past that a new one is counted every time it is met, so the series can
  overcount but never misses an object. A foreign Pod met under an exec Pod's name at start is
  counted too. Its scrape carries at most 583 samples. A host daemon publishes no such series and
  its scrape is unchanged at 582. The driver port gains `Driver::backend_collisions`, `None` by
  default.

### Changed in 0.17.0

- Changed contract: the report schema `remote-conformance-report.v1` differs from the one design 31
  published before this release. `capability_snapshot` and each snapshot read may be `null`; a
  snapshot read records only a `sha256:` digest and an RFC 3339 time; `not_covered` also lists
  every pinned registry operation no case names (`{"operation": …}`); a word (driver, driver
  version) is at most 32 characters of `[0-9A-Za-z.+-]` starting alphanumeric, so it no longer
  admits `_`. A driver version outside that fails `machine.envelope-decodes` with
  `driver-version-unrepresentable`. A consumer that parsed reports against design 31's table must
  accept `null` there.
- Pinned ESS 0.77.0 across every specification and the CI gate (`ESS_VERSION` and `ESS_SHA256` in
  `.github/workflows/gate.yml`); `cargo xtask check-ess` passes with unchanged scenario counts.

### Fixed in 0.17.0

- A Pod removal no longer treats a 409 on its uid-preconditioned delete as the Pod gone: it reads
  the name, and retries while the Pod it read is still there. The Status of the 409 does not
  decide, because an admission webhook sets its own.
- Kubernetes driver: an exec Pod still holding its bound uid 60 s or more after the driver's
  deletion was accepted ends the exec `unknown`, not `running`, even when that read already shows
  the container terminated, so the end no longer depends on when the exec is first observed; the
  garbage round keeps deleting it (design 28). A `kill` or `term` whose delete answer was lost
  starts the same bound at the first read that shows the Pod's `deletionTimestamp`; before, such
  an exec stayed `running` when no garbage round ran.
