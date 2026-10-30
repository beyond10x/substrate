---
title: Remote conformance
description: Certify a deployed Substrate endpoint over HTTPS from outside its process, and keep the report and its digest as evidence.
---

# Certify a deployed endpoint

`b10x-substrate-conformance` checks one deployed Substrate endpoint from outside its process,
container and checkout. It speaks HTTPS and WSS only and links no daemon. It writes a closed
report and the report's SHA-256.

## Build it

```bash
cargo build --release --locked -p b10x-substrate-conformance
```

Build it with `-p` and nothing else. That build links no daemon, host driver or store, and
`cargo xtask check-packages` refuses a change that would make it.

## Run it

```bash
target/release/b10x-substrate-conformance \
  --endpoint https://substrate.example:8443/ \
  --trust-roots ca.pem \
  --token-command /usr/local/bin/issue-token \
  --restart-hook /usr/local/bin/restart-substrate \
  --ops-origin http://127.0.0.1:9090 \
  --out conformance-report
```

`--out` must not exist. The runner creates it with mode 0700 at start, before any request, and
writes only `report.json` and `report.json.sha256` into it. A run that ends without a report
removes it again. `--token-command` and `--restart-hook` must be absolute paths to executable files;
the runner checks both at start.

## Supply credentials

The runner asks for each credential separately. It writes one JSON line naming the reason, the
purpose and the scopes, and reads one line back:

```json
{"reason":"request","purpose":"primary","scopes":["exec","observe","workspaces"]}
```

`--token-command` runs a program for each request, with an empty environment except the names you
pass with `--token-env`. `--token-fd <n>` uses an inherited Unix stream socket instead.

Answer `unsupported`, or exit 3, when you do not serve a purpose. The scope cases that need a
credential narrowed to `observe` are then absent. A credential never reaches a file, an argument,
another child's environment, or the report.

## Restart the endpoint

The restart cases need `--restart-hook`. The runner runs it once, with no inherited descriptor and
only the variables you pass with `--hook-env`. The hook must make the daemon restart and exit 0. The
runner then waits for a new capability probe before it checks that nothing ran twice.

## Read the verdict

| exit | verdict | report |
|---|---|---|
| 0 | passed | written |
| 1 | failed | written |
| 2 | invalid input or an unknown contract | none |
| 3 | harness failure | none |
| 4 | incomplete | written |

A case runs only when the endpoint's capability facts, your scopes and your inputs allow it. Any
other case is `absent`, with a named reason. Absence never satisfies the core groups: machine, TLS,
auth, workspace, recovery and cleanup. A run limited with `--only` is partial and never passes.

A machine body that does not decode into the contract's types fails the core case
`machine.envelope-decodes`; no other case runs, and the report is written with
`capability_snapshot` `null`. A snapshot read the report cannot carry is recorded as `null`.
`not_covered` lists the crash windows only the daemon's own vectors reach and every operation of
the pinned registry that no case of the run names.

## Sign the report

The runner holds no key. Sign the report where you certify, and verify the signature there:

```bash
cosign sign-blob --bundle conformance-report/report.json.sigstore.json \
  conformance-report/report.json
```

The report's schema is
`crates/b10x-substrate-conformance/schema/remote-conformance-report.v1.schema.json`. It names the
endpoint only by digest and carries no credential, resource id or output byte.
