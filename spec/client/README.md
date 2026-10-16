# Client CLI presentation

`client.yaml` gives the `b10x-substrate` command line its typed home: the reads it presents as
views, the requests it presents as commands, and the refusal it prints. `cli.yaml` is the
`ess-cli/2` presentation binding over that model; `ess generate cli` projects it into the package
in `spec/generated/cli`, whose `binding.json` lists every command path, callable, input, result and
error code. `cargo xtask check-ess` validates both documents and fails on any drift in that package.

It is a separate projection, as the capture selection in `../sessions` is, so the CLI cannot change
the accepted-operation ledger suite's obligations. Every observation is carried verbatim as `Json`:
the released contract bundle owns its shape, and this domain claims no lifecycle of workspaces,
execs or sessions. No conformance suite is committed or run for it. Read
[coverage and limitations](../coverage.md) before interpreting a green result.

```console
ess specify validate --path spec/client --strict-requires
ess specify cli --path spec/client --binding spec/client/cli.yaml --strict-requires
ess generate cli --path spec/client --binding spec/client/cli.yaml --out spec/generated/cli
```
