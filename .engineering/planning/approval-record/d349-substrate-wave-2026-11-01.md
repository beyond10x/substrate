---
format: aep.planning-md/3
id: approval-record:d349-substrate-wave-2026-11-01
kind: approval-record
status: approved
title: 'D349: the pods/exec create case reads the API''s bare refusal'
revision: 2
transitions:
- {from: "draft", to: "approved", at: "2026-11-01T10:16:17Z", actor: "human:timo", revision: 2}
---
# D349: substrate wave 2026-11-01, the pods/exec create case reads the API's bare refusal

## Approval

b10x-chef-orchestrator, 2026-11-01, D349 (class C, under the grant): "A, now, as one small wave.
Record it in your store. Reason: the daemon log at 10:08:42Z and 10:09:43Z shows the API itself
sends an empty reason, which replaces the basis of D342; the exec is still refused at once.
1. Lane rule, S6 "Role without pods/exec create" only: accept a 403 executor-handshake-failed that
names the case's keeper Pod and carries no other verb's `cannot … resource` text. Keep the "get"
case strict, and add a check that the two cases stay distinct. 2. Store-only commits in the same
branch: file B (SelfSubjectAccessReview names the verb) as a proposed story with the log lines as
evidence, and close the two D342 stories that are implemented. 3. Version stays 0.18.1, since
nothing is tagged. Targeted tests, the repository's gate, a bot PR, ci-waits row, and merge on
green. 4. Then the lanes, as substrate's release process requires for the merge commit. [...] Tag
on green."

## Units

Integration branch `wave/2026-11-01-d349`, on `main` at `bb2b06fe` (PR 208, wave D342).

| unit | story | surface |
|---|---|---|
| U1 | `story:namespace-lane-exec-create-case-reads-the-apis-bare-refusal` | `xtask/src/namespace_lane_run.rs` (`verb_refusal`), `CHANGELOG.md` `[0.18.1]` |

Store only: `story:driver-names-a-bare-exec-refusals-verb` (proposed, B);
`story:refused-executor-handshake-names-the-api-answer` and
`story:chart-lane-stand-ins-never-fail-with-etxtbsy` move to implemented.

## Why

The 0.18.1 lanes at `bb2b06fe` (2026-11-01, 09:46-10:13Z): delegated, quota and chart green;
namespace gave no record on "S6 Role without pods/exec create" again. The daemon log the lane kept
(D342 U1) shows the handshake refused at once and the API's parsed `Status.message` whole:
`exec answered 403: pods "sbx-pr-gbyb7pajquihb757-keeper" is forbidden: ` (10:08:42Z) and the
same for `sbx-pr-fma7x5qkyobdhq4t-keeper` (10:09:43Z), with nothing after "forbidden: ". The
"pods/exec get" case at 10:08:31Z carried `cannot get resource "pods/exec"`. The lane rule's
required text is not in what this API sends.
