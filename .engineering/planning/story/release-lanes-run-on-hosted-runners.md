---
format: aep.planning-md/3
id: story:release-lanes-run-on-hosted-runners
kind: story
status: draft
title: The four release lanes run on GitHub-hosted runners, and a release reads their records
summary: release-lanes.yml runs the confinement, quota, chart node-profiles and namespace lanes on hosted runners; release.yml takes each record from its successful run when the tag carries none
relations:
- informed_by: decision-blocker:namespace-lane-runs-on-a-workstation
- informed_by: story:project-quota-lane-has-a-provisioned-runner
- informed_by: story:chart-lane-local-execution-record
- informed_by: story:ci-runs-the-full-gate
revision: 1
---
<!-- Starting point for a `story` artifact, seeded by `protocol artifact new story <name>`.
     No frontmatter here on purpose: the `---` block is written by the CLI from the id, kind, status
     and relations you gave it, and a second copy in this file would be the one that went stale.
     Delete the italic guidance as you fill each section. -->

# Story: <name>

## Outcome

*What is true for whom once this has shipped, in one sentence. If it names a component rather than a
person, it is a task — say what changes for someone.*

## Context

*Why this is worth doing now, and what it depends on. Link the epic or specification it comes from
rather than restating it; the `derived_from` relation already carries the edge.*

## Acceptance

*The conditions under which this is done, each one something a person or a check can observe. "Works
correctly" is not one of them.*

## Out of Scope

*What a reasonable reader would expect to be included and is not — the boundary that stops this
story quietly becoming an epic.*

## Open Questions

*What is still undecided, each with who decides it. A story carrying an unowned question is a story
that stalls without anybody noticing.*
