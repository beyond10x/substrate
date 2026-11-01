---
format: aep.planning-md/3
id: approval-record:d338-review-findings-decided-in-session
kind: approval-record
status: active
title: 'D338: review findings become stories decided in the session, no triage sheet'
relations:
- informed_by: review-result:wave-2026-10-31-d337-adversary-pass-1
revision: 2
transitions:
- {from: "draft", to: "active", at: "2026-11-01T00:29:07Z", actor: "human:timo", revision: 2}
---
# D338: stories for review findings are decided in the session, without a triage sheet

## Approval

b10x-chef-orchestrator, 2026-11-01, D338, relaying Timo through host-monitor D53: "i am 100% fine
in stories being created for anything inside b10x". The order: no triage sheet for Timo; decide
the six D337 out-of-scope rows (adopt as story, decline with reason, or duplicate); the
2026-10-31 sheet's substrate rows 1, 13 and 18 are adopted, row 1 after design 34 is amended.

## Decisions on the D337 out-of-scope rows

Rows of `review-result:wave-2026-10-31-d337-adversary-pass-1` (immutable, so decided here).

| row | decision |
|---|---|
| S6b, 500 `operation.outcome-unknown` in 1 of 3 runs | adopted: `story:namespace-lane-same-named-case-creates-its-workspace` |
| `create_probe_object` tags every create failure `kubernetes.api-failed` | adopted: `story:probe-withheld-facts-name-the-failed-calls-code` |
| `verb_refusal` accepts any code anywhere in the log | adopted, with the INFEASIBLE note: `story:namespace-lane-verb-case-accepts-only-its-own-refusal` |
| `scheduled_node` and `ended` end a run with no `code=` | duplicate of `story:probe-withheld-facts-name-the-failed-calls-code` |
| `refuse_logs` answers `Forbidden` whatever the status, after the Pod-exists check | declined: the one caller passes 403 for an existing Pod, so neither difference changes a tested path; a test that needs another status or an absent Pod changes the double then |
| NetworkPolicy and heartbeat warnings do not use the "`<fact>` was not proved" form | duplicate of `story:probe-withheld-facts-name-the-failed-calls-code` |

## Sheet rows adopted

- Row 1: `story:namespace-no-egress-observed-on-every-node`, proposed; a design 34 amendment comes
  first.
- Row 13: `story:exec-end-after-await-never-panics`, draft to proposed.
- Row 18: `story:pod-orphaned-policy-is-swept`, draft to proposed.
