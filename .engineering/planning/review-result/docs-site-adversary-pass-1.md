---
format: aep.planning-md/3
id: review-result:docs-site-adversary-pass-1
kind: review-result
status: active
title: 'Adversary pass 1: the public site ported to the shared documentation layer'
relations:
- reviews: story:docs-shared-site
revision: 3
---
**Adversary report: Docusaurus port of the public site**

```
unit: Docusaurus port of the public site, commit a07490b4, as merged in tree substrate-docs-site @ 79b719cc (integration/docs-site-and-binaries)
verdict: NEEDS-CHANGE
cases: executed 346→353, red 6
origin: introduced 9 / pre-existing 0 / undecided 0
wrote-outside-worktree: 1 directory (<scratch>/adv-docs/)
needs-coordinator: none
```

### 1. What I touched

`git --no-pager diff --stat` is empty, because the one file I added is untracked. `git status --short` shows:
```
?? xtask/tests/docs_site_adversary.rs
```
That is a test file only. I changed no implementation file.

### 2. Cases added (`xtask/tests/docs_site_adversary.rs`)

Every case runs the shipped `xtask` binary against a fixture. The `check-docs` cases use a copy of the public inputs; the `docs-provenance` cases use a hand-built 21-route site. Each mutating case first runs a control that the binary accepts.

| Case | Asserts | Now |
|---|---|---|
| `raw_html_in_the_changelog_section_is_refused_on_the_release_notes_page` | after `docs-releases`, raw HTML in the newest changelog section fails `check-docs` | red |
| `an_image_in_the_changelog_section_is_refused_on_the_release_notes_page` | same, for an image | red |
| `a_duplicate_heading_id_on_a_page_is_refused` | a second `## Served now` in `status.md` fails `check-docs` | red |
| `the_hand_built_site_is_accepted_and_inventories_exactly_the_published_routes` | control: the fixture site passes and the inventory lists 21 routes | green |
| `a_built_page_outside_the_declared_routes_is_refused` | a built page that is not one of the 21 routes fails `docs-provenance` | red |
| `a_docs_page_without_the_main_anchor_is_refused` | a docs page without `id=main` fails | red |
| `a_docs_page_whose_h1_lost_its_id_is_refused` | an H1 without an ID fails | red |

Red output, each case run alone before the suite (verbatim excerpts):
```
panicked at xtask/tests/docs_site_adversary.rs:132:5:
check-docs admits raw HTML on the published Release notes page
status Some(0)
stdout:
Substrate documentation: 20 pages match the sidebar, the release notes match CHANGELOG.md, the start page's links resolve, derived tables match the specification, the reading-load ratchet holds
```
```
panicked at xtask/tests/docs_site_adversary.rs:158:5:
check-docs admits an undeclared image on the published Release notes page
status Some(0)
```
```
panicked at xtask/tests/docs_site_adversary.rs:181:5:
check-docs admits a second `## Served now` on status.md
status Some(0)
```
```
test the_hand_built_site_is_accepted_and_inventories_exactly_the_published_routes ... ok
```
```
panicked at xtask/tests/docs_site_adversary.rs:327:5:
docs-provenance bound an undeclared page into the inventory: [... "/substrate/docs/use-cases/", "/substrate/internal/plan/"]
status Some(0)
stdout:
Substrate documentation at .../build: 22 routes checked and bound to bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb
```
```
panicked at xtask/tests/docs_site_adversary.rs:344:5:
docs-provenance bound /substrate/docs/guides/deployment/ without #main
status Some(0)
```
```
panicked at xtask/tests/docs_site_adversary.rs:362:5:
docs-provenance bound /substrate/docs/concepts/boundary/ with an H1 that has no ID
status Some(0)
```

### 3. Suite run (after the cases existed)

Command: `nice -n 19 cargo test --locked -p xtask --no-fail-fast`, with `CARGO_BUILD_JOBS=4` and `CARGO_INCREMENTAL=0`. Exit status 101.
```
unittests src/main.rs                    ok. 270 passed; 0 failed; 9 ignored
tests/docs_site_adversary.rs             FAILED. 1 passed; 6 failed
tests/pty_frame_vocabulary_mutants.rs    ok. 3 passed
tests/quota_fixture_adversary_pass1.rs   ok. 12 passed
tests/read_only_roots_bundle_adversary.rs   ok. 6 passed; 1 ignored
tests/read_only_roots_bundle_adversary_2.rs ok. 6 passed
tests/release_workflow.rs                ok. 43 passed
tests/session_exec_fields_adversary_pass1.rs ok. 6 passed
EXIT=101
```
The before count (346) is the same run with my binary's 7 cases removed.

### 4. Findings (cover a07490b4 inside 79b719cc)

1. **The release-notes guard no longer applies to the generated changelog section.** NEEDS-CHANGE, introduced.
   - **Measured:** `xtask/src/docs.rs:162` parses only the text above the marker, so the guards at `:192` (raw HTML) and `:194` (image) never see the generated section. The base renderer applied both to it (`git show a07490b4^:xtask/src/docs.rs`, lines 227, 229 and 462).
   - **Why it matters:** the `.md` format renders raw HTML (`node_modules/@docusaurus/mdx-loader/lib/processor.js:110-126`, rehype-raw).
   - **What reaches it:** any `CHANGELOG.md` bullet, then the documented `cargo xtask docs-releases` step.
   - **Fix:** parse the generated section with the same guards.

2. **The duplicate-heading guard was dropped.** NEEDS-CHANGE, introduced.
   - **Measured:** the base refused a page with two headings of one ID (base `docs.rs:189`). The current `docs.rs:185` does not.
   - **Why it matters:** Docusaurus renames the later heading to `<id>-1`. A heading added above an existing one with the same text takes over its published anchor, which breaks the "keep heading IDs stable" constraint.
   - **What reaches it:** any page edit.

3. **`docs-provenance` does not hold the built routes to the declared set.** NEEDS-CHANGE, introduced.
   - **Measured:** `docs.rs:686` checks only the routes in `PAGES`; anything else that was built gets inventoried and uploaded. The base test held the inventory to `PAGES` plus the start page (base `docs.rs:647`).
   - **What reaches it:** any `.html` file in `website/static/`. `check-docs` looks at nothing in `static/` except `index.html` and `styles.css`, so this is a second publication path around the page allowlist.

4. **Nothing checks `#main` or the H1 IDs on the built pages.** NEEDS-CHANGE, introduced.
   - **Measured:** the per-page checks at `docs.rs:692` and `:694` never look for either. The base test asserted `main` on every route (base `docs.rs:666`).
   - **What reaches it:** the two theme overrides (`src/theme/DocItem/Layout`, `src/theme/Heading`) are the only thing that keeps these anchors. A docs-system bump or a swizzle change would drop them with the gate still green.

5. **UXF-004 regressed, and its regression test cannot catch it.** CONFIRMED, introduced.
   - **Labels:** on the base site, every table region was labelled with its heading (live site: `aria-label="Families by adapter table"`). In the new build's server-rendered HTML the wrapper is a bare `<div class=b10x-table-wrap>`. After hydration at 1280px, the two tables that overflow both read `aria-label="Table"`, and the third table is not focusable.
   - **Test:** the regression test `tables_scroll_in_a_frame` (`docs.rs:640`) checks only the class name.
   - **Record:** `website/ux/findings/UXF-004.md` still says the region is "labelled with the heading above it".

6. **One start-page link now points at the wrong content.** CONFIRMED, introduced.
   - **Measured:** in `website/static/index.html:4`, the text "Start a personal daemon… Complete setup and workspace round trip →" and the comment "Use the private socket from the setup guide" link to `/substrate/docs/getting-started`. That page no longer has the daemon setup or the round trip; they moved to `guides/run-a-command#start-with-an-observable-machine`. The hero link was updated; this one was missed.

7. **The published JS bundle contains the build machine's absolute path.** CONFIRMED, introduced.
   - **Measured:** `website/build/assets/js/main.3fa9b169.js` contains `staticDirectories:["static","<tree>/website/node_modules/@beyond10x/docs-system/styles/static",…`. This comes from `withProductSite` (`website/docusaurus.config.ts:87`).
   - **Inferred, not observed:** on CI this would be the runner's checkout path, so the artifact's bytes depend on where it was checked out.

8. **The route inventory now publishes unstable IDs as anchors.** CONFIRMED, introduced, note.
   - **Measured:** `docs.rs:578` now lists React `useId` IDs (`b10x-table-_R_5q7dalbah_-instructions`) and Docusaurus internals (`__docusaurus`, `theme-svg-external-link`) as anchors. The `useId` ones change whenever the component tree changes, so a consumer that diffs anchors will see them "vanish".

9. **Mermaid, chips and highlighting are held by no program.** CONFIRMED, introduced, note.
   - **Measured:** Mermaid renders only in the browser. The server-rendered `concepts/model.html` and `operations.html` contain neither the diagram nor its source, so a reader without JS sees nothing (before, they saw the source as text). No check covers Mermaid, the chips or the highlighting.

### 5. What I attacked and could not break

- **Routes and IDs:** the base and built inventories have the same 21 routes. The only lost IDs are the 7 accepted `getting-started` moves, and all 7 are on `guides/run-a-command`. The base inventory is the live site, which serves `c2b95f28`, the unit's parent.
- **Chips:** all 147 matrix cells and 3 legend chips render. Each chip's class matches its value (✓ TRUE, ? UNKNOWN, FALSE), checked in headless Chromium.
- **Mermaid:** both diagrams render as SVG (`mermaid-svg-*`).
- **Highlighting:** every fenced block has tokens, including bash, json, rust and toml; `text` blocks stay plain.
- **Start page:** `build/index.html` and `styles.css` are byte-identical to `website/static/`.
- **Trailing slashes:** `/x/` copies redirect to `/x`, keeping the hash.
- **Drift check:** it reads the same pages and headings as before, and an unknown cell word fails it.
- **UNKNOWN on always-declared members:** the check accepts this, but by design; the suite cannot express a failed probe.
- **Names and paths in the diff:** no former-brand name and no other forbidden names; no machine-local path in the committed diff.
- **Install docs:** the archive names match `release.yml:969-970`.

### 6. Paths written outside the worktree

`<scratch>/adv-docs/`, the assigned scratch directory:
- `diff-docs.patch`, `diff-nonxtask.patch`, `live-routes.json`, `suite.log`
- scripts: `cmp_routes.py`, `chips.py`, `tokens.py`, `unknown_always.py`, `dump.sh`, `run_case.sh`, `run_suite.sh`
- `dom/{adapters,model,ops}.{html,err}`
- `tmp/` (empty; the test temp directories cleaned themselves up)

The test build went to the tree's existing `target/`. I did not stop the preview server.

### 7. Findings block

```findings
- file: xtask/src/docs.rs
  line: 192
  category: contract-drift
  severity: warning
  verdict: NEEDS-CHANGE
  origin: introduced
  message: raw HTML in the newest CHANGELOG.md section reaches the published Release notes page because the guard runs only above the generated-section marker
- file: xtask/src/docs.rs
  line: 194
  category: contract-drift
  severity: warning
  verdict: NEEDS-CHANGE
  origin: introduced
  message: an image in the newest CHANGELOG.md section is published unchecked; the former renderer refused it as an undeclared public image
- file: xtask/src/docs.rs
  line: 185
  category: contract-drift
  severity: warning
  verdict: NEEDS-CHANGE
  origin: introduced
  message: the duplicate-heading guard was dropped, so a same-text heading added above an existing one silently takes over its published anchor
- file: xtask/src/docs.rs
  line: 686
  category: acceptance
  severity: warning
  verdict: NEEDS-CHANGE
  origin: introduced
  message: docs-provenance checks only PAGES routes and inventories and uploads any other built page, such as an html file dropped into website/static
- file: xtask/src/docs.rs
  line: 692
  category: contract-drift
  severity: warning
  verdict: NEEDS-CHANGE
  origin: introduced
  message: no built-site check asserts the #main anchor the DocItem Layout override exists to keep; the base test asserted it on every route
- file: xtask/src/docs.rs
  line: 694
  category: contract-drift
  severity: warning
  verdict: NEEDS-CHANGE
  origin: introduced
  message: no built-site check asserts each page's H1 ID, which only the Heading override preserves
- file: xtask/src/docs.rs
  line: 640
  category: mutant
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: UXF-004 regressed (regions are labelled "Table" instead of their heading and are focusable only after hydration) and its regression test checks only the wrapper class
- file: website/static/index.html
  line: 4
  category: acceptance
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: the start page's "Complete setup and workspace round trip" link points at getting-started, which no longer has the daemon setup or the round trip
- file: website/docusaurus.config.ts
  line: 87
  category: judgement
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: withProductSite puts the builder's absolute node_modules path into the published main JS bundle
- file: xtask/src/docs.rs
  line: 578
  category: judgement
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: the route inventory publishes React useId and Docusaurus-internal IDs as anchors, which churn on unrelated component changes
- file: website/docs/concepts/model.md
  category: judgement
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: Mermaid renders only in the browser, so a reader without JS sees no diagram and no source, and no program holds Mermaid, chips or highlighting
```
