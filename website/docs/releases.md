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

## 0.21.0 (2026-11-19)

The wire, the contract bundle (`substrate-wire/0.26.0`) and every published route are unchanged.
A deployment declares up to six egress apertures; a confined run on a Debian-family host finds
its `/etc/alternatives` tools; the daemon's `--tmp-bytes` sizes every run's `/tmp`. Four RustSec
advisories are taken.

### Changed in 0.21.0

- A deployment may declare up to six egress apertures (`MAX_DEPLOYMENT_EGRESS_APERTURES` in the
  daemon, ADR 0013 amendment); one run still selects at most four (`MAX_EGRESS_APERTURES`, wire and
  bundles unchanged). A host serving GitLab and GitHub instances declared five and the daemon
  refused to start (#225).

### Added in 0.21.0

- The Rust SDK's command, `pipe_session` and `pty_session` builders gain `writable_root`, repeatable
  like `read_only_root`, and send the roots in the order given; the SDK re-exports `WritableRoot`,
  `WritableHostRoot`, `WritableGitRoot`, `AppliedWritableRoot` and `MAX_WRITABLE_ROOTS`. More than
  `MAX_WRITABLE_ROOTS` roots are refused before the request is sent. The wire, the bundle and the
  daemon are unchanged.
- The Rust SDK's command, `pipe_session` and `pty_session` builders gain `apertures`, which selects
  several declared egress apertures in order (design 26 § 2); the n-th is reached in the run at
  `127.0.0.n`. One name is still sent as `aperture`, so an existing start serializes as before.
  The wire, the bundle and the daemon are unchanged.
- The daemon's `--tmp-bytes BYTES` (`SUBSTRATE_TMP_BYTES`) sizes every confined run's `/tmp`
  (#224, ADR 0049); absent keeps the kernel default, half the RAM. Startup refuses a size under one
  MiB as `config.tmp-bytes-invalid`, and a size with a bubblewrap that does not take `--size` as
  `config.tmp-bytes-unserved`. The wire and the bundle are unchanged.

### Fixed in 0.21.0

- A confined run on a Debian-family host finds `cc`, `awk`, `which` and every other `/usr/bin`
  name that links through `/etc/alternatives`: the Linux base view now binds the host's
  `/etc/alternatives` read-only when it has one (#223, ADR 0049). Nothing else of `/etc` joins it.

- Tests make their FIFOs with `nix::unistd::mkfifo` instead of running `mkfifo`, which on the CI
  runner now refuses a non-UTF-8 path ("invalid UTF-8 was detected in one or more arguments") and
  failed the omitted-entries suite before the daemon was reached. Test-only change.

### Security in 0.21.0

- `libz-sys` is 1.1.33, taking zlib's fix for
  [RUSTSEC-2026-0544](https://rustsec.org/advisories/RUSTSEC-2026-0544.html), a heap buffer
  overflow in `gzwrite` after a write stall. The host driver's Git stack links it through
  `curl-sys`.
- `rusqlite` is 0.41.1 and the bundled `libsqlite3-sys` 0.39.1, taking SQLite's fix for
  [RUSTSEC-2026-0459](https://rustsec.org/advisories/RUSTSEC-2026-0459.html), uninitialised memory
  read through the WAL. The daemon's ledger and the SDK's store link this SQLite.
- `rmcp` is pinned at 3.9.0, taking the fixes for
  [RUSTSEC-2026-0461](https://rustsec.org/advisories/RUSTSEC-2026-0461.html),
  [RUSTSEC-2026-0457](https://rustsec.org/advisories/RUSTSEC-2026-0457.html) and
  [RUSTSEC-2026-0462](https://rustsec.org/advisories/RUSTSEC-2026-0462.html). All three are in the
  Streamable HTTP transports and the OAuth client, which the model-only pin of `substrate-mcp`
  does not compile, so no shipped binary was reachable through them.
- `THIRD_PARTY_LICENSES.html` names the new versions.
