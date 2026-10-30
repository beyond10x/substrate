# b10x-substrate-conformance

`b10x-substrate-conformance`, the clean-room runner that certifies one deployed Substrate endpoint
over HTTPS and WSS from outside its process, container and checkout. It drives the positive journey
through `b10x-substrate-sdk`, and a small raw probe on `rustls` covers what the SDK hides: the raw
machine response, missing and forged credentials, plaintext, TLS 1.2, an `h2`-only offer, forbidden
certificate names and lost responses.

```bash
cargo build --release --locked -p b10x-substrate-conformance
target/release/b10x-substrate-conformance \
  --endpoint https://substrate.example:8443/ --trust-roots ca.pem \
  --token-command /usr/local/bin/issue-token --out conformance-report
```

Build it with `-p` alone: `cargo xtask check-packages` holds that this build links no daemon, host
driver or store. Cases are selected from the pinned bundle's operation registry and the endpoint's
capability facts; a case they do not allow is `absent` with a named reason. The report,
`report.json` with `report.json.sha256`, follows the closed schema in
`schema/remote-conformance-report.v1.schema.json`. Exit codes: 0 passed, 1 failed, 2 invalid
input, 3 harness failure, 4 incomplete.

`tests/local_endpoint.rs` runs the binary against the shipped daemon on a local TLS listener. Its
delegated scenario runs under `scripts/delegated-lane.sh`.

Guide: <https://beyond10x.github.io/substrate/docs/guides/remote-conformance/>.
