---
format: aep.planning-md/3
id: story:docs-shared-site
kind: story
status: draft
title: The public site runs on the shared documentation layer
revision: 1
---
## Outcome

The public site's documentation pages render on the shared docs-system layer, set up as the
harness repository's site: Mermaid diagrams render as diagrams, code is highlighted, and the
adapter support tables show yes, probed and no as the shared check, question and cross chips. The
getting-started page leads a first reader to one confined command.

## Decision

Operator, 2026-10-27: use the shared documentation UX like the other project sites. Keep the start
page's look (hero and the rest of `website/index.html`); only the header may change. Keep the
structure, navigation, layout and page tree.

## Acceptance

1. Same pages, sidebar groups and order, paths and heading anchors; every route and anchor in the
   old `.well-known/b10x-routes.json` is in the new one.
2. The start page looks the same, shown by a before/after comparison.
3. `concepts/operations.md` and `concepts/model.md` diagrams render.
4. `concepts/adapters.md` tables use `TRUE`, `UNKNOWN` and `FALSE`; `cargo xtask check-docs` holds
   them to the specification.
5. `.well-known/b10x-site.json` and `b10x-routes.json` are still published; `pages.yml` builds the
   site; no `b10x.docs.yaml`.
6. Getting started: install, a dry run, a confined run and one named refusal; the daemon walkthrough
   moves to an existing guide.
