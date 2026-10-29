# b10x-substrate-executor

The executor the Kubernetes namespace driver runs inside the cluster: one static binary,
`substrate-executor`, with these roles.

- `version` writes the handshake line the driver compares with its own release.
- `keeper` is PID 1 of a workspace's keeper Pod: it reaps children and exits on `TERM`.
- `fs` serves one file operation on the mounted workspace volume with the guarded `openat2` I/O
  every Substrate driver uses. No shell performs file I/O.
- `install <path>` copies the executor into an exec Pod's shared volume.
- `run <spec parts>` is PID 1 of an exec Pod: it checks the applied cgroup bounds, runs the argv
  without a shell in exactly the given environment, and tags each output chunk with its stream.

The library holds the protocol types and the frame codec the driver decodes with. The package
depends on `b10x-substrate-wire` and `b10x-substrate-guarded` and nothing else in this workspace,
so it links neither the host driver nor a TLS or Git stack, and it builds static for
`x86_64-unknown-linux-musl`.

This package is under development. See the
[public Substrate documentation](https://beyond10x.github.io/substrate/docs/guides/deployment) for
its current status.
