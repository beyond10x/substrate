# Substrate public documentation

Standalone static project site at https://beyond10x.github.io/substrate/.

`cargo xtask check-docs` validates the public inputs. `cargo xtask build-docs --out website/build --commit <full-sha>` renders the explicitly listed Markdown guides and authored homepage, copies the stylesheet, and writes exact-commit provenance. `.well-known/b10x-routes.json` lists every published route with its sorted rendered anchor IDs, stamped with the same commit; the organization Website uses it to redirect the former `/docs/substrate/` pages. The destination must be empty to prevent stale files from entering the publication artifact.

The renderer is Rust in `xtask/src/docs.rs`. It does not read internal architecture, planning or review material. The source allowlist is `PAGES`; each guide retains its `/substrate/docs/` path. No Node toolchain or global documentation integration is required.

The *Release notes* page appends the newest released section of `CHANGELOG.md` at build time. `check-docs` holds the adapter matrices, the contract tables and the CLI Commands table to the specification, and holds the site to the UX ratchets in [`ux/baseline.json`](ux/baseline.json). [`ux/trend.md`](ux/trend.md) records each improvement round. After a change that improves a measured value, run `cargo xtask docs-baseline`.
