---
format: aep.planning-md/1
id: story:published-runtime-image-carries-dynamic-libraries
kind: story
status: active
title: Published runtime images carry their dynamic libraries
scope:
- confidence: cited
  path: CHANGELOG.md
- confidence: cited
  path: Cargo.lock
- confidence: cited
  path: Cargo.toml
- confidence: cited
  path: Dockerfile
- confidence: cited
  path: xtask/tests/release_workflow.rs
revision: 4
---
# Story: published runtime image carries its dynamic libraries

## Problem

The 0.7.1 release image builds successfully but the shipped daemon cannot start because the distroless runtime does not carry `libz.so.1`, which the exact release binary dynamically links.

## Acceptance

- Both daemon and MCP runtime targets explicitly carry every non-base dynamic library required by their shipped binaries.
- A repository test fences the runtime-library copy in the Dockerfile.
- A locally built daemon image starts far enough to print its CLI help.
- Version 0.7.2 passes the full repository gate and publishes immutable images.
