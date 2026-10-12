---
format: aep.planning-md/3
id: story:bundle-publishes-read-only-roots
kind: story
status: draft
title: The published bundle states read-only roots
relations:
- decomposes: epic:host-run-gates
revision: 1
---
# Story: the published bundle states read-only roots

## Problem

The wire crate accepts `read_only_roots` on exec start (`ExecStartInput::read_only_roots`,
`crates/substrate-wire/src/lib.rs`) and the host binds them (ADR 0010), but no released bundle
through `0.18.0` publishes the field: it is absent from the exec-start input schema, from the
applied-confinement schema, and from the capability snapshot, which states no root bound. A
consumer that reads only the bundle cannot know roots exist. Found while adding `read_only_files`
to `0.18.0`, which does publish its field, fact and observation.

## Contract

A successor bundle adds `read_only_roots` to the exec-start input schema, the applied-confinement
observation and a capability fact carrying `MAX_READ_ONLY_ROOTS`, each bound to the wire crate's
constants and checked by the successor's additions check. Additive; ADR 0010 already decides the
behaviour.

## Acceptance

- The successor bundle's exec-start schema accepts the roots the wire crate accepts and rejects a
  list over `MAX_READ_ONLY_ROOTS`.
- The capability snapshot schema states the served roots bound, and the daemon publishes it.
- The applied-confinement schema lists every bound root, and a runtime vector shows it.
