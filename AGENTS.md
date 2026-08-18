# Working on daemonloom/foundation/substrate

This component owns the standalone Daemonloom execution substrate. The design-closure gate in
`docs/plan/01-design-closure.md` is accepted; implementation is in progress against its phase exit
criteria. The root [`AGENTS.md`](../../AGENTS.md) applies throughout; this file adds component
rules. Read:

1. `README.md`
2. `docs/VISION.md`
3. `architecture/overview.md`
4. `architecture/dependency-rules.md`
5. `architecture/stack-integration.md`
6. `STATUS.md` and `ROADMAP.md`
7. the applicable design documents and ADRs

## Invariants

- The monorepo is private; any future Daemonloom repository remains private unless an accepted
  architecture decision explicitly authorizes otherwise.
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
- Use repository-relative Markdown links for current material anywhere in the monorepo and
  canonical HTTPS links only for external or immutable historical sources.
  Never commit machine-local paths, sibling-checkout links, `file://` URLs, or editor URIs.

## Gate

```text
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo fmt --all --check
python3 scripts/check-links.py
python3 scripts/check-adrs.py
python3 scripts/check-contract-bundle.py
python3 scripts/check-runtime-vectors.py
```

Run `bash scripts/check-local.sh --release` from the monorepo root before treating a
cross-component change as green.

## Change discipline

Keep changes reviewable and preserve the direction from composition/products toward substrate. A
contract change must identify affected capabilities, refusal behavior, observations, events, and
consumer compatibility before implementation begins.
