---
format: aep.planning-md/3
id: story:sdk-workspace-builder-takes-the-last-source
kind: story
status: draft
title: The SDK's workspace builder takes the source selected last
summary: empty() after git() leaves the Git source set, and the create follows the Git path
tags:
- defect
revision: 1
---
# Story: the SDK's workspace builder takes the source selected last

## Outcome

On the SDK's workspace builder the last of `empty()` and `git(…)` decides what is created.

## What was seen

Read at release 0.24.0, not run: `empty()` in `crates/b10x-substrate-sdk/src/lib.rs` returns
the builder unchanged, so a Git source set before stays and `create` follows the Git path.

## What changes

`empty()` clears the source.

## Acceptance

An SDK unit case: `git(…)` then `empty()` builds the request of an empty workspace; `empty()`
then `git(…)` the Git one. Red before the change.

## Where the requirement comes from

A review of release 0.24.0 dated 2026-11-23, finding 7, which the operator handed to this
session on 2026-11-23 with the order to address every finding. Stakeholder: the operator.
