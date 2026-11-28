---
format: aep.planning-md/3
id: review-result:weft-gap-7-served-by-named-apertures
kind: review-result
status: active
title: Consumer gap 7 is served by operator-declared apertures a run names
summary: 'Gap 7 of weft needs no new work: ADR 0013 and bundle 0.21.0 apertures list.'
owner: substrate
relations:
- reviews: epic:long-lived-service-on-the-host-driver
- supersedes: review-result:weft-gap-7-is-served-by-named-apertures
revision: 1
---
# Review result: consumer gap 7 is served by operator-declared apertures a run names

## Request

Consumer gap 7 of the `weft` repository (`cloudspec` `docs/platform-ports.md` § 10, commit
`2ed72c2`): "Egress destinations a consumer names per run, within bounds the operator sets."
Check named by the consumer: "a start request names one or more operator-declared apertures; a
destination outside them is refused by name."

## Finding

Already served; no new work.

- ADR 0013: an aperture is operator-declared and a request names it; a destination is never in a
  request.
- Bundle `0.21.0` added the `apertures` list on the exec start; bundle `0.27.0` bounds it to
  `MAX_EGRESS_APERTURES` (8) per run and `MAX_DEPLOYMENT_EGRESS_APERTURES` (12) per deployment.
- An undeclared name is refused `exec.aperture-undeclared` (422); the delegated lane of
  `crates/substrate-daemon/tests/runtime_vectors.rs` runs a start with two apertures and refuses an
  undeclared one.
- The consumer that raised the matching request accepts this model, provided `weft` can pass an
  aperture name.

## Follow-up

The workload start of `story:host-driver-supervised-workload` carries the same `apertures` list.
