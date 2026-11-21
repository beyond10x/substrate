---
format: aep.planning-md/3
id: story:raise-run-aperture-and-read-only-root-bounds
kind: story
status: active
title: A run selects up to eight apertures and binds up to eight read-only roots (#236)
revision: 5
transitions:
- {from: "draft", to: "proposed", at: "2026-11-20T06:56:37Z", actor: "human:unknown", revision: 3}
- {from: "proposed", to: "active", at: "2026-11-20T06:56:38Z", actor: "human:unknown", revision: 4}
---
# A run selects up to eight apertures and binds up to eight read-only roots (#236)

Issue #236, dispatch DSP-20261120-02 (conductor REQ-20261120-apertures). A GitHub instance's box
already selects four apertures (two forge hosts, the model host, the control-plane dial-in), the
whole `MAX_EGRESS_APERTURES`, so it cannot add package-registry egress (conductor #198); a third
tool root beyond `~/.cargo` and `~/.rustup` is near `MAX_READ_ONLY_ROOTS` (conductor #197).

Decided by the amendments of 2026-11-20 to `adr/0013-egress-apertures-are-declared-by-the-operator.md`
and `adr/0010-declared-host-roots-are-mounted-read-only.md` and published by bundle `0.27.0`.

## Outcome

- `MAX_EGRESS_APERTURES` (one run) 4 -> 8; `MAX_READ_ONLY_ROOTS` 4 -> 8.
- The deployment bound moves into the wire as `MAX_DEPLOYMENT_EGRESS_APERTURES`, 6 -> 12, and
  bounds the `exec.egress-apertures` fact list: `0.26.0` states that list's `maxItems` as the run
  bound (4) while a daemon declaring five or six published more, a fact its own contract refuses.
- Successor bundle `substrate-wire/0.27.0` adds no route and publishes the three bounds; bundles
  `0.21.0` to `0.26.0` keep their bytes, their sources state the old bound as a literal.
- The daemon and SDK advertise `0.27.0`; a client pinned to `0.26.0` must move with it.

## Acceptance

- Tests fail before and pass after: eight apertures and eight roots admitted, nine refused by
  wire validation, the SDK and the host; twelve declared apertures start a daemon, thirteen refused;
  the 0.27.0 bundle check binds each bound to its wire constant.
- Every released bundle stays the fixed point of its own source (`check-bundles 0.5.0 .. 0.27.0`).
- Spec: the ESS projections model no numeric bound on apertures or roots; `spec/` is unchanged.
- Full gate green; the pull request closes #236; a release is cut.
