---
format: aep.planning-md/3
id: review-result:wave-2026-11-04-substrate-u1-adversary-pass-1
kind: review-result
status: active
title: Wave 2026-11-04 substrate U1 adversary pass 1
relations:
- reviews: story:failed-get-review-keeps-the-create-denial
revision: 1
---
# Adversary pass 1 — wave 2026-11-04 U1 (story:failed-get-review-keeps-the-create-denial)

Diff `a692acb7...2c2e5c49` (unit commit `caf551f8`); adversary commit `993bd76f` on `adv/2026-11-04-u1`, merged into the integration branch at `688eb58e`. Agent `aep:adversary`. Header and findings verbatim; prose trimmed to the measured facts.

```
verdict: nothing found
cases: executed 23→25, red 0
origin: introduced 0, pre-existing 0, undecided 0
```

Commands: `cargo test -p b10x-substrate-kubernetes --lib failed_get_review_adv` 2/0; `… --lib verb_review_tests` 25/0; `cargo fmt --all --check` exit 0; `cargo clippy -p b10x-substrate-kubernetes --lib --tests -- -D warnings` exit 0.

Cases (`verb_review_tests.rs`, `mod failed_get_review_adv`):

- A failing `get` review after a denied `create` ends in the exact expected message, with the bearer token shown as `[redacted]` and the API's message cut to 300 characters.
- An admitted `create` before a failing or late `get` review names no denial.

Mutants, run on a scratch copy of the tree, each failing the existing suite:

| mutant | tests failed |
|---|---|
| drop the denial on the failure path | 10 |
| swap the order | 9 |
| lose the denials when the 5 s bound passes | 2 |
| add an empty denial prefix when nothing was denied | 5 |

Design 34 § 7.1 and the 2026-11-04 amendment match the code.

```findings
[]
```
