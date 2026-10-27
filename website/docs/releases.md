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

## 0.14.0 (2026-10-27)

Published by [release run 40558015370](https://github.com/beyond10x/substrate/actions/runs/40558015370)
for tag `0.14.0` at commit `0b9d50c6105cacc7f959eb41d5e65f252084a282`. All three images and the
binaries' `SHA256SUMS` were keyless-signed and verified against `release.yml@refs/tags/0.14.0` by
the release workflow, and each digest resolved anonymously:

- Image: `ghcr.io/beyond10x/b10x-substrate-daemon:0.14.0` at `sha256:a22fc8a1707ac750eb86115422e26ecfb2fde64fb3df588e9fced32ef2a52182`.
- Contract bundle: `ghcr.io/beyond10x/b10x-substrate-wire:0.23.0` at `sha256:c2a50222da154028011a794cc93724043943581ba4ee44051c38dd084b759ff5`, unchanged since 0.11.0 and signed again by this release.
- Disposable MCP image: `ghcr.io/beyond10x/b10x-substrate-mcp:0.14.0` at `sha256:c6844980db262412db962228bb263421e285d57226e7c8a3a28fbbb85d589d42`, development-only.
- Binaries: `substrate-daemon-0.14.0-x86_64-unknown-linux-gnu.tar.gz` and `b10x-substrate-0.14.0-x86_64-unknown-linux-gnu.tar.gz`, listed in `SHA256SUMS` (`sha256:521435c3007898ade2a021e13f799dd5789fddc44f81cfce051d186b6c67f480`) and signed in `SHA256SUMS.sigstore.json`.

### Added in 0.14.0

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

### Changed in 0.14.0

- The public site is built with Docusaurus on the shared Docs System, with the same pages, paths, grouping and heading IDs, and the start page unchanged. The Mermaid diagrams on the operations and system model pages render as diagrams, code blocks are highlighted, and the adapter matrices show `TRUE`, `UNKNOWN` (proved by the open-time probe on this host) and `FALSE` chips.
- `cargo xtask docs-provenance --site website/build --commit <full-sha>` replaces `cargo xtask build-docs`: it checks the built site, holds the click ratchet on it, and writes `.well-known/b10x-site.json`, `.well-known/b10x-routes.json` and `.nojekyll`. `cargo xtask check-docs` keeps the Markdown checks and the reading-load ratchet without Node, and checks the pages against `website/sidebars.ts` and the start page's links. `cargo xtask docs-baseline` takes `--site` to measure clicks.
- The *Release notes* page is generated from the newest released `CHANGELOG.md` section by `cargo xtask docs-releases`, and `check-docs` fails when the two differ.
- *Getting started* now installs the command line and runs one command under the `oci` or `linux` adapter without a daemon, then shows a named refusal. The daemon walkthrough moves, with its heading IDs, to *Run a bounded command*.
