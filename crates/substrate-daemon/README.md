# b10x-substrate-daemon

The standalone Substrate daemon and its opt-in composition entrypoint. The daemon serves confined
workspace and bounded-process operations over an owner-private Unix socket. Applications can use
`b10x-substrate-sdk` to connect to an existing daemon or supervise a separate child process.

The wire contract is under development and is not stable. See the
[public Substrate documentation](https://beyond10x.github.io/substrate/) for current deployment and
security guidance.
