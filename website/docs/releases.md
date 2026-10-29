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

## 0.16.1 (2026-10-29)

Tag `0.16.0` points at commit `541cf55eb3efa7977aad6721f697668d1f029c28` and was not published.
`release.yml` releases only a commit whose `gate.yml` run concluded `success`, and `main`'s gate on
that commit ([run 40911403183](https://github.com/beyond10x/substrate/actions/runs/40911403183))
failed `cargo xtask check-advisories` on RUSTSEC-2026-0418, which was published after the pull
request's gate passed and before `main`'s ran. Release tags are write-once, so `0.16.0` cannot be
moved. 0.16.1 is the first published release of everything listed under 0.16.0.

That includes the Kubernetes namespace driver of design 34 and ADR 0047, as a development profile.
Exec on a cluster is unverified, and every exec start on the driver is refused
`exec.sandbox-unavailable`. Shared clean-room conformance follows in the next release: the
namespace lane reports its step 4 absent until `story:remote-clean-room-conformance` lands.

### Security in 0.16.1

- `libz-sys` is 1.1.31, taking the fix for
  [RUSTSEC-2026-0418](https://rustsec.org/advisories/RUSTSEC-2026-0418.html); it reaches the
  workspace through `curl-sys` and `libgit2-sys` under `substrate-host`.
  `THIRD_PARTY_LICENSES.html` is regenerated for that revision. The advisory's build script prints
  the build environment only when compiled with `cc` 1.6.0 or later. `Cargo.lock` held `cc` 1.4.4
  at `0.16.0`, in every revision of it on `main` since 2026-09-25, and still does, so this
  workspace's locked builds did not print it.
