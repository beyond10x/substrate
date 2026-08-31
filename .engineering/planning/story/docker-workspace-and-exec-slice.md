---
format: aep.planning-md/1
id: story:docker-workspace-and-exec-slice
kind: story
status: proposed
title: The Docker driver serves workspace and exec
summary: An explicit Docker socket backs the shared workspace/exec contract with closed container specs, durable dispatch and truthful capability facts.
owner: substrate
tags:
- docker
- driver
relations:
- decomposes: epic:container-driver-entry
- depends_on: story:docker-driver-entry-gate
- depends_on: story:remote-clean-room-conformance
revision: 2
---
# Story: The Docker driver serves workspace and exec

## Outcome

The same public workspace, file, operation, event and bounded argv-only exec contract can run through a Docker-backed driver without a caller learning which driver answered.

## Acceptance

The daemon connects only to the explicitly configured Docker socket and publishes the accepted root-equivalent authority fact. Every mutation is durable before the Docker API call. The driver builds a closed container specification: pinned image digest, argv array, cleared/shaped environment, read-only system image, explicit writable workspace, non-root user where the image permits it, no privileged mode, host namespaces, arbitrary bind mounts, device requests or caller-supplied runtime options. Limits, timeout, output draining, whole-container cleanup and applied observations pass the shared driver conformance journey. Unsupported sealed slots and destination apertures remain absent and requests receive named unserved refusals. Restart reconciliation binds observations to immutable container identity.

## Design gate

The accepted Docker entry design and its mechanism probes must be complete before implementation. Any additional wire capability or refusal requires its successor bundle and design closure first.
