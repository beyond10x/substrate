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

## 0.13.0 (2026-10-27)

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

### Added in 0.13.0

- The public site explains each confinement adapter (what it enforces, declares, refuses by name and needs, and when to choose it). It also gains an overview of the contracts with the ten families and eleven facts, release notes rendered from the newest released `CHANGELOG.md` section, and `confine run`; the status page moves to 0.12.0. The page list is grouped, marks the current page, and collapses on narrow screens; tables scroll inside focusable regions.
- `cargo xtask check-docs` holds the site's adapter matrices, family, fact and lane tables and CLI Commands table to the specification. It also holds the site to lower-is-better UX ratchets in `website/ux/baseline.json` (clicks per reader task, words per text block), which `cargo xtask docs-baseline` rewrites, and checks the `website/ux/findings/` records.
- The `macos` adapter resolves the selected developer tools' macOS SDK, `clang` and `ar` with `xcrun` outside the sandbox at open, with a cleared environment and `/` as its working directory, and a start gets `SDKROOT`, `CARGO_TARGET_AARCH64_APPLE_DARWIN_LINKER`, `CARGO_TARGET_X86_64_APPLE_DARWIN_LINKER`, `CC` and `AR` for each of them its exec subpaths reach, unless its environment names it, so a confined `cargo build` or `cargo test`, and a build script that compiles C, links without reaching the `/usr/bin/cc` shim, which runs `/bin/sh` and `xcodebuild`. A start over the developer tools is refused `exec.sandbox-developer-tools-unresolved` before anything runs, naming why, when the open could not resolve them, when the selected developer tools changed since the open, when the SDK the open resolved is gone, or when its exec subpaths hold the selected developer directory but not a value it needs; nothing is resolved again at start. The Seatbelt policy gains no rule. `tests/seatbelt_adversary.rs` runs the fixture's `cargo test` with no `RUSTC`, `SDKROOT` or linker set by the caller.

### Changed in 0.13.0

- The gate workflow's `pod-lane` job runs `scripts/pod-lane.sh`: the `ess_confinement_pod` and `adversary_pod` suites against the `kind` cluster, each required by its own `test result:` line with at least one case passed, none failed, ignored or filtered out, and no case reporting itself absent; the script takes no arguments. `security_review_pod` reads no lane variable and stays in the workspace gate. The job adds a second ServiceAccount bound to the Pod and NetworkPolicy Role without `pods/log` for the no-log case, and names the kind node for the kubelet-stop case, which now waits for the adapter to remove the run's Pod and NetworkPolicy once the kubelet answers again.
- `tests/confinement_public_items.rs` now freezes every type the `confinement` module's public items reach, to a fixed point, beside the module's own items. From the host crate that adds `SecretSlot`, `EgressAperture`, `GitSourceBinding`, `DriverError` and `DriverErrorClass`. From `substrate-wire` it adds `CapabilitySnapshot`, `CapabilityFacts`, `HostDriverKind`, `EgressApertureFact` and the fact types `CapabilityFacts` holds (`StorageQuotaFacts`, `ScratchQuotaFacts`, `ReadOnlyRootFacts`, `ReadOnlyFileFacts`, `ResourceUsageFacts`, `MetricsStreamFacts`, `StreamBoundsFacts`, `AttachmentBoundsFacts`, `ExecutionCapsuleFacts`, `NamespaceFacts`, `CgroupLimitFacts`). The list also covers:
  - an `impl` of a module type in any file of the host crate;
  - the associated types and constants of trait impls on held types;
  - a wire type that shares a name with a module type;
  - a type reached through a renaming `pub use … as …`.

  `tests/confinement-public-items.txt` lists them. Adding, removing or reshaping any of them changes the list.
- `b10x-substrate confine run` names its private group `confine-<pid>-<start time>` (the start time is field 22 of `/proc/<pid>/stat`). Before opening, it removes each such group whose owner has exited (including an owner still unreaped), deepest first, if the group holds no process. It never touches a running owner's group and never kills. Each group it leaves holding a process, or cannot remove, is named on one stderr line, and so is an unreadable root. Under `--output json`, these go instead into the verb's one document, as `left_groups` (`path` and `reason`) in `result` or in `error.data`. The exit status does not change.
- The public site's command-line guide describes how `confine run` removes a killed run's group and `left_groups`; the profiles guide describes the `macos` developer-tool resolution and its refusal.

### Fixed in 0.13.0

- An egress aperture's listening socket is shut down when an aperture list fails to install, before its forwarders are killed. Before, only the installing process's descriptor was closed, so a child another thread forked meanwhile could keep the socket accepting connections from inside the run. A list that installs completely closes the installing process's copy without a shutdown, so the forwarder holds the last one and a forwarder that dies mid-run leaves its aperture refusing connections. A descriptor received and then refused inside the install is shut down as well.
- `scripts/delegated-lane.sh` and `scripts/pod-lane.sh` read each suite's log through a here-string. Piped into `grep -q` under `pipefail`, a log longer than a pipe buffer after a match read as no match, so an `ABSENT` line could pass the lane.
