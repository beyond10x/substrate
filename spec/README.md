# Substrate executable specification

The first ESS projection describes the **accepted-operation ledger** implemented by the SQLite
store. It is derived from shipped code; it neither changes the runtime nor replaces the immutable
wire bundles. Read [coverage and limitations](coverage.md) before interpreting a green result.

The exact CLI pin is in `ess-inputs.yaml`: ESS 0.50.0. Install that official release, or use
`ess specify toolchain install 0.50.0` with an existing ESS CLI. CI installs its checksum-verified
Linux archive. There is no ESS implementation dependency in the Substrate workspace.

```console
ess specify validate --path spec
ess specify compile --path spec --format json
ess verify conform synthesize --path spec --out spec/operations-suite.json
cargo xtask check-ess
cargo test --locked -p b10x-substrate-store --test ess_operations -- --nocapture
```

`cargo xtask check-ess` regenerates into a temporary directory and compares exact bytes with the
committed suite. The full gate executes all 14 scenarios against the production `Store`, reading
persisted rows and events. The Rust adapter fails on an unknown command, step, expression or event,
and pins the scenario count so a changed inventory requires review. It does not execute the ESS
reference implementation or infer observations from the expected outcome.

`task check` runs the complete existing repository gate, including ESS and public documentation.
