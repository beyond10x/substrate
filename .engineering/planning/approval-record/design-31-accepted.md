---
format: aep.planning-md/3
id: approval-record:design-31-accepted
kind: approval-record
status: active
title: Accept design 31 as recommended (D172)
relations:
- decides: story:remote-clean-room-conformance
revision: 2
transitions:
- {from: "draft", to: "active", at: "2026-10-29T01:51:52Z", actor: "human:timo", revision: 2}
---
# Decision: accept design 31 as recommended (D172)

Taken 2026-10-28 by the coordinating session as its D172, under the operator's standing grant,
relayed to this repository's session.

- **Q1:** C. A certification is signed with keyless `cosign sign-blob` under the certifier's OIDC
  identity; a private deployment signs key-based, without the transparency log. Lane reports in
  this repository stay unsigned. The runner holds no signing key.
- **Q2:** yes. An `incomplete` run exits with code 4 and keeps its report.
- **Q3:** the runner binary does not ship as a release artifact yet; that is a later story.

None of the three needs an ADR or changes a documented contract: design 31 § 8 states no wire byte
changes and no bundle is cut, and release artifact names are untouched.

Design 31 moves to accepted for implementation. `story:remote-clean-room-conformance` joins the
wave after `wave/adapter-io`.
