---
format: aep.planning-md/3
id: review-result:design-34-u10-security-pass-1
kind: review-result
status: active
title: Security review of design 34 U10, the probe, facts and daemon wiring, pass 1 (1 warning, 1 note)
relations:
- reviews: story:kubernetes-namespace-workspace-and-exec-slice
revision: 1
---
# Security review of design 34 U10, the probe, facts and daemon wiring, pass 1

Reviewed `wave/k8s-namespace-1` at `4f50de0c` (U10 unit commit `8980ad44`, and the merge's
resolution that the probe reads its log through `read_owned_log`). Red case: `7057d880` on
`sec/u10-daemon-wiring` (`crates/substrate-kubernetes/src/namespace/probe_tests.rs`, +65, test file
only). Crate suite 69 before, 70 after, 1 red.

Verdict: NEEDS-CHANGE. No blocker, 1 warning, 1 note.

## Findings

```findings
- {file: crates/substrate-kubernetes/src/namespace/probe.rs, line: 105, category: concurrency, severity: warning, verdict: NEEDS-CHANGE, origin: introduced, message: "a probe in flight across a lost and retaken Lease writes its proof after forget_probe, so it is published under the new Lease and step 7 does not run again"}
- {file: crates/substrate-kubernetes/src/lib.rs, line: 190, category: contract-drift, severity: note, verdict: CONFIRMED, origin: introduced, message: "workspace.adopt-unserved fires only for a free root name; a held name answers 404 or 409, so 501 against 404 shows whether another subject holds the name"}
```

## Red case

- `namespace::probe_tests::a_probe_in_flight_across_a_lost_and_retaken_lease_does_not_stand_for_step_seven`:
  a retaken Lease does not run step 7 again; the probe begun under the lost Lease stands for it.

## Cleared

- The probe reads its log through `read_owned_log`, with a uid GET before and after, after `ended`
  matched the uid.
- Every proved fact needs the binding generation, the Lease and `valid_until`; bounds need a probe
  that passed the handshake. No Linux fact and no `confinement.no-egress` is published; no fact
  names the driver.
- Probe Pod: images by digest, restricted security context, read-only workspace mount, no
  service-account token, uid-preconditioned deletes, leftovers removed first.
- `--driver kubernetes`: clap conflicts, `config.driver-conflict` and `kubernetes.config-invalid`
  before anything listens; no host fallback.
- Admission is one predicate; the capsule refusal follows it, before dispatch. The subject comes
  only from peer credentials.
- The deployment guide matches the code.

## Not verified

Clap conflicts on values set through `SUBSTRATE_*` environment variables; token redaction in
kubeconfig parse errors (U1/U6 code); any kind or cluster run.

## Disposition

Both go to fix unit N: the probe records the Lease epoch it ran under and a proof from another
epoch is dropped, as a stale generation is; every adopt answers `workspace.adopt-unserved` (501)
before any lookup, as design 34 § 8 states.
