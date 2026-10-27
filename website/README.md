# Substrate public documentation

Standalone project site at https://beyond10x.github.io/substrate/, built with Docusaurus on the shared Docs System, which `package.json` pins by commit.

```console
npm --prefix website ci
npm --prefix website run build
cargo xtask docs-provenance --site website/build --commit "$(git rev-parse HEAD)"
```

The build writes `website/build`. `docs-provenance` checks the built site and writes `.well-known/b10x-site.json` with the exact commit and `.well-known/b10x-routes.json`, which lists every published route with its sorted anchor IDs; the organization Website uses it to redirect the former `/docs/substrate/` pages. Keep page paths and heading IDs stable.

The pages are the Markdown files in `docs/`, listed in `sidebars.ts` and in `PAGES` in `xtask/src/docs.rs`. The start page is the static `static/index.html` with `static/styles.css`, copied into the build as it is. `cargo xtask check-docs` needs no Node toolchain: it checks that the pages, the sidebar and `PAGES` agree, that the start page's links resolve, and that the reading-load ratchet holds.

The *Release notes* page ends with the newest released section of `CHANGELOG.md`. After a release, run `cargo xtask docs-releases`; `check-docs` fails while the two differ. `check-docs` also holds the adapter matrices, the contract tables and the CLI Commands table to the specification. Matrix cells are `TRUE`, `UNKNOWN` or `FALSE`, which the Docs System renders as chips.

[`ux/baseline.json`](ux/baseline.json) holds the UX ratchets: reading load per page, held by `check-docs`, and clicks per reader task, held by `docs-provenance` on the built site. [`ux/trend.md`](ux/trend.md) records each improvement round. After a change that improves a measured value, run `cargo xtask docs-baseline --site website/build`.
