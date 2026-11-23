---
format: aep.planning-md/3
id: story:public-text-states-the-release-and-what-shipped
kind: story
status: draft
title: The site, STATUS.md and the README state the current release, what shipped and who owns what
summary: Public pages named 0.16.1 as current, listed shipped Kubernetes capabilities as absent, and the README gives the Connectors relationship the wrong way round
tags:
- defect
revision: 2
---
# Story: the site, STATUS.md and the README state the current release, what shipped and who owns what

## Outcome

An adopter who follows the public pages installs a CLI that speaks to the current daemon, reads
which capabilities shipped, and reads which repository owns which side of an integration.

## What was seen

A review of release 0.24.0, findings 12, 13, 14 and the Substrate side of 9:

- The quickstart installed CLI 0.16.1, whose `machine` command exits 1 `contract_mismatch`
  against a 0.24.0 daemon. The contract overview named `0.23.0` as newest, and the contract
  reference said no release advertises `substrate-wire/0.27.0`. These three are corrected by
  round R-02 of the site's UI loop (finding UXF-009), which also makes `check-docs` hold them.
- `STATUS.md` in the repository root calls 0.16.1 the latest release.
- The Linux-only example of the quickstart uses a job directory that only the Docker
  instructions create.
- The status page and the deployment guide list the shared conformance runner (shipped in
  0.17.0) and command execution on the namespace driver (0.18.0, on clusters that satisfy its
  requirements) as absent or as following in the next release.
- The README states the Connectors relationship in the wrong direction. Atlas ADR 0093:
  Connectors owns the provider declaration and the governance of admitted operations, Substrate
  owns daemon enrollment and execution, and the integration is intended, not delivered.
- A daemon upgraded alone to 0.24.0 fails the handshake of an SDK pinned to an earlier bundle:
  the SDK compares the bundle name and its digest. The pages do not tell an adopter to upgrade
  the daemon and its SDK clients together.

## What changes

The pages named above. `check-docs` holds `STATUS.md`'s release line like the site's. The
deployment guide and the release notes state that a daemon and its SDK clients move together
while the contract is in development.

## Acceptance

`cargo xtask check-docs` fails on a `STATUS.md` that names another release than the workspace
version. The quickstart's Linux example runs as written on a host that never ran the Docker
steps. The status page lists neither shipped capability as absent.

## Where the requirement comes from

## Where the requirement comes from

A review of release 0.24.0 dated 2026-11-23, findings 9, 12, 13 and 14, which the operator
handed to this session on 2026-11-23 with the order to address every finding. Stakeholder: the
operator.

The findings of that review that are not Substrate's:

- Finding 9, consumer side: `mantle` and `workspace` pin an SDK that expects an earlier bundle
  and need a paired upgrade; filed in those repositories.
- Finding 15 (`atlas`) and finding 16 (`agent-stack`): filed in those repositories.
- Findings 10 and 11, and `conductor`'s part of 9: not filed. The operator said on 2026-11-23
  that `zwirn` and `conductor` are retired. `zwirn` was archived on the forge that day;
  `conductor` is retired and not archived.
