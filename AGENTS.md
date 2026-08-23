# Working on Substrate

[github.com/beyond10x/substrate](https://github.com/beyond10x/substrate) is the canonical home of
Substrate. It was extracted from the daemonloom monorepo at
[`e01ea676`](https://github.com/daemonloom/daemonloom/tree/e01ea676da18fb855814e7621514e0c98fc57c2c)
with full history on 2026-08-23; the monorepo keeps a pinned git-submodule checkout at
`foundation/substrate`. The gate is `bash scripts/gate.sh`.

This repository owns the standalone b10x execution substrate. The design-closure gate in
`docs/plan/01-design-closure.md` is accepted; implementation is in progress against its phase exit
criteria. The monorepo-era root
[`AGENTS.md`](https://github.com/daemonloom/daemonloom/blob/e01ea676da18fb855814e7621514e0c98fc57c2c/AGENTS.md)
is provenance for the rules this file inherited. Read:

1. `README.md`
2. `docs/VISION.md`
3. `architecture/overview.md`
4. `architecture/dependency-rules.md`
5. `architecture/stack-integration.md`
6. `STATUS.md` and `ROADMAP.md`
7. the applicable design documents and ADRs

## Invariants

- This repository is private, as is the daemonloom monorepo it came from; any future Daemonloom
  repository remains private unless an accepted architecture decision explicitly authorizes
  otherwise.
- Automated commits and pushes use the GitHub App identity `daemonloom-bot`; never fall back to a
  human identity.
- Substrate owns generic bounded execution and observed state. It contains no agent loop, connector
  vendor semantics, grant engine, fleet scheduler, or cloud product policy.
- Substrate is Flux-free: no Flux crate or type may appear in any dependency kind or public/private
  implementation. Flux is prior art and a possible client.
- A missing isolation or capability guarantee is a named refusal, never silent degradation.
- Drivers implement one substrate contract and expose verified capability facts; clients do not
  branch on driver internals.
- Every created JSON authority must have one exact schema classification and validate in the gate.
  Unclassified JSON fails closed. Every JSON Schema must validate offline against its declared
  Draft 2020-12 meta-schema with the pinned standards validator; immutable historical bundle bytes
  are classified externally without rewriting them.
- Implementation follows the accepted design-closure gate. A contract or capability change beyond
  its named decisions and deferrals needs a design document or ADR before code.

## Documents

- Current architecture belongs in `architecture/`.
- Draft contract work belongs in `docs/design/` and must state its status.
- Accepted decisions belong in `adr/` and use YAML frontmatter with `date` and `status`.
- Sequencing belongs in `ROADMAP.md`; observed progress belongs in `STATUS.md`.
- Use repository-relative Markdown links for current material in this repository and
  canonical HTTPS links only for external or immutable historical sources (including
  SHA-pinned monorepo provenance URLs).
  Never commit machine-local paths, sibling-checkout links, `file://` URLs, or editor URIs.

## Gate

```text
bash scripts/gate.sh
```

It runs `cargo test --workspace --locked`, `cargo fmt --all --check`,
`cargo clippy --workspace --all-targets --locked -- -D warnings`, and the script checks
(`check-links.py`, `check-adrs.py`, `check-contract-bundle.py`, `check-runtime-vectors.py`).
The monorepo-era cross-component suite (`scripts/check-local.sh` at the monorepo root) no longer
applies here; changes that affect monorepo consumers are picked up when the monorepo advances its
`foundation/substrate` submodule pin.

## Change discipline

Keep changes reviewable and preserve the direction from composition/products toward substrate. A
contract change must identify affected capabilities, refusal behavior, observations, events, and
consumer compatibility before implementation begins.
