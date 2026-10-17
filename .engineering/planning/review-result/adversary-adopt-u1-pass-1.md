---
format: aep.planning-md/3
id: review-result:adversary-adopt-u1-pass-1
kind: review-result
status: active
title: Adversary pass 1, unit u1 (workspace.adopt)
relations:
- reviews: story:exec-in-an-adopted-directory
revision: 1
---
# Adversary pass 1, unit u1 (workspace.adopt)

Agent type `aep:adversary`. Branch `unit/2026-10-17-u1-adopt`, head `36a56546` (never compiled at review time). Eight cases in `crates/substrate-daemon/tests/adopt_adversary.rs` (`4b991782`). Verdict: NEEDS-CHANGE; fixed in `4f1d8d8f`.

A blocker workspaces.rs:1302,1322 PendingWorkspaceDestroy literals miss `origin` (E0063).
B blocker workspaces.rs:308-317 held-name refusal records resource ws_… + workspace-kind refused event; absent-dir refusal records NULL/operation kind -> cross-subject oracle via /v1/ops, /v1/events. Fix resource: None. Case adopt_ledger_record_is_no_cross_subject_existence_oracle.
C warning workspaces.rs:331-388 + store workspaces.rs:189-203: held name skips observation; holder freed before reserve -> Ready row unobserved, 201 for absent dir. Fix: always observe before reserve, or refuse when holder changed. Case adopt_never_admits_a_name_freed_after_its_holder_was_read_without_observing_it.
D warning wire lib.rs WorkspaceOrigin Copy + &self methods -> trivially_copy_pass_by_ref.
E note ADR 0035 table: non-string root_name refused at address input, not root_name.

Fixes: A origin field added; B held-name refusals record through one unbound-refusal path (no resource, operation-kind event), so they cannot be told apart from an absent directory; C the reservation answers from the holder read when the holder changed (narrower than always observing, keeps the declared refusal order); D by-value self; E ADR 0035 table split into input and root_name rows.
