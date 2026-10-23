---
format: aep.planning-md/3
id: story:daemon-image-serves-exec-or-says-it-cannot
kind: story
status: active
title: The README says what the published daemon image provides for exec and what a deployment must supply
summary: The daemon stage carries bwrap 0.11.2 and socat (Dockerfile:48-59); README § Serving exec does not say so or name the four deployment prerequisites.
tags:
- review
relations:
- decomposes: epic:review-2026-09-03-findings
scope:
- confidence: cited
  path: README.md
revision: 13
transitions:
- {from: "draft", to: "proposed", at: "2026-10-23T06:27:15Z", actor: "agent:b10x-chef-orchestrator", revision: 12, decided_on: {"recorded":{"review_outcome":4}}}
- {from: "proposed", to: "active", at: "2026-10-23T06:27:15Z", actor: "agent:b10x-chef-orchestrator", revision: 13, decided_on: {"recorded":{"review_outcome":4}}}
---
# Story: The daemon image serves exec or says it cannot

## Context

Exec capability requires the configured bubblewrap binary and `/usr/bin/socat` on the host
(`crates/substrate-host/src/probe.rs`; documented in `README.md` § Serving exec). When this story
was drafted the daemon image was distroless and carried neither. That premise no longer holds: the
`daemon` stage now builds `FROM execution-prerequisites` (`Dockerfile:48-59`), an Ubuntu 24.04
base that installs `socat`, `coreutils` and `bash`, and copies a non-setuid bubblewrap 0.11.2
built from a checksum-pinned source tarball to `/usr/bin/bwrap`. The `mcp` stage alone stays
distroless (`Dockerfile:74`) and serves no exec by design.

What is still missing is the statement: `README.md` § Serving exec lists what a host must provide
and never says what the published image already provides, or what a deployment of that image must
still supply (the delegated cgroup subtree, a process-free delegation root, `--cgroup-root`, and a
container runtime that permits bubblewrap's namespaces).

## Acceptance

`README.md` § Serving exec states that the published daemon image carries bubblewrap (with the
`--disable-userns` support the probe needs) and `/usr/bin/socat`, names all four things a deployment of that
image must still supply before the probe advertises exec (a delegated cgroup v2 subtree, a process-free
delegation root, `--cgroup-root` naming it, and a container runtime that permits bubblewrap's
namespaces), and states that the disposable MCP image
serves no exec. Each claim about the image cites the `Dockerfile` stage it comes from.

## Notes

The GHCR release notes repeat the README; they are not a second claim. No image is pulled and run
by this story; whether the image passes the probe on a given host is the deployment's delegated
lane, not this statement.

## Parallel work

Lands on `README.md` only. Not in the 2026-10-14 wave: a documentation-only statement about an image
that already carries the prerequisites fixes neither correctness nor containment, so it waits for a
wave that touches the README.
