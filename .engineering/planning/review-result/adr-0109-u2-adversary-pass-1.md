---
format: aep.planning-md/3
id: review-result:adr-0109-u2-adversary-pass-1
kind: review-result
status: active
title: Adversary pass 1 on story:confinement-domain-specification
relations:
- reviews: story:confinement-domain-specification
revision: 1
---
# Adversary pass 1: story:confinement-domain-specification

Verdict: needs change. Unit commit 3bca3a1, read at integration a086e93. Four probe scenarios
written first and red against the interpreted reference: 62 scenarios, 58 passed, 4 failed.

## Findings

- F1 (blocker, `spec/confinement/confinement.yaml:285`, `spec/confinement-suite.json:11`): guards
  read the opened row's facts, and `local` can be opened without `declared_insecure`, so the
  synthesized `AdmitProfile/outcome/admitted` admits `local` with no reason and
  `declared_insecure: false`, against ADR 0044 decision 3 and atlas ADR 0109 decision 4. Probe:
  `local-without-declared-insecure-fact-reason-missing-on-portable` expected
  `local-reason-missing`, observed `admitted`.
- F2 (warning, `confinement.yaml:298`, `spec/confinement/README.md:32`): the image check is a
  substring test for `@sha256:`; `registry.invalid/run@sha256:` and
  `registry.invalid/run@sha256:latest` are admitted on `oci` and `pod`.
- F3 (warning, `confinement.yaml:241-243, 499`): a profile requesting `systemd` around `linux` on a
  scope opened around `local` is admitted and recorded `inner: linux`.
- F4 (warning, the five `*-adapter-unserved-*` scenarios): absence scenarios carry the adapter
  prefix the adapter stories select by, so an adapter's own lane would select the scenario
  asserting it is absent.
- F5 (warning, `spec/confinement-suite.json`, `CHANGELOG.md:12`): the 22 synthesized scenarios
  name no lane or adapter; the CHANGELOG claims lane tags ESS 0.70.0 scenarios cannot carry.
- F6 (warning): `executable-system-specification:confinement` does not exist.
- F7 (note, `confinement.yaml:245-249`): no `ProfileRefused` audit event; ESS refuses events on
  error outcomes.

## Checked and not broken

Every family and adapter variant has its admission and refusal; five absence scenarios in the
right lanes; refusal order holds for a reserved field beside a missing family; the declared linux
and macos facts match `probe.rs:276,882,537` and `seatbelt.rs:141`; ADR 0044 cites atlas ADR 0109
and ADR 0037 and states that `oci` and `pod` are not plan 03's drivers; `contracts/` and
`crates/substrate-wire/` unchanged; `check_confinement` compares suite and types byte for byte.
