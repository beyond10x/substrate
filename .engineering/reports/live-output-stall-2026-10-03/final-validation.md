# Bounded live output validation

Candidate: b8ae300fb0c7c5b92ed34f4f375b2c8a42bb7057. Production source last changed at 445d74a593db7b73729d7fc7345996ac921c9c14. Independent review and retained red/green tests are recorded alongside this report.

GitHub Full gate passed on the exact candidate (8m36s): https://github.com/beyond10x/substrate/actions/runs/37071689392. Documentation, security/privacy and CodeQL checks also passed.

Locally, all portable gate steps passed across an original run and a resumed run. The original run passed ESS/docs checks, workspace release tests (619 passed, 8 ignored) and formatting. Clippy then encountered tmpfs disk quota exhaustion. The idle task cache was moved to disk; Clippy and all remaining checks were run again or for the first time, in repository order, each exiting zero. This is not described as a single uninterrupted successful gate invocation. Immutable bundle checks covered 17 bundles and 3,908 JSON documents.

The final `bash scripts/delegated-lane.sh` run exited zero on the exact candidate. Host, public SDK managed/unrecorded/remote, MCP, WSS and runtime-vector lanes executed. Seven real-quota fixture cases and the unserved host resource-counter lane remained explicitly absent. These exclusions are not passed cases.

Candidate daemon SHA-256: `1a29c5e0a01b05772efda6eae399e7a8752604a14e2618f54e0931b121f10206`.

Mantle's independent synthetic transport proof used that exact daemon, current SDK contract 0.17 and the corrected production terminal module. Two real-PTY attachments each delivered 262,144 exact replay bytes, keyboard round-trip, detach/reconnect to the same agent and terminal restoration. Workspace destruction returned `operation.outcome-unknown`; subsequent lookup and independent checks established temporary resources absent. No existing worker was restarted, and authenticated Codex or installed wire-0.16 acceptance is not claimed by that proof.

Full local logs are retained in the coordinator's task scratch under `~/.cache/substrate-output-stall/`: `full-gate-tmpfs-quota.log`, `full-gate-resume-clippy.log`, `full-gate-resume-checks.log`, `gate-status.tsv` and `final-delegated.log`.
