# b10x-substrate-cli

`b10x-substrate`, a command-line client that drives a running Substrate daemon over its Unix
socket. Every command is one call through `b10x-substrate-sdk`; the client adds no daemon route,
wire field or behaviour.

```bash
cargo install --path crates/b10x-substrate-cli --locked
export SUBSTRATE_SOCKET=./run/substrate.sock
b10x-substrate machine
ws=$(b10x-substrate --output json workspace create | jq -r .result.workspace)
b10x-substrate exec run --workspace "$ws" -- /usr/bin/printf 'hello\n'
b10x-substrate workspace destroy "$ws"
```

`--socket <path>` selects the daemon and falls back to `SUBSTRATE_SOCKET`. `--output json` prints
the `{"ok":…}` envelope; a daemon refusal is the error `refused` with the daemon's named code in
`error.data.code`, and the exit status is non-zero. `exec run` passes the command's output through
and exits with its status.

The command line is the ESS CLI binding in `spec/client/cli.yaml`. The `cli_binding_validates` test
holds the clap definition to its resolved form, `spec/generated/cli/binding.json`, and the
acceptance scenarios in `tests/scenarios.rs` drive the shipped daemon binary.

Guide: <https://beyond10x.github.io/substrate/docs/guides/cli/>.
