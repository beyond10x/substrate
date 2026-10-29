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

## 0.16.0 (2026-10-29)

### Added in 0.16.0

- The host execution AppArmor profile `substrate-host-exec-v2`
  (`deploy/apparmor/substrate-host-execution-v2`): `v1`'s rules with `noatime` and `nodiratime`
  added to the `remount options in (…) /**` set, no comment lines, and nothing else changed. Bubblewrap keeps a
  source mount's `noatime` when it remounts a bind read-only. `v1` refuses that remount, so on a
  node whose filesystems are mounted `noatime` the confinement probe failed and the daemon
  published no `exec.*` fact. A test checks every remount flag combination against both
  policies: `v2` refuses everything `v1` refuses except a remount that adds `noatime` or
  `nodiratime`.
- Development contract bundle `substrate-wire/0.25.0` (design 34 § 11, ADR 0047), now advertised
  by the daemon and the Rust SDK. It adds no route: a capability snapshot's `driver` may be
  `kubernetes`, a driver may publish the optional fact `confinement.no-egress`, and three refusals
  are published at 501 with a vector each: `exec.inline-capsule-unserved`, `exec.signal-unserved`
  and `workspace.adopt-unserved`. The host daemon's answers do not change.
- The release publishes the executor image `ghcr.io/beyond10x/b10x-substrate-executor:<version>`
  from the Dockerfile's `executor` target (design 34 § 10.4): write-once, pushed only after its
  `version` handshake names this release and the tagged commit and its executable is static,
  keyless-signed, verified with the tagged workflow identity and read back anonymously before the
  GitHub release is announced, with its digest beside the others. The release also refuses a tag
  without a `Namespace-lane: <sha> <observation-count> <RFC 3339 UTC>` record for the tagged commit,
  counted from `OBSERVATIONS` in `xtask/src/namespace_lane.rs` (D182), and a tag whose commit has
  no successful `namespace-lane.yml` run.
- The Kubernetes namespace driver (design 34, ADR 0047): `substrate-daemon --driver kubernetes
  --kubernetes-config <file>` serves workspaces as a PersistentVolumeClaim with a keeper Pod, every
  file route through the executor over `pods/exec`, and each exec as one Pod, in one namespace the
  driver claims with a Lease. The file is a self-contained JSON kubeconfig with the context
  extension `b10x-substrate/namespace-driver`; nothing else is read, no ambient kubeconfig and no
  in-cluster account. A file that does not load stops startup with `kubernetes.config-invalid`, and
  host-only flags are refused (`config.driver-conflict`). A cluster check that fails leaves every
  driver fact absent while the daemon serves; each failure's code is logged. `/readyz` reads the
  snapshot taken after the first probe, or when the cluster check fails for a reason a retry cannot
  clear; a held Lease is retried. The driver's Role needs `get` and `create` on `pods/exec`,
  because the API server authorizes the exec WebSocket as
  `get`, and `get` and `list` on `networkpolicies`. A ResourceQuota refusal is `workspace.capacity`
  or `exec.capacity` (429). `workspace.adopt` is refused `workspace.adopt-unserved` before any name
  is looked up, and the `INT` signal `exec.signal-unserved`, both 501. After a restart a running exec reads `unknown` and its Pod is deleted. The
  probe never proves the four Linux confinement facts or `confinement.no-egress`, so every exec
  start on this driver is refused `exec.sandbox-unavailable` in this release. The deployment guide
  has its section.
  - **Limits.** The driver is a development profile. Exec on a cluster is unverified: on the kind
    lane a Pod under the default-deny policy still reached the API server (design 34 § 10.3 O3), so
    no enforced egress denial was observed and `confinement.no-egress` stays absent. Shared
    clean-room conformance follows in the next release: the namespace lane reports its step 4
    absent until `story:remote-clean-room-conformance` lands. Only empty workspaces are served; a
    storage quota is `workspace.storage-quota-unserved` and any other source
    `workspace.source-unserved`, both 501.
- `cargo xtask namespace-lane check` holds the namespace lane's rendered objects to their goldens
  and its Role to ADR 0047 decision 3, run by the new `namespace-lane.yml` workflow;
  `cargo xtask namespace-lane observe` runs design 34 § 10.3's observations on a kind cluster of its
  own on a workstation and prints the `Namespace-lane:` record a release requires.
- `cargo xtask chart-lane cluster --node-profiles --image <ref> --node-image <ref>` runs the four
  execution-enabled scenarios the hosted chart lane reports absent, on a one-worker kind cluster
  built from `fixtures/chart-lab/Dockerfile` on a host whose kernel enables AppArmor, and prints
  `Chart-lane: <sha> <scenario-count> <RFC 3339 UTC>` only when every scenario passed at a clean,
  unmoved `HEAD` with an image carrying that revision. A release that publishes the chart now
  refuses a tag without that record for the tagged commit, counted from `SCENARIOS` in
  `xtask/src/chart_cluster.rs`.

### Changed in 0.16.0

- `/metrics` on the operational listener holds at most 582 samples (was 579): the refusal counter
  gains the three codes of bundle `0.25.0`. The 471 samples without it are unchanged.

- The specification and CI use ESS 0.76.0 (was 0.75.0); the suites and CLI package regenerate unchanged, and the four generated type files differ only in their generator header, the workspace types also in their schema projection digest (the root name now publishes its length bound).
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

### Fixed in 0.16.0

- The host confinement probe binds its sentinel socket on a path that fits `sun_path`. Under a
  long `TMPDIR` the path exceeded it, so the probe answered false and the daemon published no
  `exec.*` fact for a reason unrelated to the host. It now tries a fresh private directory under
  `TMPDIR`, then `XDG_RUNTIME_DIR`, then binds through `/proc/self/fd`; when none fits, the log
  names each path's length and the limit.
- The `pod` confinement adapter's Pod removal never deletes a same-named replacement. Once it has
  read the Pod's uid, every delete carries that uid as a precondition, and a 409 or a read showing
  another uid ends the removal as the original Pod gone, without deleting the other object.
