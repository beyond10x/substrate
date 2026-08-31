---
format: aep.planning-md/1
id: story:firecracker-workspace-and-exec-slice
kind: story
status: proposed
title: Firecracker serves one microVM per exec
summary: A Rust guest agent over vsock runs a bounded argv-only command with a read-only boot closure and writable workspace, then reports exact evidence.
owner: substrate
tags:
- exec
- firecracker
- microvm
relations:
- decomposes: epic:firecracker-driver
- depends_on: story:firecracker-entry-and-boot-artifact-gate
revision: 2
---
# Story: Firecracker serves one microVM per exec

## Outcome

A direct Firecracker driver runs one Substrate execution in a fresh microVM and returns the same durable operation, output, limit and observation semantics as the host driver where the backend can prove them.

## Acceptance

The host records the operation before creating any VM resource. A Rust guest agent reached over authenticated instance-bound vsock receives only a validated argv, shaped environment and limits; no shell or general remote command protocol exists. The verified kernel/rootfs and execution capsule are read-only. The workspace is a separately bounded writable device or filesystem whose exact design is accepted before code. vCPU, memory, process count, disk, output and wall-time limits are enforced at the VM and host-cgroup layers, and exact available usage is observed without inventing mean memory. Timeout, cancellation, attachment loss and daemon restart clean the Firecracker process, jail and transient devices and reconcile by immutable VM identity. Shared conformance runs on the dedicated KVM node and portable cases name absence elsewhere.

## Initial capability boundary

No egress, secret slots, PTY, snapshots, suspend/resume or multi-VM workload lifecycle in this slice. Their facts remain absent and requests are refused by name.
