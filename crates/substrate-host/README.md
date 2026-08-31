# b10x-substrate-host

The Linux host driver used by the Substrate daemon to implement confined workspaces and bounded
processes. Direct driver calls do not provide the daemon's durable operation ledger, authenticated
Unix-socket boundary, events, or recovery; applications should use `b10x-substrate-sdk`.

This package is under development. See the
[public Substrate documentation](https://beyond10x.github.io/substrate/) for its current status.
