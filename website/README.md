# Substrate documentation website

This directory contains the self-contained public Docusaurus site for Substrate. Its `docs/` tree
is written for people arriving cold. It does not publish or link to the repository's internal design
records, plans, reviews, ADRs, status files, or private source.

## Develop

```bash
npm ci
npm run start
```

The local site is served at <http://localhost:3000/substrate/>.

## Gate

```bash
npm run typecheck
npm run build
```

Broken links and anchors fail the production build. The GitHub Pages workflow builds documentation
changes and publishes the static output from `main`.
