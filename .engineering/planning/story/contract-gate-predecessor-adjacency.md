---
format: aep.planning-md/1
id: story:contract-gate-predecessor-adjacency
kind: story
status: implemented
title: The contract gate refuses a bundle that names a non-adjacent predecessor
summary: compatibility.predecessor is read from the bundle and never checked against released order, so an older predecessor hides every route added since it.
relations:
- decomposes: epic:release-hardening
revision: 4
---
# Story: The contract gate refuses a bundle that names a non-adjacent predecessor

## Outcome

A successor bundle cannot make earlier routes invisible to the compatibility check by declaring an
older version as its predecessor.

## Context

`check_compatibility` reads `compatibility.predecessor` out of the bundle's own `bundle.json`
(`xtask/src/bundle.rs:203-206`) and joins it straight onto `inputs.contracts_root`
(`:216-219`). Nothing anywhere requires that string to name the adjacent released version:
`grep -rn 'compatibility/predecessor\|"predecessor"' xtask/src/bundle.rs` returns exactly one
production site, `:204`. The per-version dispatch in `check_additions` (`:383-400`) branches on the
bundle's *own* version and never on its predecessor; `PREDECESSOR: &str = "0.7.0"` at `:1258` is a
constant used by the file's own cases, not a released-order assertion.

So a `0.9.0` that declares `predecessor: "0.5.0"` is compared against `0.5.0`'s inventory. Every
route introduced in `0.6.0`, `0.7.0` and `0.8.0` becomes invisible to the drop check and — after
`story:contract-gate-sees-route-paths` — to the moved-path check as well. The counts are not a
barrier: the bundle authors its own `adds_routes` and `preserves_routes`, so it declares the
inflated numbers and they agree.

## Provenance

Found by `engineering-protocols:adversary` on 2026-08-30 while attacking
`story:contract-gate-sees-route-paths`, reported as finding F6 and marked pre-existing — it is not
caused by that change. The two `file:line` claims above and the `grep` result were re-checked by the
coordinator against `wave/2026-08-30-byte-plane`, which does not carry that change.

**Not observed as an exploit.** No bundle in `contracts/substrate-wire/` declares a non-adjacent
predecessor, and no test drives one. What is verified is the *absence of a check*, not a failure of
one; nobody has rendered a bundle that takes this route.

## Acceptance

A bundle whose `compatibility.predecessor` is not the released version immediately below its own
fails `cargo xtask check-bundle`, by name, and the failure says which version it named and which one
it should have named.

Evidence that satisfies it, in order:

1. A failing-first case in `xtask/src/bundle.rs`'s existing `mod tests`: a scratch successor
   declaring a two-step-back predecessor, refused by `check`, with the message naming both versions.
2. A second case driving the same bundle through `run()` — the acceptance names
   `cargo xtask check-bundle`, and a unit test on `check` is not that verb.
3. The adjacency rule is derived from what is on disk under `contracts/substrate-wire/`, not from a
   hard-coded table, so cutting `0.9.0` needs no edit here.
4. `0.5.0` through `0.8.0` still verify unchanged, and `git status --porcelain contracts/` is empty
   (invariant 6).

## Open Questions

`0.1.0` has no predecessor. Whether the first bundle is exempt by version or by the absence of any
lower directory is undecided; both `0.2.0` and `0.3.0` pin `0.1.0` in their Python checkers
(`scripts/check-contract-bundle-0.2.0.py:1528`), which suggests the rule already exists in prose for
the old bundles and was never carried into the Rust verb.
