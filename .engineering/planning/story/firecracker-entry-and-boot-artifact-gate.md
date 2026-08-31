---
format: aep.planning-md/1
id: story:firecracker-entry-and-boot-artifact-gate
kind: story
status: proposed
title: Firecracker starts only from verified immutable boot artifacts
summary: KVM, jailer, kernel/rootfs provenance and host isolation are probed and refused by name before the direct driver exists.
owner: substrate
tags:
- driver
- firecracker
- security
relations:
- decomposes: epic:firecracker-driver
- depends_on: story:remote-clean-room-conformance
- depends_on: story:driver-port-carries-no-host-types
revision: 2
---
# Story: Firecracker starts only from verified immutable boot artifacts

## Outcome

The repository has an accepted and executable entry gate for a direct Firecracker driver, including the host and artifact properties needed to make a microVM confinement claim.

## Acceptance

An accepted design or ADR fixes supported architectures and Firecracker version, /dev/kvm access, jailer uid/gid and chroot layout, seccomp and cgroup ownership, kernel command line, tap/network default, vsock identity, boot timeout, crash cleanup, snapshot stance, and boot-artifact provenance. Kernel and rootfs inputs are operator-configured OCI artifacts or files pinned by digest; their bytes are verified before dispatch and callers cannot supply host paths. Startup probes exercise KVM and the jailer in a disposable VM and publish facts only after success. Missing KVM, unsupported CPU, unsafe permissions, unverified artifacts and failed jailer setup are separate named unserved refusals.

## Development environment

Live conformance runs only on a dedicated tainted KVM-capable node pool. The current T3a/T4g EKS nodes have no KVM surface; on them the portable lane must report the Firecracker cases absent and must never report them passed.
