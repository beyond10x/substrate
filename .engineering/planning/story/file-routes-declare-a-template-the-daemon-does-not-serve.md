---
format: aep.planning-md/1
id: story:file-routes-declare-a-template-the-daemon-does-not-serve
kind: story
status: active
title: The file routes are served at a template the contract cannot declare
summary: The daemon registers a wildcard file path and every bundle declares a single-segment one; the renderer cannot express the wildcard, and render.rs is frozen by generator.digest.
relations:
- decomposes: epic:release-hardening
revision: 3
---
# Story: The file routes are served at a template the contract cannot declare

## Outcome

`operations.json` names the template the daemon actually registers, and a client can address every
nested path the contract's own schema admits.

## What is verified

Four facts, each read from the tree at `wave/2026-08-30-byte-plane` by the coordinator:

| | |
|---|---|
| the daemon registers | `/v1/workspaces/{workspace_id}/files/{*path}` — `crates/substrate-daemon/src/app/routes.rs:42`, and the `/v2/` form at `:48` |
| every released bundle declares | `/v1/workspaces/{workspace_id}/files/{path}` — checked in `0.4.0` and `0.8.0` |
| the renderer cannot render the daemon's form | `path_parameters` (`xtask/src/render.rs:653-662`) strips `{` and `}` and nothing else, so `{*path}` yields a property literally named `*path` and a `$ref` into `common.json` under that name, which resolves to nothing |
| the schema expects nesting | `common.json` `$defs/relative-path` is a string with `maxLength: 4096` and `x-b10x-max-depth: 64`, whose pattern forbids a *leading* slash and any `..` segment — and therefore admits interior slashes |

A matchit `{param}` matches one segment. So the contract declares a template that cannot express what
its own schema admits: `a/b/c.txt` is a valid `relative-path`, the daemon serves it, and the registry
says the route takes a single segment.

## Reported, not reproduced here

From the fifth adversarial pass on `story:contract-gate-sees-route-paths`, driving real matchit
0.8.4: a request for `/v1/workspaces/w1/files/a/b/c.txt` matches under `{*path}` and does not match
under `{path}`; and `check_classification` never notices the dangling reference, because address
documents are registered and never compiled.

## Why it cannot simply be corrected

`xtask/src/render.rs` is effectively frozen: every released `bundle.json` records that file's sha256
as `generator.digest`, so a change to it stops all eight released bundles being fixed points of their
own sources at once. Teaching `path_parameters` about `*` is therefore not a one-line fix — it has to
land together with whatever admits the released bundles afterwards, and that decision is not made
anywhere in the tree.

Until then the gate refuses a wildcard segment by name — added under
`story:contract-gate-sees-route-paths`, which found this. That refusal is a placeholder for this
story, not a resolution of it.

## Acceptance

`operations.json` declares the template the daemon registers, a client can address a nested path
through the published address schema, and `cargo xtask check-bundle` verifies every released bundle
unchanged.

Evidence that satisfies it, in order:

1. A recorded decision on how `generator.digest` survives a renderer change — the options visible
   from here are pinning each released bundle to the renderer it was cut with, or re-recording the
   digests as a deliberate one-off with the old bytes proven identical. Neither is chosen anywhere,
   and this is the real content of the story.
2. `path_parameters` handles a wildcard segment, generating a property under the bare name bound to
   `relative-path`; written failing-first against the malformed document it produces today.
3. `check_classification` compiles address documents, so a dangling reference in one is a failure —
   that blind spot is what let this stay invisible.
4. A successor bundle declaring the wildcard form, with the change from the single-segment template
   treated as the widening it is: every URL the old template answered, the new one answers, at the
   same operation.
5. The `v2` route at `routes.rs:48` is covered by the same change or explicitly deferred.

## Provenance

Found by `engineering-protocols:adversary` on the fifth pass over
`story:contract-gate-sees-route-paths`, 2026-08-30, while establishing that a false refusal in that
unit was reachable. The four verified rows above were re-read by the coordinator; the matchit
behaviour and the `check_classification` blind spot are the adversary's, quoted.

**Not observed as a live client failure.** Nobody has reported a nested-path request failing. What is
established is that the registry and the router disagree, and that the request hash `hashing.json`
normalises against the registry template cannot match the URLs the daemon serves.
