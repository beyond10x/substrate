---
title: Release notes
description: What changed in the newest Substrate release, generated from CHANGELOG.md.
---

# What changed in the newest release

This page ends with the newest released section of `CHANGELOG.md`, and `cargo xtask check-docs`
fails when the two differ. Earlier releases are in the
[full changelog](https://github.com/beyond10x/substrate/blob/main/CHANGELOG.md). The
[confinement adapters](./concepts/adapters.md) page explains each adapter the notes name.

What 0.16.1 first publishes is listed under 0.16.0 in the full changelog and summarized on the
[status page](./status.md#new-in-0161), including the namespace driver's limits.

<!-- Generated from CHANGELOG.md by `cargo xtask docs-releases`. Edit CHANGELOG.md, not the text below. -->

## 0.19.2 (2026-11-04)

Fixes only; the wire, the contract bundle (`substrate-wire/0.26.0`) and every published route are
unchanged. A bare exec refusal from the Kubernetes namespace driver keeps a denied `create` named
when the following access review fails, a refused egress aperture install reaps its forwarder
through an interrupted wait, and the namespace lane settles a workspace create whose outcome is
unknown.

### Fixed in 0.19.2

- When the Kubernetes namespace driver's `SelfSubjectAccessReview` denies `pods/exec` `create` and
  the following `get` review is refused, fails, answers no `allowed` or does not answer within the
  5 s bound, the bare exec refusal names the `create` denial before it says the refused verb is
  unknown, and keeps the API's answer (design 34 § 7.1).
- A refused egress aperture install reaps its abandoned forwarder even when a signal handler
  installed without `SA_RESTART` interrupts the wait, so no refused install leaves a zombie.
- `cargo xtask namespace-lane observe` settles a workspace create answered 500
  `operation.outcome-unknown` in its same-named-object and foreign-uid cases by reading the
  operation to its end, as design 01 § 6 asks, instead of failing the case and leaving the
  workspace's keeper Pod and claim behind.
- `cargo test -p b10x-substrate-daemon --test tls_listener` no longer fails
  `adversary_dispatch_counts_one_admission_and_no_replay` with `Connection reset by peer (os error
  104)`. The test's readiness wait now retries a connect the starting daemon resets, as it already
  did for a refused connect, within the same readiness deadline.
- `security_review_systemd_linux_run_reaches_no_user_manager_and_keeps_the_floor` passes on a host
  that has a tunnel module such as `ipip` loaded. That module puts a down, unaddressed fallback
  device (`tunl0`) in every new network namespace. The case now reads each interface's state and
  tolerates such a device only while it is down and not up. Any other interface, or a fallback
  tunnel that is up, still fails it. `scripts/delegated-lane.sh` runs this case, so the
  `Confinement-lane:` record a release needs can be made on such a host.

### Security in 0.19.2

- `pulldown-cmark` is 0.13.6, taking the fix for
  [RUSTSEC-2026-0433](https://rustsec.org/advisories/RUSTSEC-2026-0433.html), quadratic parsing of
  nested image wikilinks. Only `xtask` depends on it, and `check-links` parses the repository's own
  Markdown with `Options::all()`, which turns on `ENABLE_WIKILINKS`. No shipped crate is affected.
  `THIRD_PARTY_LICENSES.html` names the new version.
