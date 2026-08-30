# Design 14: a network session authority is bound to a client key and one TLS channel

**Status:** proposed · **Date:** 2026-08-30

This document precedes the ADR that `story:network-session-authority` names as its first evidence.
It fixes the proof binding, the TLS dependency, where single redemption becomes durable, the named
refusals, and which listener may serve the byte plane. Its successor bundle number assumes it is the
only change against the current frontier; if a sibling design lands first, this one moves to that
bundle's successor.

## Context

[Design 05](05-streams-sessions-and-endpoints.md) § 2 says session authority is "proof-bound on
network transport, and single-use for initial redemption", and V1 decision 3 fixes 60 seconds, one
redemption, one concurrent attachment and a fresh authority per reconnect
(`docs/design/05-streams-sessions-and-endpoints.md:34-36`, `:66-67`). Decision 4 fixes WebSocket over
TLS (`:68-69`), and [deployment postures](../../architecture/deployment-postures.md) require
TLS/mTLS or a configured trusted tunnel for any non-loopback control listener
(`architecture/deployment-postures.md:16`).

**The document those decisions defer to is not in this repository.** Decision 2 says the issuer split
is "fixed by `architecture/adr/0016-operation-scoped-session-authority.md`"
(`docs/design/05-streams-sessions-and-endpoints.md:65`). There is no `architecture/adr/` directory
here — `architecture/` holds five documents and no ADR — and `adr/` holds 0001 through 0014. Four
sibling citations have the same shape (`docs/design/01-contract.md:355`, `:304`,
`docs/design/06-authentication-secrets-and-trust.md:20`,
`architecture/stack-integration.md:32`), so this is the repository's convention for material that
lives elsewhere (`AGENTS.md` § *Document placement*), not a broken link — and
`cargo xtask check-links` never sees it, because inline code is not a link. The consequence stands
either way: **nothing in this repository states what "proof-bound" is proof of.** This document
decides it rather than citing it.

What exists at HEAD is one attachment path with no authority at all. The Unix listener and the
development-only TCP listener build the *same* service from the *same* `router(app)`
(`crates/substrate-daemon/src/runtime.rs:514` and `:785`), and that router registers
`/v1/pipe-sessions/{session_id}/attach` (`crates/substrate-daemon/src/app/routes.rs:75-78`) among 26
routes (`contracts/substrate-wire/0.8.0/bundle.json`, `compatibility.preserves_routes`). Admission is
`session.not-attachable`, `session.already-attached` and `session.attachment-capacity`
(`crates/substrate-daemon/src/app/sessions.rs:767`, `:781`, `:793`) over an in-memory permit and one
durable claim.

## Decision

**The binding is a client key, and the RFC 5705 exporter is what that key signs.** At mint the client
supplies an Ed25519 public key and the authority carries its SHA-256 thumbprint. At redemption the
client sends one frame carrying a signature over the authority id, the exporter value for *that* TLS
connection, and the redemption instant. The daemon derives the exporter for its own side of the same
connection and verifies the signature under the bound key.

**The exporter alone was rejected because it proves nothing.** Both ends of *any* TLS connection
derive a matching exporter, and the authority is minted before the attaching connection exists, so
it cannot be bound to an exporter at mint time. An attacker holding the authority bytes opens their
own connection, computes their own matching exporter, and redeems. The only attacker exporter-only
stops is one that terminates TLS and re-opens it — a re-encrypting proxy — and calling the result
"proof-bound" would be a guarantee named and not held, which is invariant 3's silent degradation
rather than its named refusal. The key costs the client a key pair; it buys the property design 05
already claims.

**No new crypto crate, and no new identity surface.** `ed25519-dalek = "2.1"` is already a workspace
dependency (`Cargo.toml:27`) and the daemon already verifies Ed25519 signatures for delegated context
(`crates/substrate-daemon/src/delegation.rs:32`, ADR 0011). The key is the client's own, authenticates
nothing but this one authority, and names no principal: principal identity and token audiences belong
to `identity` (`AGENTS.md` § *Out of scope*). The exporter label is a new wire-visible identifier;
whether it may carry the former brand is atlas ADR 0001's question and is settled with the successor
bundle, not here.

**The TLS crate is `rustls` with `tokio-rustls`.** The workspace has none today: `rustls`,
`tokio-rustls`, `native-tls` and `openssl` appear in no member `Cargo.toml` and as no `Cargo.lock`
package. rustls is chosen because RFC 5705 export is a first-class API on the connection —
`ConnectionCommon::export_keying_material`, `src/conn.rs:460` in the published rustls 0.23 source —
so the binding above is implementable; with `native-tls` it is not portably reachable, which would
decide the binding by omission. `tokio-rustls` because the accept loop is tokio and hands the service
a `TokioIo`-wrapped stream (`crates/substrate-daemon/src/runtime.rs:516`); a `TlsStream` wraps the
same way. The per-connection exporter reaches the handler as an axum `Extension`, the path the
per-connection `Identity` already takes (`:514`, `:785`).

**The C build cost is stated, not discovered.** rustls 0.23's default features include `aws_lc_rs`,
which pulls `aws-lc-sys` — a C and assembly build. That is not a new class of build dependency:
`rusqlite` is already `features = ["bundled"]` (`Cargo.toml:39`) and compiles the SQLite
amalgamation, `cc` is already resolved (`Cargo.lock:244`), and the builder image is
`rust:1.97-bookworm` over a `distroless/cc-debian12` runtime (`Dockerfile:2`, `:12`). If the aws-lc
build proves unreproducible under the pinned toolchain, the `ring` provider is a feature flag, not a
redesign.

**Single redemption reuses the durable primitive; it does not get a second one.**
`claim_pipe_session_attachment` already consumes the one attachment right inside an `Immediate`
transaction and answers `AlreadyClaimed` on a second call
(`crates/substrate-store/src/sessions.rs:526-546`). The authority is consumed **in that same
transaction**, not beside it: two transactions leave a window where the authority is spent and the
attachment is not. The function takes the authority id and the presented proof and returns one more
refusal variant, so one transaction decides both.

**The binding record is not the resource.** The `sessions` table stores `deployment, subject, id,
exec_id, resource_json` (`crates/substrate-store/src/schema.rs:154-162`), and `resource_json` is the
serialized wire `PipeSession` (`crates/substrate-store/src/sessions.rs:614-632`) — the exact bytes
`GET /v1/pipe-sessions/{id}` returns. Authority state therefore lives in its own columns beside
`resource_json`, never inside it, or design 05 decision 1's "absent from URLs, logs, events, and
durable client configuration" is broken by the resource projection itself. **The authority value is
never stored**: what is kept is its id, the key thumbprint, the expiry instant and the redemption
outcome — the rule the TCP bearer already follows, which stores `bearer_sha256: [u8; 32]` and never
bearer text (`crates/substrate-daemon/src/runtime.rs:760-762`).

**Verify before consuming.** A forged, malformed or expired authority must not burn the session's one
attachment right. The cheap stage — well-formed, unexpired, signature verifies under the bound key
over this channel's exporter — runs before the in-memory permit (`:773` at HEAD); the consuming stage
runs inside the claim transaction. A replayed but otherwise valid authority is caught in the
transaction; everything else never reaches it.

**Five named refusals, in the existing `session.*` family and existing classes**
(`crates/substrate-wire/src/lib.rs:151-157`):

| condition | class | code | answered as |
|---|---|---|---|
| no authority on a listener that requires one | `refused` | `session.authority-absent` | 401 |
| past its stated expiry, at most 60 s after mint | `refused` | `session.authority-expired` | 401 |
| already redeemed | `conflict` | `session.authority-redeemed` | 409 |
| signature fails, or its exporter is not this channel's | `refused` | `session.authority-unbound` | 401 |
| attach on a listener that cannot carry an authority confidentially | `refused` | `session.transport-insecure` | 400 |

A sixth is not needed: a second concurrent attachment is already `session.already-attached`
(`crates/substrate-daemon/src/app/sessions.rs:781`, `:814`), so one of the story's five tests asserts
a guarantee that exists and four assert new ones.

**Reconnect is a new session, not a resumed one.** At HEAD the attachment right is terminal by design
— "a failed upgrade or a lost attachment is therefore terminally contained instead of becoming
reconnectable" (`crates/substrate-store/src/sessions.rs:524-525`, ADR 0008) — and no client frame
resumes anything (`PipeClientFrame` has `Stdin`, `CloseInput` and `Signal` only,
`crates/substrate-wire/src/lib.rs:1305-1318`). So a fresh authority is necessary and not sufficient:
what forbids resumption is the terminal attachment right, and the ADR records that rather than
implying the authority carries the whole guarantee.

**The Unix socket keeps attaching with no authority.** Design 05 § 2 says proof-bound *on network
transport*; the Unix path is authenticated by kernel peer credentials into `local:<uid>`
(`crates/substrate-daemon/src/runtime.rs:509-513`), and adding a token there is ceremony over a
channel the kernel already authenticated, at the price of every existing pipe-session vector.

**The router splits, and the story's acceptance stands.** Acceptance 5 says the development-only TCP
transport "must not gain this route"; it has had it since the route existed, because both listeners
call the same `router(app)`. The split is by property, not by listener name: **a route that mints or
redeems a channel authority is not served by a listener that cannot carry one confidentially** —
`POST /v1/pipe-sessions` (`crates/substrate-daemon/src/app/routes.rs:67-70`) and
`GET /v1/pipe-sessions/{session_id}/attach` (`:75-78`). Serving the mint over plaintext would put the
authority on the wire and make the binding decorative, so excluding only the attach is not enough.
The read-only session routes carry no authority and stay.

**What the split costs, and who is exposed if it is skipped.** It costs one function signature:
`router` gains a sibling, and its three call sites (`crates/substrate-daemon/src/runtime.rs:514`,
`:785`, `crates/substrate-daemon/src/app/tests.rs:49`) each say which set they want, on the shared
`routes.rs` the story's own scope flags as the epic's collision surface. The recurring cost is
discipline — every future route is classified when it is added — and that is the point, because a
route added later is otherwise served by the development listener silently. Restating the acceptance
instead costs nothing today and leaves this: `serve_tcp` refuses without `--tcp-development-only` and
`--tcp-private-overlay` (`crates/substrate-daemon/src/runtime.rs:765-770`), but **`--tcp-private-overlay`
is an operator assertion, not a check.** There is no `is_loopback` call anywhere in `runtime.rs` or
`main.rs`, so a non-loopback plaintext bind is accepted, and `architecture/deployment-postures.md:16`
is enforced by nothing. The credential guarding it is a static bearer whose file is bounded by
ownership, mode and length and read for **no expiry at all** (`read_bearer_digest`,
`crates/substrate-daemon/src/runtime.rs:906-931`), so design 06 V1 decision 1's explicit expiry,
overlap rotation and revocation are absent on this path. Concretely: on an address an operator
declared private and was wrong about, one observer reads a never-expiring bearer off the wire and
then owns the raw stdin and stdout of every confined process, indefinitely.

**The static-bearer TCP path is kept and narrowed, not removed.** Removing `--tcp-listen` breaks a
documented posture (`architecture/deployment-postures.md:8`) and buys nothing here. It is narrowed by
the split above and by one new startup refusal: a `--tcp-listen` address that is not loopback and has
no TLS material does not bind — a fifth `bail!` beside the four in `serve_tcp`
(`crates/substrate-daemon/src/runtime.rs:765-776`), which makes that posture rule enforced rather than
asserted. `require_tcp_bearer` (`:884`) is unchanged and is **not** the session authority: the bearer
admits a subject to the control plane, the authority admits one channel to one session once.
Superseding the bearer is the hosted trust-envelope verifier's job (design 06 § 1, phase 7),
explicitly outside this story. mTLS remains a listener-level reachability control and is orthogonal —
a client certificate authenticates a peer for a whole connection, never one authority for one
redemption.

**Successor bundle `0.9.0`, provisionally.** `contracts/substrate-wire/0.8.0` is the frontier and
records `predecessor: 0.7.0`, `adds_routes: 0`, `preserves_routes: 26`
(`contracts/substrate-wire/0.8.0/bundle.json`). Another design is being drafted against the same
frontier; **the number is provisional and belongs to whichever lands first.** The successor adds no
route — the attach path already exists — and adds the endpoint reference on the start response with
the authority as a body field (design 05 decision 1), one client redemption frame, and the five
refusal codes above. Its `cargo xtask check-bundle 0.9.0` goes into `scripts/gate.sh` in the same
change; a bundle whose check is not in the gate is unverified from the next commit onward
(`AGENTS.md` § *The gate*). Earlier directories keep their bytes (invariant 6).

## Consequences

A client on another machine can attach over TLS with an authority that a thief cannot use, on a
channel a relay cannot substitute, once. The listener that could already serve those bytes without any
of it stops serving them.

The cost is a key. A client is a program, not a `curl` — there is no hand-typed attach over the
network. The development ergonomics are unchanged only because the Unix socket still needs no
authority at all.

The gate proves more of this than of the last two ADRs. TLS, the exporter, expiry under paused time,
second-redemption refusal, a signature over the wrong exporter, the 404 on the development listener
and the non-loggability grep all run over loopback on a hosted runner. Only the confined child behind
them needs the delegated lane on a self-hosted runner, and that half is reported **absent rather than
passed** (invariant 3). The test certificate is generated at run time by a dev-dependency, never
committed: this repository is public (invariant 9), `scripts/check-secrets.sh` scans the whole
history, and the one Gitleaks exception is scoped to the delegated-context JWT vectors and to nothing
else (`AGENTS.md` § *Safety envelope*).

Two things this design does not fix. `docs/design/05-streams-sessions-and-endpoints.md:65` still
points at a document that is not here, and only its own edit can correct that. The development
bearer still has no expiry, no rotation overlap and no revocation; this design records that the
development posture does not claim them and leaves them to the trust-envelope verifier.
