---
format: aep.planning-md/1
id: story:daemon-image-serves-exec-or-says-it-cannot
kind: story
status: draft
title: The published daemon image carries bubblewrap and socat, or documents that it serves no exec
summary: Exec needs bwrap and /usr/bin/socat (probe.rs:319); the distroless image (Dockerfile:12) has neither; inferred from the base image.
tags:
- review
relations:
- decomposes: epic:review-2026-09-03-findings
scope:
- confidence: cited
  path: Dockerfile
- confidence: cited
  path: README.md
- confidence: cited
  path: STATUS.md
- confidence: cited
  path: crates/substrate-host/src/probe.rs
revision: 7
---
# Story: The daemon image serves exec or says it cannot

## Context

Exec capability requires the configured bubblewrap binary and `/usr/bin/socat` on the host
(`crates/substrate-host/src/probe.rs:319-320`; documented at `README.md:280`). The published
daemon image is `gcr.io/distroless/cc-debian12:nonroot` (`Dockerfile:12`) and installs neither,
nor `/usr/bin/env`, which the sandbox argv execs (`process.rs`, `command`). Inferred from the base
image, not pulled: that image can only answer `exec.sandbox-unavailable`, and a reader of the
release notes is not told so.

## Acceptance

`README.md` § Serving exec states whether the published daemon image serves exec and, when it does not, names what a host must add.

## Notes

The GHCR release notes are generated from the release workflow and repeat the README statement; they are not a second claim. The statement holds either way: if a later story ships bubblewrap, socat and coreutils in the image and runs the delegated lane against it, the README says the image serves exec. The `socat` dependency exists only to prove the seccomp `AF_UNIX` refusal; a small Rust probe child would remove it.

## Parallel work

This story shares `crates/substrate-host/src/probe.rs` with story:backend-recheck-hashes-only-on-change, story:confined-processes-cannot-nest-user-namespaces, story:daemon-image-serves-exec-or-says-it-cannot, story:exec-oom-kills-the-whole-tree and story:seccomp-denies-af-vsock; three of them also share `crates/substrate-host/src/process.rs`. Work them one at a time, or in one wave by one implementor; `aep artifact waves` sequences them.
