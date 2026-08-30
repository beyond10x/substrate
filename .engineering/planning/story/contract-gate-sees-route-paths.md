---
format: aep.planning-md/1
id: story:contract-gate-sees-route-paths
kind: story
status: draft
title: The contract gate refuses a renamed route path
summary: 'route_ids collects operation ids, never paths, so moving a route passes check-bundle as adds_routes: 0.'
relations:
- decomposes: epic:release-hardening
revision: 2
---
# Story: The contract gate refuses a renamed route path

## Outcome

A consumer that pinned a bundle can trust that the paths in it did not move. Today they can move
silently: the successor check reads operation **ids** and never the paths they are served at, so a
released route can be relocated and every gate step still passes.

## Context

Invariant 6 says every released contract bundle directory is immutable and a wire change adds a
successor bundle. `cargo xtask check-bundle` enforces that for the bundle's bytes, and
`check_additions` enforces that a successor drops no route:

```rust
for dropped in previous.difference(&current) {
    failures.push(format!(
        "operations.json: route {dropped} served by {predecessor} is absent; an additive \
         successor never drops one"
    ));
}
```

(`xtask/src/bundle.rs:263-268`.) The sets it differences come from `route_ids`, which collects
exactly one field:

```rust
.filter_map(|entry| entry.get("id").and_then(Value::as_str))
```

(`xtask/src/bundle.rs:534-536`.) So the property enforced is *no operation id disappears*. A
successor that keeps every id and moves `/v1/pipe-sessions/{session_id}/attach` to
`/v1/anything-else/{session_id}/attach` has an empty difference, reports `adds_routes: 0`, and
passes. **The gate cannot tell a path rename from a no-op.**

This is not hypothetical: `docs/design/16-sessions-are-not-pipe-sessions.md` proposes moving that
exact family, and had to give its aliases new ids precisely so the checker could see the change at
all. A design that wanted to move a path quietly would simply not do that, and nothing would
object.

Found while designing the `/v1/sessions` rename, 2026-08-30.

## Acceptance

A successor bundle that serves an existing operation id at a different path fails
`cargo xtask check-bundle`, by name, and the failure says which id moved and between which paths.

Evidence that satisfies it:

1. A failing-first test: take `0.8.0`'s authored source, move one route's path, leave every id
   intact, and assert `check-bundle` fails. It must pass before the change and fail after.
2. The existing seven bundles still verify unchanged — the check is added, no frozen byte moves
   (invariant 6).
3. A deliberate, declared path change is still expressible, because
   `docs/design/16-sessions-are-not-pipe-sessions.md` needs one. Whatever shape that takes — an
   `alias_of` field, an explicit moved-from record — the check reads it rather than being switched
   off for that version.

## Out of Scope

Removing routes. That needs a non-additive compatibility kind, which
`contracts/substrate-wire/0.8.0/schemas/bundle.json:21-22` does not have (`"const": "additive-v1"`)
and which is its own decision.

## Open

Whether the same blind spot exists for other route-adjacent fields — method, status codes,
`response_branches`. This story covers paths, because that is what was found; a sweep of what else
`route_ids` does not read would be worth doing while the code is open.
