---
format: aep.planning-md/3
id: epic:host-process-supervision
kind: epic
status: draft
title: Host process supervision
revision: 1
---
# Epic: host process supervision

## Problem

Long-lived processes that agent sessions start are owned by nothing once the session ends: user
systemd units written or enabled by a session, processes re-parented to PID 1, background shells
and wait loops nobody awaited, and Docker and k3d containers left running with restart policies.
On 2026-10-16, 57 such leaks were found on one host across six mechanisms. Sessions also wait on
those processes by polling logs and files, and the waits time out, never match or spin.

## Outcome

Every long-lived process a caller starts on the host is a Substrate record: owned by a session,
held under a lease or a declared `keep` reason, bounded by an optional hard deadline, killed as a
whole cgroup when it ends, listed with what was observed, logged to a bounded file, and followed
through an event stream instead of a polling loop. Its lack of confinement is explicit on every
record.

## Decision and specification

`adr/0036-host-processes-are-a-supervised-resource-family.md`
(`architecture-decision-record:host-processes-are-a-supervised-resource-family`), status
proposed. The ESS domain is `spec/host-processes/`; each story's acceptance is a set of named
conformance scenarios there. The family rides a successor contract bundle after 0.18.0.

## Order

Start and profile, cgroup tree, lease, stop by owner, listing, logs and deadlines first, with the
event stream before any caller's watcher consumes it; then container and cluster records; then the
one-time adoption of survivors.

## Out of scope

Scheduling policy, product policy, restart or health supervision, and any profile between `host`
and exec's isolation floor. The client command-line tool over the socket is separate work.
