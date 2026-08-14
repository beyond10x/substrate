# Working on daemonloom/substrate

This repository owns the standalone Daemonloom execution substrate. During the design phase, read:

1. `README.md`
2. `docs/VISION.md`
3. `architecture/overview.md`
4. `architecture/dependency-rules.md`
5. `architecture/stack-integration.md`
6. `STATUS.md` and `ROADMAP.md`
7. the applicable design documents and ADRs

## Invariants

- The repository and every future Daemonloom repository are private.
- Automated commits and pushes use the GitHub App identity `daemonloom-bot`; never fall back to a
  human identity.
- Substrate owns generic bounded execution and observed state. It contains no agent loop, connector
  vendor semantics, grant engine, fleet scheduler, or cloud product policy.
- Substrate is Flux-free: no Flux crate or type may appear in any dependency kind or public/private
  implementation. Flux is prior art and a possible client.
- A missing isolation or capability guarantee is a named refusal, never silent degradation.
- Drivers implement one substrate contract and expose verified capability facts; clients do not
  branch on driver internals.
- Every created JSON authority must have one exact schema classification and validate in CI.
  Unclassified JSON fails closed. Every JSON Schema must validate offline against its declared
  Draft 2020-12 meta-schema with the pinned standards validator; immutable historical bundle bytes
  are classified externally without rewriting them.
- Do not add implementation code until the design-closure gate in `docs/plan/01-design-closure.md`
  is accepted.

## Documents

- Current architecture belongs in `architecture/`.
- Draft contract work belongs in `docs/design/` and must state its status.
- Accepted decisions belong in `adr/` and use YAML frontmatter with `date` and `status`.
- Sequencing belongs in `ROADMAP.md`; observed progress belongs in `STATUS.md`.
- Use repository-relative Markdown links locally and canonical HTTPS links across repositories.
  Never commit machine-local paths, sibling-checkout links, `file://` URLs, or editor URIs.

## Change discipline

Keep changes reviewable and preserve the direction from composition/products toward substrate. A
contract change must identify affected capabilities, refusal behavior, observations, events, and
consumer compatibility before implementation begins.
