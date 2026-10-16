---
format: aep.planning-md/3
id: review-result:wave-2026-10-16-u1-adversary-pass-1
kind: review-result
status: active
title: Adversary pass 1, unit u1 (client CLI)
relations:
- reviews: story:a-client-cli-drives-the-daemon
revision: 1
---
fixed

Adversary pass 1 on the client CLI unit, against a0f9e316. Verdict needs-change.

Findings:
- **F1, blocker, fixed.** The human rendering of `exec run` swallowed a refusal recorded on an exited exec; it is now printed on stderr.
- **F2, blocker, fixed.** The binding test accepted eight mutants of the command line; the comparison now lives in `tests/common`, shared by `cli_binding_validates` and `tests/binding_mutants.rs`, and compares per leaf aliases, sources, required-ness and enum values.
- **F3, fixed.** Under `--output json` a usage error printed clap's text; it is now one `cli_input` envelope with exit 2.
- **F4, fixed.** An empty `SUBSTRATE_SOCKET` counted as a transport error; it now counts as unset.
- **F5, fixed in the story.** The story named `--json`; the binding's global is `--output json`.
- **F6, accepted.** The exec bounds are recorded on the observation, not in `error.data.code`; documented in the guide.
- **F7, accepted.** Exit 1 is shared by a refusal, a transport error and the command's own status 1; documented in the guide.

Fix: e019f866. Holding required-ness to the binding showed that `events --limit` and `session signal --grace-ms` carry CLI defaults the binding stated as required; both became optional in `spec/client/client.yaml`, regenerated.
