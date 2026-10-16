---
format: aep.planning-md/3
id: review-result:wave-2026-10-16-u1-adversary-pass-2
kind: review-result
status: active
title: Adversary pass 2, unit u1 (client CLI)
relations:
- reviews: story:a-client-cli-drives-the-daemon
revision: 1
---
fixed

Adversary pass 2 on the client CLI unit, against e019f866; cases in `tests/adversary_pass2.rs`. Verdict needs-change.

Red evidence: each of the five named scenarios failed under a one-line mutation in a scratch copy (machine facts emptied; renew sent half the TTL; the unserved refusal class renamed; the exit code shifted by one on the delegated lane; the refusal code dropped from the human line; the `grace-ms` long renamed).

Findings:
- **G1, fixed.** `events --limit 0` or `1001` exited 1 as `unavailable/protocol` (`events_limit_out_of_range_is_a_usage_error`, red); the argument is now bounded to `1..=MAX_EVENT_PAGE_ITEMS`, a usage error with exit 2.
- **G2, fixed.** The comparison read only `is_required_set`, so a conditional requirement on a binding-optional argument passed (`required_mutant_a_conditional_requirement_is_seen`, `required_mutant_required_unless_present_is_seen`, red); it now refuses an argument with a conflict, override, requirement, conditional requirement or conditional default, and a group that requires or excludes. clap exposes these only through `Debug`; `r_ifs_all` and `r_unless_all` are absent from it and not seen.
- **Probe, green.** Events paging, an empty page and a bad cursor; operation show; metrics; session show and signal, unknown and live; the stated defaults of limit, grace and exec bounds; a dropped or added default.

Fix: 0ff355a4. Both lanes 36 passed, 0 failed.
