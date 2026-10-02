# Interactive capture selection

This ESS projection gives the immutable capture selection its typed home. The wire `CaptureMode`
is generated with `ess generate types`, and `cargo xtask check-ess` regenerates and compares it.
It is separate from the accepted-operation ledger projection so adding capture behavior cannot
silently change that suite's obligations.

The domain's selected event projects the durable session reservation. This is not a specification
of the full transport or a claim that a reference interpreter proved privacy. Production conformance
lives in `crates/b10x-substrate-sdk/tests/unrecorded.rs`, the host drain test, the store sink test,
and the existing session attachment/confinement suites. `scripts/delegated-lane.sh` runs the real
SDK/daemon journey. Without delegation it explicitly reports that journey absent.

Design 23 names the scenario set and the privacy boundary. Counters after restart are the last
persisted observations, with the execution reported unknown; they are not reconstructed final
counts. Queued bytes describe driver-channel delivery, not a client acknowledgment.
