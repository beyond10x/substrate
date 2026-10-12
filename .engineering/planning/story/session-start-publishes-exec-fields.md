---
format: aep.planning-md/3
id: story:session-start-publishes-exec-fields
kind: story
status: draft
title: Session start publishes the exec fields exec start does
relations:
- decomposes: epic:host-run-gates
revision: 1
---
# Story: session start publishes the exec fields exec start does

## Problem

The `pipe-session-start` input schema's `exec` object, through bundle `0.18.0`, lacks `scratch`,
`measurements` and `workspace_access`, which the exec-start schema carries and the wire crate's
start input accepts on both routes. A bundle-only consumer cannot request them on a session, or
cannot tell whether a session honours them. Found while adding `read_only_files` to both start
schemas in `0.18.0`.

## Contract

A successor bundle states the three fields on the session-start `exec` object exactly as on exec
start, or, for any field a session does not serve, the daemon refuses it with a named code and the
bundle says so. Which of the two holds per field is read from the daemon first.

## Acceptance

- For each of `scratch`, `measurements` and `workspace_access`, the session-start schema and the
  daemon agree: accepted and honoured, or refused before dispatch with a named, published code.
- A runtime vector exercises each field on the session route.
