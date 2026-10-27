//! Adversary pass on the Docusaurus port of the public site (`website/`, `xtask/src/docs.rs`).
//!
//! Everything runs through the shipped `xtask` binary against fixtures, never against the
//! checkout: `cargo xtask check-docs` and `cargo xtask docs-releases` in a copy of the public
//! inputs (the root is the nearest ancestor of the working directory that declares
//! `[workspace]`), and `cargo xtask docs-provenance` against a hand-built site that passes every
//! built-site check the unit wrote. Each mutating case starts from a control the binary accepts,
//! so a red case is red for the mutation and nothing else.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// Every docs page, as `PAGES` in `xtask/src/docs.rs` lists it.
const PAGES: [&str; 20] = [
    "index",
    "getting-started",
    "use-cases",
    "concepts/boundary",
    "concepts/confinement",
    "concepts/adapters",
    "concepts/operations",
    "concepts/model",
    "guides/run-a-command",
    "guides/confinement-profiles",
    "guides/storage-and-metrics",
    "guides/deployment",
    "guides/rust-sdk",
    "guides/cli",
    "guides/mcp-adapter",
    "reference/contracts",
    "reference/contract",
    "releases",
    "status",
    "security",
];
const GROUPS: [&str; 5] = ["Start", "Concepts", "Guides", "Reference", "Project"];

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask has a parent")
        .to_path_buf()
}

fn copy_tree(from: &Path, to: &Path) {
    if from.is_dir() {
        fs::create_dir_all(to).unwrap();
        for entry in fs::read_dir(from).unwrap() {
            let entry = entry.unwrap();
            copy_tree(&entry.path(), &to.join(entry.file_name()));
        }
    } else {
        fs::create_dir_all(to.parent().unwrap()).unwrap();
        fs::copy(from, to).unwrap();
    }
}

/// A root holding the public inputs `check-docs` reads, copied from this checkout, and no Git.
fn input_root() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("Cargo.toml"), "[workspace]\n").unwrap();
    for relative in [
        "CHANGELOG.md",
        "website/docs",
        "website/sidebars.ts",
        "website/static/index.html",
        "website/static/styles.css",
        "website/ux",
        "spec/confinement/confinement.yaml",
        "spec/confinement-suite.json",
        "spec/generated/cli/binding.json",
        "xtask/src",
    ] {
        copy_tree(&repo().join(relative), &dir.path().join(relative));
    }
    dir
}

fn xtask(root: &Path, args: &[&str]) -> Output {
    let ceiling = root.parent().unwrap();
    Command::new(env!("CARGO_BIN_EXE_xtask"))
        .current_dir(root)
        .env("GIT_CEILING_DIRECTORIES", ceiling)
        .args(args)
        .output()
        .unwrap()
}

fn shown(output: &Output) -> String {
    format!(
        "status {:?}\nstdout:\n{}\nstderr:\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn check_docs_passes(root: &Path) {
    let control = xtask(root, &["check-docs"]);
    assert!(
        control.status.success(),
        "control: the copied inputs must pass check-docs\n{}",
        shown(&control)
    );
}

/// The changelog's newest released section is published on the Release notes page. The former
/// renderer parsed that section with the page renderer and refused raw HTML in it, as it does for
/// every authored page ("raw HTML is not admitted in public Markdown"). `docs.rs` now parses only
/// the text above the generated-section marker, and Docusaurus renders `.md` HTML through
/// `rehype-raw`, so HTML in a changelog bullet reaches the published page unchecked.
#[test]
fn raw_html_in_the_changelog_section_is_refused_on_the_release_notes_page() {
    let root = input_root();
    check_docs_passes(root.path());
    let changelog = root.path().join("CHANGELOG.md");
    let text = fs::read_to_string(&changelog).unwrap();
    let section = text.find("\n## [0.").expect("a released section");
    let added = section
        + text[section..]
            .find("### Added\n\n")
            .expect("an Added list")
        + "### Added\n\n".len();
    let mutated = format!(
        "{}- <img src=x onerror=alert(document.domain)> published as HTML.\n{}",
        &text[..added],
        &text[added..]
    );
    fs::write(&changelog, mutated).unwrap();
    let releases = xtask(root.path(), &["docs-releases"]);
    assert!(releases.status.success(), "{}", shown(&releases));
    let page = fs::read_to_string(root.path().join("website/docs/releases.md")).unwrap();
    assert!(
        page.contains("<img src=x onerror="),
        "the HTML is on the page"
    );
    let check = xtask(root.path(), &["check-docs"]);
    assert!(
        !check.status.success(),
        "check-docs admits raw HTML on the published Release notes page\n{}",
        shown(&check)
    );
}

/// The same path for an image: the former renderer refused one ("undeclared public image");
/// the generated section is no longer parsed for it.
#[test]
fn an_image_in_the_changelog_section_is_refused_on_the_release_notes_page() {
    let root = input_root();
    check_docs_passes(root.path());
    let changelog = root.path().join("CHANGELOG.md");
    let text = fs::read_to_string(&changelog).unwrap();
    let section = text.find("\n## [0.").expect("a released section");
    let added = section
        + text[section..]
            .find("### Added\n\n")
            .expect("an Added list")
        + "### Added\n\n".len();
    let mutated = format!(
        "{}- ![tracking pixel](https://example.invalid/pixel.png) published as an image.\n{}",
        &text[..added],
        &text[added..]
    );
    fs::write(&changelog, mutated).unwrap();
    let releases = xtask(root.path(), &["docs-releases"]);
    assert!(releases.status.success(), "{}", shown(&releases));
    let check = xtask(root.path(), &["check-docs"]);
    assert!(
        !check.status.success(),
        "check-docs admits an undeclared image on the published Release notes page\n{}",
        shown(&check)
    );
}

/// The former renderer refused a page with two headings of one ID ("duplicate heading … in …").
/// Docusaurus does not: it renames the later one `<id>-1`, so a heading added above an existing one
/// of the same text takes its published anchor. Nothing refuses it any more.
#[test]
fn a_duplicate_heading_id_on_a_page_is_refused() {
    let root = input_root();
    check_docs_passes(root.path());
    let page = root.path().join("website/docs/status.md");
    let text = fs::read_to_string(&page).unwrap();
    let heading = text
        .lines()
        .find(|line| line.starts_with("## "))
        .expect("status.md has an H2")
        .to_owned();
    fs::write(&page, format!("{text}\n{heading}\n")).unwrap();
    let check = xtask(root.path(), &["check-docs"]);
    assert!(
        !check.status.success(),
        "check-docs admits a second `{heading}` on status.md\n{}",
        shown(&check)
    );
}

/// A page's canonical href, as the built site writes it.
fn href(page: &str) -> String {
    if page == "index" {
        "/substrate/docs".into()
    } else {
        format!("/substrate/docs/{page}")
    }
}

/// Where Docusaurus writes a page under `trailingSlash: false`.
fn file(page: &str) -> String {
    if page == "index" {
        "docs.html".into()
    } else {
        format!("docs/{page}.html")
    }
}

/// The IDs each task in `website/ux/tasks.json` names, and the links that give each task the
/// click count `website/ux/baseline.json` holds: three targets one click from the start page,
/// three two clicks away through the docs index.
fn ids(page: &str) -> &'static [&'static str] {
    match page {
        "guides/cli" => &[
            "install",
            "run-under-a-confinement-adapter-without-a-daemon",
        ],
        "concepts/confinement" => &["refusal-is-part-of-the-contract"],
        "concepts/adapters" => &["pod"],
        "releases" => &["what-changed-in-the-newest-release"],
        "reference/contracts" => &["the-ten-families"],
        _ => &[],
    }
}

/// One built docs page holding every check `docs-provenance` makes: the page list names each
/// group and marks this page, the narrow-screen toggle and the wide-screen container are there,
/// the content sits in `#main`, the H1 has its ID, and every table is framed.
fn docs_page(page: &str) -> String {
    let groups: String = GROUPS
        .iter()
        .map(|group| ["<span class=label>", group, "</span>"].concat())
        .collect();
    let headings: String = ids(page)
        .iter()
        .map(|id| ["<h2 id=", id, ">", id, "</h2>"].concat())
        .collect();
    // The frame is a focusable region named by the heading above the table (UXF-004).
    let label = ids(page).last().copied().unwrap_or("T");
    let links = if page == "index" {
        "<a href=/substrate/docs/guides/cli>cli</a><a href=/substrate/docs/concepts/confinement>c</a>"
    } else {
        ""
    };
    format!(
        "<!doctype html><html><body><nav class=navbar><button class=\"navbar__toggle clean-btn\" type=button></button></nav><div id=main><aside class=theme-doc-sidebar-container>{groups}<a class=\"menu__link menu__link--active\" aria-current=page href={}>here</a></aside><article><h1 id=page-title>T</h1>{headings}<div class=b10x-table-wrap role=region tabindex=0 aria-label=\"{label} table\"><table><tr><td>x</td></tr></table></div>{links}</article></div></body></html>",
        href(page)
    )
}

/// A built site `docs-provenance` accepts, and the root holding the UX ratchet it reads.
fn built_site() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("root");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("Cargo.toml"), "[workspace]\n").unwrap();
    copy_tree(
        &repo().join("website/ux/tasks.json"),
        &root.join("website/ux/tasks.json"),
    );
    copy_tree(
        &repo().join("website/ux/baseline.json"),
        &root.join("website/ux/baseline.json"),
    );
    let site = dir.path().join("build");
    fs::create_dir_all(site.join("docs")).unwrap();
    fs::write(
        site.join("index.html"),
        "<!doctype html><html><body><main id=main><a href=/substrate/docs>docs</a><a href=/substrate/docs/concepts/adapters>a</a><a href=/substrate/docs/reference/contracts>c</a><a href=/substrate/docs/releases>r</a></main></body></html>",
    )
    .unwrap();
    for page in PAGES {
        let path = site.join(file(page));
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, docs_page(page)).unwrap();
    }
    (dir, root, site)
}

fn provenance(root: &Path, site: &Path) -> Output {
    let commit = "b".repeat(40);
    xtask(
        root,
        &[
            "docs-provenance",
            "--site",
            site.to_str().unwrap(),
            "--commit",
            &commit,
        ],
    )
}

fn inventory_paths(site: &Path) -> Vec<String> {
    let text = fs::read_to_string(site.join(".well-known/b10x-routes.json")).unwrap();
    let value: serde_json::Value = serde_json::from_str(&text).unwrap();
    value["routes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|route| route["path"].as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn the_hand_built_site_is_accepted_and_inventories_exactly_the_published_routes() {
    let (_dir, root, site) = built_site();
    let output = provenance(&root, &site);
    assert!(output.status.success(), "{}", shown(&output));
    assert_eq!(inventory_paths(&site).len(), 21);
}

/// The former build wrote exactly the start page and `PAGES`, and its test held the inventory to
/// that set. A Docusaurus build publishes whatever `website/static/` holds, and nothing in
/// `check-docs` or `docs-provenance` compares the built routes with `PAGES`: an undeclared page is
/// checked by nothing, inventoried as a published route, and uploaded.
#[test]
fn a_built_page_outside_the_declared_routes_is_refused() {
    let (_dir, root, site) = built_site();
    fs::create_dir_all(site.join("internal")).unwrap();
    fs::write(
        site.join("internal/plan.html"),
        "<!doctype html><html><body><h1 id=plan>work log</h1></body></html>",
    )
    .unwrap();
    let output = provenance(&root, &site);
    let listed = if output.status.success() {
        inventory_paths(&site)
    } else {
        Vec::new()
    };
    assert!(
        !output.status.success(),
        "docs-provenance bound an undeclared page into the inventory: {listed:?}\n{}",
        shown(&output)
    );
}

/// `src/theme/DocItem/Layout` exists only to keep `#main`, "every docs page published #main …
/// and the route inventory keeps every published anchor". The former test asserted `main` on
/// every inventoried route; nothing checks it now, so a page that loses it is bound as is.
#[test]
fn a_docs_page_without_the_main_anchor_is_refused() {
    let (_dir, root, site) = built_site();
    let path = site.join(file("guides/deployment"));
    let html = fs::read_to_string(&path)
        .unwrap()
        .replace("<div id=main>", "<div>");
    fs::write(&path, html).unwrap();
    let output = provenance(&root, &site);
    assert!(
        !output.status.success(),
        "docs-provenance bound /substrate/docs/guides/deployment/ without #main\n{}",
        shown(&output)
    );
}

/// `src/theme/Heading` exists only to keep each page's H1 ID, "a published anchor that links and
/// redirects into this site already use". Nothing checks the built H1 has one.
#[test]
fn a_docs_page_whose_h1_lost_its_id_is_refused() {
    let (_dir, root, site) = built_site();
    let path = site.join(file("concepts/boundary"));
    let html = fs::read_to_string(&path)
        .unwrap()
        .replace("<h1 id=page-title>", "<h1>");
    fs::write(&path, html).unwrap();
    let output = provenance(&root, &site);
    assert!(
        !output.status.success(),
        "docs-provenance bound /substrate/docs/concepts/boundary/ with an H1 that has no ID\n{}",
        shown(&output)
    );
}
