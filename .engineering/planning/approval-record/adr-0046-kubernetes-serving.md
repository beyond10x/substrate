---
format: aep.planning-md/3
id: approval-record:adr-0046-kubernetes-serving
kind: approval-record
status: active
title: Operator accepts ADR 0046 with every recommended option (K1-K9)
relations:
- decides: story:node-bound-kubernetes-serving-profile
revision: 2
transitions:
- {from: "draft", to: "active", at: "2026-10-28T15:49:11Z", actor: "human:timo", revision: 2}
---
# Decision: accept ADR 0046 with every recommended option

Taken 2026-10-28 by the operator. The orchestrating session relayed the answer verbatim:
"A" to the option "accept ADR 0046 with all nine recommendations". This answers
`decision-blocker:adr-0046-kubernetes-serving`.

| question | choice | what it fixes |
|---|---|---|
| K1 | A | callers address one pod by its per-pod DNS name under the headless Service; no other Service selects the pods |
| K2 | A | one certificate listing every ordinal's per-pod name, in one Secret; no SAN resolves to more than one pod |
| K3 | A | an init container copies the TLS and Identity CA material into a memory-backed `emptyDir`; a rotation restarts the pods |
| K4 | A | a dedicated namespace at `enforce: privileged`, `warn`/`audit: baseline`, and a chart test on the pod spec |
| K5 | A | default deny both ways; egress per destination with an explicit `cidr` and `port` |
| K6 | A | an OCI chart on GHCR, write-once, keyless cosign, `GITHUB_TOKEN` only |
| K7 | C | `actions/attest-build-provenance` stored as an OCI referrer of the chart digest |
| K8 | A | `story:operational-health-and-telemetry` first; `httpGet` probes on `/livez` and `/readyz` |
| K9 | B, fallback C | `chart-lane.yml` of its own; a recorded local lane only for scenarios a hosted runner cannot run |

`adr/0046-kubernetes-serves-one-node-bound-daemon-per-stable-pod-address.md` is accepted with
§ *Decision* unchanged. The story's `depends_on` edge to `story:operational-health-and-telemetry`
already exists (revision 6).
