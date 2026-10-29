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

## 0.16.1 (2026-10-29)

Published by [release run 40949928365](https://github.com/beyond10x/substrate/actions/runs/40949928365)
(attempt 2) for tag `0.16.1` at commit `f2fd606b88a0b778970e8babe9a96eb600cd57b2`. All four images,
the chart and the binaries' `SHA256SUMS` were keyless-signed and verified against
`release.yml@refs/tags/0.16.1` by the release workflow, and each digest resolved anonymously:

- Image: `ghcr.io/beyond10x/b10x-substrate-daemon:0.16.1` at `sha256:ab798921666cbbbcdbcc36da132b106dfcd7ee03bb7b0b5adcf7edbddc61ae88`.
- Contract bundle: `ghcr.io/beyond10x/b10x-substrate-wire:0.25.0` at `sha256:6f31a62628ae3e084134f4fe47900eaf2fc1932ced61c54ca9498f39b9c7e322`, first published by this release.
- Disposable MCP image: `ghcr.io/beyond10x/b10x-substrate-mcp:0.16.1` at `sha256:77342f97515579f1c5b12c9aa4b6429d9e23d0dd9fbce97932ac34cb6346959d`, development-only.
- Executor image: `ghcr.io/beyond10x/b10x-substrate-executor:0.16.1` at `sha256:e3ed53485640dc759760dd47eb08ec0eb9eea895aa8bcd630f60e5087ab6729e`, first published by this release.
- Chart: `oci://ghcr.io/beyond10x/charts/b10x-substrate` `0.16.1` at `sha256:23b9f96f5fbfb5f3c4091f32f6d7c0da52a657fbcb185a25c5d2a7d7883f03a0`.
- Binaries: `substrate-daemon-0.16.1-x86_64-unknown-linux-gnu.tar.gz` and `b10x-substrate-0.16.1-x86_64-unknown-linux-gnu.tar.gz`, listed in `SHA256SUMS` (`sha256:a5b3170fad432678b1da5f451145dc834d09e64b8c4d59ff6a16c3ba27aa49fc`) and signed in `SHA256SUMS.sigstore.json`.
- The tag records `Confinement-lane:` 182, `Quota-lane:` 7, `Chart-lane:` 4 and `Namespace-lane:` 8, each at `f2fd606b`.

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
