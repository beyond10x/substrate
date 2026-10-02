# Substrate public documentation

Standalone static project site at https://beyond10x.github.io/substrate/.

`cargo xtask check-docs` validates the public inputs. `cargo xtask build-docs --out website/build --commit <full-sha>` renders the explicitly listed Markdown guides and authored homepage, copies the stylesheet, and writes exact-commit provenance. The destination must be empty to prevent stale files from entering the publication artifact.

The renderer is Rust in `xtask/src/docs.rs`. It does not read internal architecture, planning or review material. The source allowlist is `PAGES`; each guide retains its `/substrate/docs/` path. No Node toolchain or global documentation integration is required.
