---
format: aep.planning-md/3
id: story:oci-adapter
kind: story
status: implemented
title: 'The oci adapter: Docker or podman'
owner: substrate
tags:
- adr-0109
relations:
- decomposes: epic:confinement-contract-family
- depends_on: story:confinement-contract-on-linux-and-local
- informed_by: epic:container-driver-entry
scope:
- confidence: inferred
  path: .github/workflows/gate.yml
- confidence: inferred
  path: CHANGELOG.md
- confidence: inferred
  path: crates/substrate-host/src/confinement/oci.rs
- confidence: inferred
  path: crates/substrate-host/tests/ess_confinement_oci.rs
- confidence: inferred
  path: docs/design/04-security-and-isolation.md
revision: 8
transitions:
- {from: "draft", to: "proposed", at: "2026-10-27T01:13:04Z", actor: "human:timo", revision: 6, decided_on: {"recorded":{"review_outcome":3}}}
- {from: "proposed", to: "active", at: "2026-10-27T01:13:04Z", actor: "human:timo", revision: 7, decided_on: {"recorded":{"review_outcome":3}}}
- {from: "active", to: "implemented", at: "2026-10-27T07:47:38Z", actor: "human:timo", revision: 8, decided_on: {"recorded":{"test_result":1,"review_outcome":10}}}
---
# The `oci` adapter: Docker or podman

ADR 0109 decision 5: the `sandbox` repository's Docker argv becomes the `oci` adapter
(`sandbox` `src/docker.rs:42-130` at `1b0377d`). The adapter runs a command in a container with the
posture `sandbox` uses — `run --rm`, `--user <uid>:<gid>`, `--network none`, `--cap-drop ALL`,
`--security-opt no-new-privileges`, `--read-only --tmpfs /tmp`, a read-only `/workspace` tmpfs with
the declared writable mounts bound beneath it, read-only roots as `-v <root>:<root>:ro`, `-w`, and
the five baseline variables — plus what `sandbox` lacks: `--pids-limit`, `--memory`/`--memory-swap`
and `--cpus` from the profile's bounds, an image pinned by digest, and the runtime's default seccomp
profile kept. It serves `process_run`, `process_watch`, `output_read`, `wait`, `workspace`,
`filesystem_view`, `limits` and `audit`; not `network_egress` or `network_ingress` (no forwarder:
refused by name). It proves `image_digest_pinned`. It probes the runtime (`docker` or `podman` on
`PATH`, `info` answering, the runtime rootless or not as a fact) at open and fails closed.

This is a confinement adapter behind substrate-host's API, recorded as such in ADR 0044
(`story:confinement-domain-specification`). It is not plan 03's wire-level Docker driver
(`epic:container-driver-entry`, `docs/design/15-docker-driver-entry-gate.md`), which stays
deferred; ADR 0036 S8's container records are a separate host-process family.

## Acceptance

- `crates/substrate-host/tests/ess_confinement_oci.rs` executes every `oci` scenario of the suite
  against Docker on the CI gate runner: `.github/workflows/gate.yml` sets `SUBSTRATE_OCI_LANE=1` on
  the existing gate job, in which an absent runtime fails the test rather than skipping it; without
  the variable the lane is absent and says so.
- A profile with `pids_max` 8 observes the fork refusal inside the container; a write outside the
  declared mounts fails; an outbound connection fails; a profile needing `network_egress` is refused
  `confinement.family-unserved` before any container starts.
- An image named by tag only is refused `confinement.image-unpinned`; the adapter runs only a
  digest.
- `render` for a fixed profile returns the `docker run` argv, and a portable test asserts it holds
  the flags `sandbox` lacks: `--pids-limit`, `--memory`, `--memory-swap`, `--cpus` and the image by
  digest. The `sandbox` flags are `story:sandbox-repository-parity`'s to assert.
- The container is removed after `wait`, after a kill, and after a timeout (no container with the
  adapter's label remains: `docker ps -a --filter label=…` empty).
- An `aep:adversary` pass and an `aep:security-reviewer` pass (the Docker socket is
  root-equivalent, `docs/design/04-security-and-isolation.md`) are each recorded as a
  `review-result` that `reviews:` this story, and each of their findings has a `review_outcome`
  record: `fixed` in this story, or `escalated` with a filed story id.

## Scope

`crates/substrate-host/src/confinement/oci.rs` (replaces the S0 stub),
`crates/substrate-host/tests/ess_confinement_oci.rs` (new), `.github/workflows/gate.yml` (one
environment variable on the gate job; `story:pod-adapter` adds a separate job to the same file and
merges after this story), `docs/design/04-security-and-isolation.md` (the adapter's authority
paragraph), `CHANGELOG.md` (one line under `## [Unreleased]`; the other units of its wave add
theirs, and the merge into the integration branch keeps every line).

## Depends on

`story:confinement-contract-on-linux-and-local`.
