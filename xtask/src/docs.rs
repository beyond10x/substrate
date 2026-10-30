//! The public site's inputs and its built-site provenance.
//!
//! Docusaurus renders the site from `website/` on the shared docs-system layer. What runs here is
//! what needs no Node toolchain: the page allowlist and its agreement with `website/sidebars.ts`,
//! the release notes generated from `CHANGELOG.md`, the static start page's links, and the
//! Markdown measurements `site_drift` and `site_ux` hold. After `npm run build`,
//! `cargo xtask docs-provenance` checks the built pages and binds them to their source commit.
use anyhow::{Context, Result, bail, ensure};
use clap::Args;
use pulldown_cmark::{Event, LinkType, Options, Parser, Tag, TagEnd};
use serde_json::json;
use std::fmt::Write as _;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Component, Path, PathBuf},
    process::ExitCode,
};

/// Every published guide: its path below `website/docs/`, its navigation title and its group.
pub(crate) const PAGES: &[(&str, &str, &str)] = &[
    ("index", "Overview", "Start"),
    ("getting-started", "Get started", "Start"),
    ("use-cases", "Use cases", "Start"),
    ("concepts/boundary", "System boundary", "Concepts"),
    ("concepts/confinement", "Confinement", "Concepts"),
    ("concepts/adapters", "Confinement adapters", "Concepts"),
    ("concepts/operations", "Operations", "Concepts"),
    ("concepts/model", "System model", "Concepts"),
    ("guides/run-a-command", "Run a command", "Guides"),
    (
        "guides/confinement-profiles",
        "Confinement profiles",
        "Guides",
    ),
    (
        "guides/storage-and-metrics",
        "Storage and metrics",
        "Guides",
    ),
    ("guides/deployment", "Deployment", "Guides"),
    ("guides/rust-sdk", "Rust SDK", "Guides"),
    ("guides/cli", "Command line", "Guides"),
    ("guides/mcp-adapter", "MCP adapter", "Guides"),
    ("guides/remote-conformance", "Remote conformance", "Guides"),
    ("reference/contracts", "Contracts overview", "Reference"),
    ("reference/contract", "Wire contract", "Reference"),
    ("releases", "Release notes", "Reference"),
    ("status", "Status", "Project"),
    ("security", "Security", "Project"),
];
/// The navigation groups, in the order the sidebar shows them.
const GROUPS: [&str; 5] = ["Start", "Concepts", "Guides", "Reference", "Project"];
/// The page whose tail is the newest released section of `CHANGELOG.md`.
const RELEASES: &str = "releases";
const RELEASES_FILE: &str = "website/docs/releases.md";
/// The line after which `website/docs/releases.md` is generated; everything above it is authored.
const RELEASES_MARKER: &str = "<!-- Generated from CHANGELOG.md by `cargo xtask docs-releases`. Edit CHANGELOG.md, not the text below. -->";
/// Where a repository-relative link inside the changelog points once it is published.
const SOURCE_BLOB: &str = "https://github.com/beyond10x/substrate/blob/main/";
/// The start page: authored HTML that Docusaurus copies unchanged to the site root.
pub(crate) const START_PAGE: &str = "website/static/index.html";
const SIDEBARS: &str = "website/sidebars.ts";
/// Where the site is served, with both slashes.
pub(crate) const BASE: &str = "/substrate/";

#[derive(Debug, Args)]
pub struct ProvenanceArgs {
    /// The directory `npm --prefix website run build` wrote.
    #[arg(long)]
    site: PathBuf,
    #[arg(long)]
    commit: String,
}

/// One parsed Markdown source: the word count of each text block (a paragraph, a list item's own
/// text, a table cell), its tables, each under the ID of the heading above it, and its text.
#[derive(Debug, Default)]
pub(crate) struct Rendered {
    pub blocks: Vec<usize>,
    pub tables: Vec<(String, Vec<Vec<String>>)>,
    pub text: String,
}

/// A page's route in the inventory's `/x/` form.
pub(crate) fn route(page: &str) -> String {
    if page == "index" {
        format!("{BASE}docs/")
    } else {
        format!("{BASE}docs/{page}/")
    }
}
/// A heading's ID as the docs site writes it for the headings these pages use.
fn slug(text: &str) -> String {
    text.to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == ' ' || *c == '-')
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join("-")
}
/// A relative link from `page` must stay inside the docs and name a published guide.
fn link(page: &str, target: &str) -> Result<String> {
    if target.starts_with("https://") || target.starts_with('#') {
        return Ok(target.into());
    }
    ensure!(
        !target.starts_with('/'),
        "unexpected absolute link {target}"
    );
    let (path, anchor) = target.split_once('#').unwrap_or((target, ""));
    let path = Path::new(page).parent().unwrap_or(Path::new("")).join(path);
    let mut parts = Vec::new();
    for part in path.components() {
        match part {
            Component::Normal(p) => parts.push(p.to_string_lossy().into_owned()),
            Component::ParentDir => {
                ensure!(parts.pop().is_some(), "link escapes public docs");
            }
            Component::CurDir => (),
            _ => bail!("invalid public link"),
        }
    }
    let path = parts.join("/");
    let name = path
        .strip_suffix(".md")
        .context("public links must name a Markdown guide")?;
    ensure!(
        PAGES.iter().any(|(p, _, _)| *p == name),
        "undeclared public page {name}"
    );
    Ok(format!(
        "{}{}",
        route(name),
        if anchor.is_empty() {
            String::new()
        } else {
            format!("#{anchor}")
        }
    ))
}
/// A repository-relative link from the changelog, as a link into the source on GitHub.
fn repository_link(target: &str) -> Result<String> {
    if target.starts_with("https://") || target.starts_with('#') {
        return Ok(target.into());
    }
    let path = Path::new(target.split_once('#').map_or(target, |(path, _)| path));
    ensure!(
        path.components().all(|c| matches!(c, Component::Normal(_))),
        "changelog link {target} must be repository-relative"
    );
    Ok(format!("{SOURCE_BLOB}{target}"))
}
fn markdown(page: &str, source: &str) -> Result<Rendered> {
    let body = source
        .strip_prefix("---\n")
        .and_then(|s| s.split_once("\n---\n").map(|(_, body)| body))
        .context("missing document frontmatter")?;
    // One page shares one set of heading IDs: Docusaurus renames a repeated one `<id>-1`, which
    // moves a published anchor, so a repeat is refused.
    let mut ids = BTreeSet::new();
    if page != RELEASES {
        return render(page, body, &mut ids);
    }
    let (authored, generated) = body
        .split_once(RELEASES_MARKER)
        .with_context(|| format!("{RELEASES_FILE} has no generated-section marker"))?;
    let rendered = render(page, authored, &mut ids)?;
    // The generated release section is the changelog's text, not the site's to shorten (UXF-007),
    // so it is not measured. It is published on this page all the same, so the same guards hold
    // for it: no raw HTML, no image, no undeclared link and no repeated heading ID.
    render(page, generated, &mut ids)?;
    Ok(rendered)
}
fn render(page: &str, source: &str, ids: &mut BTreeSet<String>) -> Result<Rendered> {
    let mut rendered = Rendered::default();
    // Open text blocks, innermost last, each with its word count so far.
    let mut open: Vec<usize> = Vec::new();
    let mut heading: Option<String> = None;
    let mut current = String::new();
    let mut table: Option<Vec<Vec<String>>> = None;
    let mut cell: Option<String> = None;
    let words = |text: &str| text.split_whitespace().count();
    for event in Parser::new_ext(
        source,
        Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH,
    ) {
        match event {
            Event::Start(Tag::Heading { .. }) => heading = Some(String::new()),
            Event::End(TagEnd::Heading(_)) => {
                current = slug(&heading.take().unwrap_or_default());
                ensure!(
                    ids.insert(current.clone()),
                    "duplicate heading {current} in {page}"
                );
            }
            Event::Start(Tag::Link { dest_url, .. }) => {
                link(page, &dest_url)?;
            }
            Event::Html(_) | Event::InlineHtml(_) => {
                bail!("raw HTML is not admitted in public Markdown: {page}")
            }
            Event::Start(Tag::Image { .. }) => bail!("undeclared public image in {page}"),
            Event::Start(Tag::Table(_)) => table = Some(Vec::new()),
            Event::End(TagEnd::Table) => {
                if let Some(rows) = table.take() {
                    rendered.tables.push((current.clone(), rows));
                }
            }
            Event::Start(Tag::TableHead | Tag::TableRow) => {
                if let Some(rows) = table.as_mut() {
                    rows.push(Vec::new());
                }
            }
            Event::Start(tag @ (Tag::Paragraph | Tag::Item | Tag::TableCell)) => {
                if matches!(tag, Tag::TableCell) {
                    cell = Some(String::new());
                }
                open.push(0);
            }
            Event::End(end @ (TagEnd::Paragraph | TagEnd::Item | TagEnd::TableCell)) => {
                if let Some(count) = open.pop().filter(|count| *count > 0) {
                    rendered.blocks.push(count);
                }
                if matches!(end, TagEnd::TableCell)
                    && let (Some(rows), Some(text)) = (table.as_mut(), cell.take())
                    && let Some(row) = rows.last_mut()
                {
                    row.push(text.trim().to_owned());
                }
            }
            Event::Text(text) | Event::Code(text) => {
                if let Some(heading) = heading.as_mut() {
                    heading.push_str(&text);
                }
                if let Some(count) = open.last_mut() {
                    *count += words(&text);
                }
                if let Some(cell) = cell.as_mut() {
                    cell.push_str(&text);
                }
                rendered.text.push_str(&text);
            }
            Event::SoftBreak | Event::HardBreak => rendered.text.push('\n'),
            _ => (),
        }
    }
    Ok(rendered)
}
pub(crate) fn read_public(root: &Path, relative: &str) -> Result<String> {
    let mut path = root.to_path_buf();
    for component in Path::new(relative).components() {
        ensure!(
            matches!(component, Component::Normal(_)),
            "invalid public input"
        );
        path.push(component);
        ensure!(
            !fs::symlink_metadata(&path)?.file_type().is_symlink(),
            "public input is a symlink: {relative}"
        );
    }
    ensure!(
        fs::metadata(&path)?.is_file(),
        "public input is not a regular file: {relative}"
    );
    Ok(fs::read_to_string(path)?)
}
/// The newest released section of a Keep a Changelog file, as Markdown whose headings name the
/// version, so the sections of one page never share a heading ID, and whose repository-relative
/// links point into the source on GitHub.
fn newest_release(changelog: &str) -> Result<String> {
    let mut lines = changelog
        .lines()
        .skip_while(|line| !line.starts_with("## [") || line.starts_with("## [Unreleased]"));
    let heading = lines
        .next()
        .context("CHANGELOG.md has no released section")?;
    let (version, date) = heading
        .strip_prefix("## [")
        .and_then(|rest| rest.split_once(']'))
        .context("a release heading reads `## [<version>] — <date>`")?;
    let date = date.trim().trim_start_matches('—').trim();
    ensure!(
        !version.is_empty() && !date.is_empty(),
        "a release heading reads `## [<version>] — <date>`"
    );
    let mut section = format!("## {version} ({date})\n");
    for line in lines.take_while(|line| !line.starts_with("## [")) {
        match line.strip_prefix("### ") {
            Some(kind) => writeln!(section, "### {kind} in {version}")?,
            None => writeln!(section, "{line}")?,
        }
    }
    absolute_links(section.trim_end())
}
/// `section` with every inline link's destination passed through `repository_link`.
fn absolute_links(section: &str) -> Result<String> {
    let mut out = String::new();
    let mut at = 0;
    for (event, range) in Parser::new_ext(section, Options::ENABLE_TABLES).into_offset_iter() {
        let Event::Start(Tag::Link {
            link_type,
            dest_url,
            ..
        }) = event
        else {
            continue;
        };
        let target = repository_link(&dest_url)?;
        if target == *dest_url {
            continue;
        }
        ensure!(
            link_type == LinkType::Inline,
            "changelog link {dest_url} must be an inline link"
        );
        let needle = format!("]({dest_url}");
        let found = section[range.clone()].rfind(&needle).with_context(|| {
            format!("changelog link {dest_url} is not written `[…]({dest_url})`")
        })?;
        let start = range.start + found + 2;
        out.push_str(&section[at..start]);
        out.push_str(&target);
        at = start + dest_url.len();
    }
    out.push_str(&section[at..]);
    out.push('\n');
    Ok(out)
}
/// `website/docs/releases.md` as it must read: its authored part up to the marker, then the
/// newest released section of `CHANGELOG.md`.
fn release_notes(root: &Path) -> Result<String> {
    let current = read_public(root, RELEASES_FILE)?;
    let (authored, _) = current
        .split_once(RELEASES_MARKER)
        .with_context(|| format!("{RELEASES_FILE} has no generated-section marker"))?;
    Ok(format!(
        "{authored}{RELEASES_MARKER}\n\n{}",
        newest_release(&read_public(root, "CHANGELOG.md")?)?
    ))
}
/// Rewrite `website/docs/releases.md` from `CHANGELOG.md`.
pub fn write_release_notes() -> Result<ExitCode> {
    let root = crate::repo::root()?;
    let notes = release_notes(&root)?;
    let changed = read_public(&root, RELEASES_FILE)? != notes;
    if changed {
        fs::write(root.join(RELEASES_FILE), notes)?;
    }
    println!(
        "{RELEASES_FILE}: {}",
        if changed { "rewritten" } else { "unchanged" }
    );
    Ok(ExitCode::SUCCESS)
}
/// Every parsed Markdown page, keyed by its path below `website/docs/`.
pub(crate) fn rendered(root: &Path) -> Result<BTreeMap<&'static str, Rendered>> {
    let mut rendered = BTreeMap::new();
    for (page, _, _) in PAGES {
        rendered.insert(
            *page,
            markdown(
                page,
                &read_public(root, &format!("website/docs/{page}.md"))?,
            )?,
        );
    }
    Ok(rendered)
}
/// Every Markdown file below `website/docs/`, as a page path. Docusaurus publishes each one, so
/// one that `PAGES` does not list would be an undeclared page.
fn docs_files(root: &Path) -> Result<BTreeSet<String>> {
    fn walk(dir: &Path, prefix: &str, out: &mut BTreeSet<String>) -> Result<()> {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let kind = entry.file_type()?;
            if kind.is_dir() {
                walk(&entry.path(), &format!("{prefix}{name}/"), out)?;
            } else if let Some(page) = name
                .strip_suffix(".md")
                .or_else(|| name.strip_suffix(".mdx"))
            {
                out.insert(format!("{prefix}{page}"));
            }
        }
        Ok(())
    }
    let mut out = BTreeSet::new();
    walk(&root.join("website/docs"), "", &mut out)?;
    Ok(out)
}
/// The sidebar as `website/sidebars.ts` declares it: each category label, then each
/// `page('<id>', '<label>')` under it, in source order.
fn sidebar(source: &str) -> Vec<(String, String, String)> {
    let quoted = |text: &str| -> Option<String> {
        let rest = text.strip_prefix('\'')?;
        rest.split_once('\'').map(|(value, _)| value.to_owned())
    };
    let mut group = String::new();
    let mut pages = Vec::new();
    for line in source.lines() {
        if let Some(label) = line.trim().strip_prefix("label: ").and_then(quoted) {
            group = label;
        }
        for call in line.split("page(").skip(1) {
            let Some(id) = quoted(call) else { continue };
            let label = call
                .split_once(", ")
                .and_then(|(_, rest)| quoted(rest))
                .unwrap_or_default();
            pages.push((id, label, group.clone()));
        }
    }
    pages
}
/// The sidebar lists `PAGES` in order, under their groups, with their titles, and nothing else
/// is published from `website/docs/`.
fn structure(root: &Path) -> Result<Vec<String>> {
    let mut failures = Vec::new();
    let declared = sidebar(&read_public(root, SIDEBARS)?);
    let expected: Vec<(String, String, String)> = PAGES
        .iter()
        .map(|(page, title, group)| ((*page).into(), (*title).into(), (*group).into()))
        .collect();
    if declared != expected {
        failures.push(format!(
            "{SIDEBARS}: the sidebar is {declared:?}, PAGES declares {expected:?}"
        ));
    }
    let groups: Vec<&str> = PAGES.iter().fold(Vec::new(), |mut groups, (_, _, group)| {
        if groups.last() != Some(group) {
            groups.push(group);
        }
        groups
    });
    if groups != GROUPS {
        failures.push(format!(
            "PAGES groups run {groups:?}, the sidebar order is {GROUPS:?}"
        ));
    }
    let listed: BTreeSet<String> = PAGES.iter().map(|(page, _, _)| (*page).into()).collect();
    for extra in docs_files(root)?.difference(&listed) {
        failures.push(format!(
            "website/docs/{extra}.md is not a declared public page"
        ));
    }
    Ok(failures)
}
/// Every local link on the static start page names a published route, the stylesheet beside it
/// or an ID on the page itself. Docusaurus does not check a static file's links.
fn start_page(root: &Path) -> Result<Vec<String>> {
    let html = read_public(root, START_PAGE)?;
    ensure!(!html.contains("<script"), "{START_PAGE} runs a script");
    let ids = attribute_values(&html, "id");
    let mut failures = Vec::new();
    for href in attribute_values(&html, "href") {
        if href.starts_with("https://") || href == "/substrate/styles.css" || href == BASE {
            continue;
        }
        if let Some(anchor) = href.strip_prefix('#') {
            if !ids.contains(anchor) {
                failures.push(format!("{START_PAGE}: no #{anchor} on the page"));
            }
            continue;
        }
        let path = href.split_once('#').map_or(href.as_str(), |(path, _)| path);
        let known = PAGES
            .iter()
            .any(|(page, _, _)| route(page).trim_end_matches('/') == path);
        if !known {
            failures.push(format!(
                "{START_PAGE}: {href} is not a published route in its canonical form"
            ));
        }
    }
    Ok(failures)
}
pub fn check(root: &Path) -> Result<crate::report::Report> {
    let rendered = rendered(root)?;
    let mut failures = structure(root)?;
    if read_public(root, RELEASES_FILE)? != release_notes(root)? {
        failures.push(format!(
            "{RELEASES_FILE} does not end with the newest released CHANGELOG.md section; run `cargo xtask docs-releases`"
        ));
    }
    failures.extend(start_page(root)?);
    read_public(root, "website/static/styles.css")?;
    failures.extend(crate::site_drift::check(root, &rendered)?);
    failures.extend(crate::site_ux::check(root, &rendered)?);
    if !failures.is_empty() {
        return Ok(crate::report::Report::failed(failures));
    }
    Ok(crate::report::Report::passed(format!(
        "Substrate documentation: {} pages match the sidebar, the release notes match CHANGELOG.md, the start page's links resolve, derived tables match the specification, the reading-load ratchet holds",
        rendered.len()
    )))
}

/// Every value of an attribute in `html`, quoted or not (the build minifies its HTML).
pub(crate) fn attribute_values(html: &str, name: &str) -> BTreeSet<String> {
    attribute_list(html, name).into_iter().collect()
}
/// Every value of an attribute in `html`, in document order.
pub(crate) fn attribute_list(html: &str, name: &str) -> Vec<String> {
    let mut found = Vec::new();
    let needle = format!(" {name}=");
    let mut rest = html;
    while let Some(at) = rest.find(&needle) {
        rest = &rest[at + needle.len()..];
        let value = match rest.as_bytes().first() {
            Some(quote @ (b'"' | b'\'')) => {
                let quote = char::from(*quote);
                rest[1..].split(quote).next().unwrap_or_default()
            }
            _ => rest
                .split(|c: char| c.is_whitespace() || c == '>')
                .next()
                .unwrap_or_default(),
        };
        if !value.is_empty() {
            found.push(value.to_owned());
        }
    }
    found
}
/// Whether `html` is a copy that only sends the reader on: a refresh, or the copy docs-system
/// writes at `x/index.html` for `x.html` under `trailingSlash: false`.
fn is_redirect(html: &str) -> bool {
    html.contains("http-equiv=\"refresh\"")
        || html.contains("http-equiv=refresh")
        || html.contains("<!-- b10x-trailing-slash-copy -->")
}
/// The route a built file answers, in the inventory's `/x/` form, or `None` for no page.
fn built_route(relative: &str) -> Option<String> {
    if relative == "404.html" {
        return None;
    }
    if let Some(dir) = relative.strip_suffix("index.html") {
        return Some(format!("{BASE}{dir}"));
    }
    relative
        .strip_suffix(".html")
        .map(|page| format!("{BASE}{page}/"))
}
/// The built site at `site`: every page keyed by its route, the route of every copy that only
/// sends the reader on, and every file, relative to `site`.
#[derive(Debug, Default)]
pub(crate) struct BuiltSite {
    pub pages: BTreeMap<String, String>,
    pub copies: BTreeSet<String>,
    pub files: Vec<String>,
}
pub(crate) fn built_site(site: &Path) -> Result<BuiltSite> {
    fn walk(site: &Path, dir: &Path, out: &mut BuiltSite) -> Result<()> {
        let mut entries: Vec<_> = fs::read_dir(dir)
            .with_context(|| format!("reading {}", dir.display()))?
            .collect::<std::result::Result<_, _>>()?;
        entries.sort_by_key(fs::DirEntry::path);
        for entry in entries {
            let path = entry.path();
            if entry.file_type()?.is_dir() {
                walk(site, &path, out)?;
                continue;
            }
            let relative = path
                .strip_prefix(site)?
                .to_str()
                .context("non-UTF-8 path in the built site")?
                .replace('\\', "/");
            out.files.push(relative.clone());
            let Some(route) = built_route(&relative) else {
                continue;
            };
            let html = fs::read_to_string(&path)?;
            if is_redirect(&html) {
                out.copies.insert(route);
                continue;
            }
            ensure!(
                out.pages.insert(route.clone(), html).is_none(),
                "two built files answer {route}"
            );
        }
        Ok(())
    }
    let mut built = BuiltSite::default();
    walk(site, site, &mut built)?;
    ensure!(
        built.pages.contains_key(BASE),
        "the built site has no start page"
    );
    Ok(built)
}
/// Every page of the built site at `site`, keyed by its route.
pub(crate) fn built_pages(site: &Path) -> Result<BTreeMap<String, String>> {
    Ok(built_site(site)?.pages)
}
/// The routes the site publishes: the start page and `PAGES`.
fn declared_routes() -> BTreeSet<String> {
    std::iter::once(BASE.to_owned())
        .chain(PAGES.iter().map(|(page, _, _)| route(page)))
        .collect()
}
/// Extensions a static host serves as an HTML page. Docusaurus writes `.html`; a file with one of
/// these is a page nothing here checks.
const OTHER_PAGE_EXTENSIONS: [&str; 3] = ["htm", "xhtml", "shtml"];
/// The built site holds the declared routes and nothing else: a page Docusaurus copied from
/// `website/static/`, or a copy that sends the reader to one, is an undeclared page that no check
/// reads and the inventory would publish.
fn only_declared_routes(built: &BuiltSite) -> Vec<String> {
    let declared = declared_routes();
    let mut failures = Vec::new();
    for route in built.pages.keys().chain(&built.copies) {
        if !declared.contains(route) {
            failures.push(format!("{route}: built, but not a declared public page"));
        }
    }
    for file in &built.files {
        let other = Path::new(file).extension().is_some_and(|extension| {
            OTHER_PAGE_EXTENSIONS
                .iter()
                .any(|other| extension.eq_ignore_ascii_case(other))
        });
        if other {
            failures.push(format!("{file}: built, but not a declared public page"));
        }
    }
    failures
}
/// Whether `id` is one React or Docusaurus generates for its own use: a `useId` value, which
/// changes with the component tree (`_R_…_` in React 19.1, `«r…»` in 19.0, `:r…:` before), or a
/// framework element. None of them is an anchor a link may name.
fn generated_id(id: &str) -> bool {
    const FRAMEWORK: [&str; 1] = ["theme-svg-external-link"];
    id.contains("_R_")
        || id.contains('«')
        || id.contains(":r")
        || id.starts_with("__docusaurus")
        || FRAMEWORK.contains(&id)
}
/// The IDs of `html` the route inventory publishes: every element ID but the generated ones.
fn published_anchors(html: &str) -> BTreeSet<String> {
    attribute_values(html, "id")
        .into_iter()
        .filter(|id| !generated_id(id))
        .collect()
}
/// The published route and anchor inventory: every page with its sorted published IDs, stamped
/// with the same commit as the site provenance. The organization Website reads it to redirect
/// the former `/docs/substrate/` pages and to check links into this site.
fn inventory(pages: &BTreeMap<String, String>, commit: &str) -> Result<Vec<u8>> {
    let routes: Vec<_> = pages
        .iter()
        .map(|(path, page)| json!({"path": path, "anchors": published_anchors(page)}))
        .collect();
    Ok(serde_json::to_vec_pretty(&json!({
        "schema": "b10x-project-routes/v1",
        "repository": "substrate",
        "commit": commit,
        "baseUrl": BASE,
        "routes": routes,
    }))?)
}
/// The `<a …>` tag around byte `at` of `html`, when there is one.
fn enclosing_link(html: &str, at: usize) -> Option<&str> {
    let start = html[..at].rfind("<a ")?;
    let end = start + html[start..].find('>')?;
    (end >= at).then(|| &html[start..=end])
}
/// UXF-001: the page list names every group and marks the page the reader is on, and nothing
/// else, as the current page.
fn marks_current_page(route: &str, html: &str) -> Vec<String> {
    let mut failures = Vec::new();
    let canonical = route.trim_end_matches('/');
    let mut own = 0;
    for (at, _) in html.match_indices("aria-current=") {
        let href = enclosing_link(html, at)
            .and_then(|tag| attribute_list(tag, "href").into_iter().next())
            .unwrap_or_default();
        if href == canonical {
            own += 1;
        } else {
            failures.push(format!("{route}: {href:?} is marked as the current page"));
        }
    }
    if own == 0 {
        failures.push(format!("{route}: the page list does not mark this page"));
    }
    for group in GROUPS {
        if !html.contains(&format!(">{group}</span>")) {
            failures.push(format!("{route}: the page list does not name {group}"));
        }
    }
    failures
}
/// UXF-002: on a narrow screen the page list is the navbar's drawer, closed until opened, and
/// the sidebar beside the article is the wide-screen container only.
fn page_list_is_closed_on_narrow_screens(route: &str, html: &str) -> Vec<String> {
    let mut failures = Vec::new();
    if !html.contains("theme-doc-sidebar-container") {
        failures.push(format!("{route}: no wide-screen page list"));
    }
    if !html.contains("navbar__toggle") {
        failures.push(format!("{route}: no narrow-screen page-list toggle"));
    }
    if html.contains("navbar-sidebar--show") {
        failures.push(format!("{route}: the narrow-screen page list starts open"));
    }
    failures
}
/// `text` with the character references the build writes replaced by their characters.
fn decode(text: &str) -> String {
    text.replace("&quot;", "\"")
        .replace("&#x27;", "'")
        .replace("&#39;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}
/// The text of the last heading in `html`, without its markup and its direct-link mark.
fn last_heading(html: &str) -> Option<String> {
    let start = (1..=6)
        .filter_map(|level| {
            let open = format!("<h{level}");
            html.match_indices(open.as_str())
                .filter(|(at, _)| matches!(html.as_bytes().get(at + open.len()), Some(b' ' | b'>')))
                .map(|(at, _)| (at, level))
                .last()
        })
        .max()?;
    let (at, level) = start;
    let inner = &html[at..];
    let inner = &inner[inner.find('>')? + 1..];
    let inner = &inner[..inner.find(&format!("</h{level}>")).unwrap_or(inner.len())];
    let mut text = String::new();
    let mut in_tag = false;
    for c in inner.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            '\u{200b}' => (),
            c if !in_tag => text.push(c),
            _ => (),
        }
    }
    Some(decode(text.trim()))
}
/// UXF-004: every table scrolls inside the site's frame (`website/src/theme/MDXComponents`),
/// which the server-rendered page already makes a keyboard-focusable region named by the heading
/// above the table, as `<heading> table`, or `Table` with no heading above it.
fn tables_scroll_in_a_frame(route: &str, html: &str) -> Vec<String> {
    let mut failures = Vec::new();
    for (at, _) in html.match_indices("<table") {
        let before = html[..at].trim_end();
        let tag = before
            .rfind('<')
            .map(|start| &before[start..])
            .filter(|tag| tag.starts_with("<div ") && tag.ends_with('>'))
            .unwrap_or_default();
        let value = |name: &str| attribute_list(tag, name).into_iter().next();
        let framed = value("class")
            .is_some_and(|class| class.split_whitespace().any(|c| c == "b10x-table-wrap"));
        if !framed {
            failures.push(format!("{route}: a table outside the scrolling frame"));
            continue;
        }
        let expected = match last_heading(&html[..at]) {
            Some(heading) if !heading.is_empty() => format!("{heading} table"),
            _ => "Table".to_owned(),
        };
        let label = value("aria-label").map(|label| decode(&label));
        if value("role").as_deref() != Some("region")
            || value("tabindex").as_deref() != Some("0")
            || label.as_deref() != Some(expected.as_str())
        {
            failures.push(format!(
                "{route}: the frame of a table is not a focusable region named {expected:?} ({tag})"
            ));
        }
    }
    failures
}
/// Every link on the start page into the site reaches a built page and, when it names one, an ID
/// on that page. Docusaurus checks its own pages' links, not a static file's.
fn start_page_links_resolve(pages: &BTreeMap<String, String>) -> Vec<String> {
    let mut failures = Vec::new();
    let Some(start) = pages.get(BASE) else {
        return vec![format!("{BASE}: not built")];
    };
    for href in attribute_list(start, "href") {
        let stylesheet = Path::new(&href)
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("css"));
        if !href.starts_with(BASE) || stylesheet {
            continue;
        }
        let (path, anchor) = href.split_once('#').unwrap_or((href.as_str(), ""));
        let route = if path.ends_with('/') {
            path.to_owned()
        } else {
            format!("{path}/")
        };
        match pages.get(&route) {
            None => failures.push(format!("{BASE}: {href} reaches no built page")),
            Some(page) if !anchor.is_empty() && !attribute_values(page, "id").contains(anchor) => {
                failures.push(format!("{BASE}: {href} names no ID on {route}"));
            }
            Some(_) => (),
        }
    }
    failures
}
/// The published anchors every docs page keeps: `#main` (`src/theme/DocItem/Layout`) and the
/// ID of each H1 (`src/theme/Heading`), which links and redirects into this site already use.
fn keeps_page_anchors(route: &str, html: &str) -> Vec<String> {
    let mut failures = Vec::new();
    if !attribute_values(html, "id").contains("main") {
        failures.push(format!("{route}: no #main"));
    }
    let h1: Vec<&str> = html
        .match_indices("<h1")
        .filter(|(at, _)| matches!(html.as_bytes().get(at + 3), Some(b' ' | b'>')))
        .map(|(at, _)| html[at..].split_inclusive('>').next().unwrap_or_default())
        .collect();
    if h1.is_empty() {
        failures.push(format!("{route}: no H1"));
    }
    for tag in h1 {
        if attribute_list(tag, "id").is_empty() {
            failures.push(format!("{route}: an H1 without an ID ({tag})"));
        }
    }
    failures
}
/// The built-site checks: the built routes are the declared ones, the start page's links
/// resolve, and every docs page keeps its anchors and holds UXF-001, UXF-002 and UXF-004.
fn check_built(built: &BuiltSite) -> Vec<String> {
    let pages = &built.pages;
    let mut failures = only_declared_routes(built);
    failures.extend(start_page_links_resolve(pages));
    for (page, _, _) in PAGES {
        let route = route(page);
        let Some(html) = pages.get(&route) else {
            failures.push(format!("{route}: not built"));
            continue;
        };
        failures.extend(keeps_page_anchors(&route, html));
        failures.extend(marks_current_page(&route, html));
        failures.extend(page_list_is_closed_on_narrow_screens(&route, html));
        failures.extend(tables_scroll_in_a_frame(&route, html));
    }
    failures
}
/// Where a home directory starts on the hosts that build this site.
const HOME_PREFIXES: [&str; 2] = ["/home/", "/Users/"];
/// Every built file that names a home directory or `checkout`: a path of the machine that built
/// the site, which the site would publish (Docusaurus writes its whole configuration, absolute
/// static directories included, into the client bundle).
fn absolute_paths(site: &Path, checkout: &Path) -> Result<Vec<String>> {
    let checkout = checkout.to_string_lossy().into_owned();
    let mut failures = Vec::new();
    for file in built_site(site)?.files {
        let bytes = fs::read(site.join(&file))?;
        let contains = |needle: &[u8]| {
            !needle.is_empty() && bytes.windows(needle.len()).any(|window| window == needle)
        };
        let home = HOME_PREFIXES.iter().find(|prefix| {
            bytes.windows(prefix.len() + 1).any(|window| {
                window.starts_with(prefix.as_bytes())
                    && window[prefix.len()].is_ascii_alphanumeric()
            })
        });
        if contains(checkout.as_bytes()) {
            failures.push(format!("{file}: names the checkout {checkout}"));
        } else if let Some(prefix) = home {
            failures.push(format!("{file}: names a home directory under {prefix}"));
        }
    }
    Ok(failures)
}
/// The pages whose diagrams are `mermaid` fences.
#[cfg(test)]
const MERMAID_PAGES: [&str; 2] = ["concepts/model", "concepts/operations"];
/// The diagrams render in the browser through `@docusaurus/theme-mermaid`: each page in `pages`
/// holds a fenced `mermaid` block, and `config` (`website/docusaurus.config.ts`) turns Mermaid on
/// and loads the theme. Without either, a diagram is published as its source text. A test holds
/// the committed files to it; `check-docs` reads no Docusaurus configuration.
#[cfg(test)]
fn mermaid_failures(pages: &[(&str, &str)], config: &str) -> Vec<String> {
    let mut failures = Vec::new();
    for (page, source) in pages {
        let fenced = Parser::new(source).any(|event| {
            matches!(
                event,
                Event::Start(Tag::CodeBlock(pulldown_cmark::CodeBlockKind::Fenced(language)))
                    if &*language == "mermaid"
            )
        });
        if !fenced {
            failures.push(format!(
                "website/docs/{page}.md: no fenced `mermaid` diagram"
            ));
        }
    }
    let lines: Vec<&str> = config.lines().map(str::trim).collect();
    if !(lines.contains(&"mermaid: true,")
        && lines.contains(&"themes: ['@docusaurus/theme-mermaid'],"))
    {
        failures.push(
            "website/docusaurus.config.ts: Mermaid is not on with `mermaid: true` and `themes: ['@docusaurus/theme-mermaid']`"
                .into(),
        );
    }
    failures
}
fn valid_commit(commit: &str) -> Result<()> {
    ensure!(
        commit.len() == 40
            && commit
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            && commit != "0".repeat(40),
        "commit must be a nonzero full Git revision, 40 lowercase hex digits"
    );
    Ok(())
}
/// After the build: check the built pages and the click ratchet, then write
/// `.well-known/b10x-site.json`, `.well-known/b10x-routes.json` and `.nojekyll`.
pub fn provenance(args: &ProvenanceArgs) -> Result<ExitCode> {
    valid_commit(&args.commit)?;
    let root = crate::repo::root()?;
    let built = built_site(&args.site)?;
    let pages = &built.pages;
    let mut failures = check_built(&built);
    failures.extend(absolute_paths(&args.site, &root)?);
    failures.extend(crate::site_ux::check_site(&root, pages)?);
    if !failures.is_empty() {
        for failure in &failures {
            eprintln!("{failure}");
        }
        bail!(
            "the built site at {} fails {} checks",
            args.site.display(),
            failures.len()
        );
    }
    fs::create_dir_all(args.site.join(".well-known"))?;
    fs::write(
        args.site.join(".well-known/b10x-site.json"),
        serde_json::to_vec_pretty(
            &json!({"schema":"b10x-project-site/v1","repository":"substrate","commit":args.commit,"baseUrl":BASE}),
        )?,
    )?;
    fs::write(
        args.site.join(".well-known/b10x-routes.json"),
        inventory(pages, &args.commit)?,
    )?;
    fs::write(args.site.join(".nojekyll"), "")?;
    println!(
        "Substrate documentation at {}: {} routes checked and bound to {}",
        args.site.display(),
        pages.len(),
        args.commit
    );
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A built docs page as Docusaurus writes it, reduced to what the checks read.
    fn built(current: &str, table: &str) -> String {
        let groups: String = GROUPS
            .iter()
            .map(|group| {
                [
                    "<a class=menu__link role=button href=/substrate/docs><span class=label>",
                    group,
                    "</span></a>",
                ]
                .concat()
            })
            .collect();
        format!(
            "<nav><a aria-current=page class=navbar__link href={current}>Top</a><button aria-label=\"Toggle navigation bar\" class=\"navbar__toggle clean-btn\" type=button></button></nav><aside class=theme-doc-sidebar-container>{groups}<a class=\"menu__link menu__link--active\" aria-current=page tabindex=0 href={current}>Here</a><a class=menu__link href=/substrate/docs/other>Other</a></aside><article>{table}</article>"
        )
    }

    #[test]
    fn links_cannot_escape_the_public_allowlist() {
        assert!(link("index", "../../AGENTS.md").is_err());
        assert!(link("index", "../../.engineering/planning/story/private.md").is_err());
        assert_eq!(
            link("concepts/model", "../getting-started.md#prerequisites").unwrap(),
            "/substrate/docs/getting-started/#prerequisites"
        );
    }
    #[cfg(unix)]
    #[test]
    fn public_inputs_cannot_follow_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("private.md"), "private").unwrap();
        std::os::unix::fs::symlink("private.md", dir.path().join("public.md")).unwrap();
        assert!(read_public(dir.path(), "public.md").is_err());
    }
    #[test]
    fn public_markdown_refuses_executable_html() {
        assert!(markdown("index", "---\ntitle: Test\n---\n<script>alert(1)</script>").is_err());
    }
    /// The generated changelog section of the Release notes page is held to every guard an
    /// authored page is, and one page never repeats a heading ID, across both parts.
    #[test]
    fn the_generated_release_section_and_repeated_headings_are_guarded() {
        let page = |generated: &str| {
            format!(
                "---\ntitle: R\n---\n# Release notes\n\n## Newest\n\n{RELEASES_MARKER}\n\n{generated}"
            )
        };
        markdown(
            RELEASES,
            &page("## 1.0.0 (2026-01-01)\n\n- [a](https://x.invalid/a)\n"),
        )
        .unwrap();
        for refused in [
            "- <img src=x onerror=alert(1)>\n",
            "<div>block</div>\n",
            "- ![pixel](https://x.invalid/p.png)\n",
            "- [up](../AGENTS.md)\n",
            "## Newest\n",
        ] {
            assert!(markdown(RELEASES, &page(refused)).is_err(), "{refused}");
        }
        let generated = "## 1.0.0 (2026-01-01)\n\n".to_owned() + &"word ".repeat(500);
        assert_eq!(
            markdown(RELEASES, &page(&generated)).unwrap().blocks,
            Vec::<usize>::new(),
            "the generated section is not measured"
        );
        assert!(markdown("status", "---\ntitle: S\n---\n# S\n\n## A b\n\n## A  B\n").is_err());
        markdown("status", "---\ntitle: S\n---\n# S\n\n## A\n\n## B\n").unwrap();
    }
    /// The built site is the declared routes and nothing else, whatever form the extra page takes.
    #[test]
    fn a_built_route_outside_the_declared_set_is_refused_in_every_form() {
        let site = |pages: &[&str], copies: &[&str], files: &[&str]| BuiltSite {
            pages: pages
                .iter()
                .map(|r| ((*r).to_owned(), String::new()))
                .collect(),
            copies: copies.iter().map(|r| (*r).to_owned()).collect(),
            files: files.iter().map(|f| (*f).to_owned()).collect(),
        };
        let declared: Vec<String> = declared_routes().into_iter().collect();
        let declared: Vec<&str> = declared.iter().map(String::as_str).collect();
        assert_eq!(declared.len(), PAGES.len() + 1);
        let files = ["index.html", "assets/js/main.js", "img/mark.svg"];
        assert!(only_declared_routes(&site(&declared, &declared, &files)).is_empty());
        let extra = [declared.clone(), vec!["/substrate/internal/plan/"]].concat();
        assert_eq!(only_declared_routes(&site(&extra, &[], &files)).len(), 1);
        assert_eq!(
            only_declared_routes(&site(&declared, &["/substrate/internal/plan/"], &files)).len(),
            1
        );
        assert_eq!(
            only_declared_routes(&site(&declared, &[], &["internal/plan.htm"])).len(),
            1
        );
    }
    /// Every docs page keeps `#main` and an H1 with its ID.
    #[test]
    fn a_docs_page_keeps_main_and_its_h1_id() {
        let route = "/substrate/docs/";
        let page = "<div id=main><h1 id=t>T</h1><h2 id=a>A</h2></div>";
        assert!(keeps_page_anchors(route, page).is_empty());
        assert!(
            keeps_page_anchors(route, "<div id=main><h1 class=x id=\"t\">T</h1></div>").is_empty()
        );
        for broken in [
            page.replace("<div id=main>", "<div>"),
            page.replace("<h1 id=t>", "<h1>"),
            page.replace("<h1 id=t>", "<h1 class=title>"),
            page.replace("<h1 id=t>T</h1>", ""),
        ] {
            assert_eq!(keeps_page_anchors(route, &broken).len(), 1, "{broken}");
        }
    }
    #[test]
    fn the_committed_site_inputs_hold() {
        let root = crate::repo::root().unwrap();
        let report = check(&root).unwrap();
        assert!(report.failures().is_empty(), "{:#?}", report.failures());
    }
    #[test]
    fn the_sidebar_is_read_in_source_order_under_its_groups() {
        let source = "    {\n      label: 'Start',\n      items: [page('index', 'Overview'), page('getting-started', 'Get started')],\n    },\n    {\n      label: 'Project',\n      items: [page('status', 'Status')],\n";
        assert_eq!(
            sidebar(source),
            [
                ("index".into(), "Overview".into(), "Start".into()),
                (
                    "getting-started".into(),
                    "Get started".into(),
                    "Start".into()
                ),
                ("status".into(), "Status".into(), "Project".into()),
            ]
        );
    }
    #[test]
    fn built_files_become_trailing_slash_routes_and_redirect_copies_are_skipped() {
        assert_eq!(built_route("index.html").as_deref(), Some("/substrate/"));
        assert_eq!(
            built_route("docs.html").as_deref(),
            Some("/substrate/docs/")
        );
        assert_eq!(
            built_route("docs/guides/cli.html").as_deref(),
            Some("/substrate/docs/guides/cli/")
        );
        assert_eq!(built_route("404.html"), None);
        assert_eq!(built_route("assets/a.js"), None);
        assert!(is_redirect("<!-- b10x-trailing-slash-copy --><html>"));
        assert!(!is_redirect("<html><body>page</body></html>"));
    }
    #[test]
    fn attributes_are_read_quoted_and_unquoted() {
        let html = r##"<h2 id="the-guard">x</h2><main id=main class=a><a href="#top">"##;
        assert_eq!(
            attribute_values(html, "id").into_iter().collect::<Vec<_>>(),
            ["main", "the-guard"]
        );
        assert_eq!(attribute_list(html, "href"), ["#top"]);
    }
    #[test]
    fn provenance_refuses_a_bad_commit_and_binds_the_routes_to_a_good_one() {
        assert!(valid_commit(&"0".repeat(40)).is_err());
        assert!(valid_commit(&"A".repeat(40)).is_err());
        assert!(valid_commit("abc").is_err());
        let commit = "b".repeat(40);
        valid_commit(&commit).unwrap();
        let pages: BTreeMap<String, String> = [
            ("/substrate/".to_owned(), "<main id=main></main>".to_owned()),
            (
                "/substrate/docs/".to_owned(),
                "<h2 id=\"b\"></h2><h2 id=a></h2>".to_owned(),
            ),
        ]
        .into();
        let inventory: serde_json::Value =
            serde_json::from_slice(&inventory(&pages, &commit).unwrap()).unwrap();
        assert_eq!(inventory["schema"], "b10x-project-routes/v1");
        assert_eq!(inventory["commit"], commit.as_str());
        assert_eq!(inventory["baseUrl"], BASE);
        assert_eq!(inventory["routes"][1]["path"], "/substrate/docs/");
        assert_eq!(inventory["routes"][1]["anchors"], json!(["a", "b"]));
    }
    #[test]
    fn a_built_site_without_a_start_page_or_with_two_answers_for_a_route_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("docs.html"), "<h1 id=a></h1>").unwrap();
        assert!(built_pages(dir.path()).is_err());
        fs::write(dir.path().join("index.html"), "<main id=main></main>").unwrap();
        fs::create_dir(dir.path().join("docs")).unwrap();
        fs::write(
            dir.path().join("docs/index.html"),
            "<!-- b10x-trailing-slash-copy -->",
        )
        .unwrap();
        let pages = built_pages(dir.path()).unwrap();
        assert_eq!(
            pages.keys().collect::<Vec<_>>(),
            ["/substrate/", "/substrate/docs/"]
        );
        fs::write(dir.path().join("docs/index.html"), "<h1 id=b></h1>").unwrap();
        assert!(built_pages(dir.path()).is_err());
    }
    #[test]
    fn start_page_links_must_reach_a_built_page_and_its_anchor() {
        let pages = |start: &str| -> BTreeMap<String, String> {
            [
                (BASE.to_owned(), start.to_owned()),
                ("/substrate/docs/x/".to_owned(), "<h2 id=a></h2>".to_owned()),
            ]
            .into()
        };
        let good = "<link href=\"/substrate/styles.css\"><a href=\"/substrate/docs/x#a\">x</a><a href=\"/substrate/\">s</a><a href=\"https://example.com/\">e</a>";
        assert!(start_page_links_resolve(&pages(good)).is_empty());
        assert_eq!(
            start_page_links_resolve(&pages("<a href=\"/substrate/docs/x#b\">x</a>")).len(),
            1
        );
        assert_eq!(
            start_page_links_resolve(&pages("<a href=\"/substrate/docs/y\">y</a>")).len(),
            1
        );
    }
    /// UXF-001: the page list names its groups and marks the page the reader is on.
    #[test]
    fn every_page_marks_itself_current_in_the_grouped_page_list() {
        let here = "/substrate/docs/guides/cli";
        assert!(marks_current_page("/substrate/docs/guides/cli/", &built(here, "")).is_empty());
        let elsewhere = built(here, "").replace(
            "aria-current=page tabindex=0 href=/substrate/docs/guides/cli",
            "aria-current=page tabindex=0 href=/substrate/docs/other",
        );
        assert!(!marks_current_page("/substrate/docs/guides/cli/", &elsewhere).is_empty());
        let unmarked = built(here, "").replace("aria-current=page ", "");
        assert!(!marks_current_page("/substrate/docs/guides/cli/", &unmarked).is_empty());
        let ungrouped = built(here, "").replace(">Concepts</span>", ">x</span>");
        assert_eq!(
            marks_current_page("/substrate/docs/guides/cli/", &ungrouped).len(),
            1
        );
    }
    /// UXF-002: on a narrow screen the page list is a closed drawer, not links above the article.
    #[test]
    fn the_narrow_screen_page_list_is_a_closed_disclosure() {
        let route = "/substrate/docs/";
        assert!(page_list_is_closed_on_narrow_screens(route, &built(route, "")).is_empty());
        let open = built(route, "").replace("<nav>", "<nav class=navbar-sidebar--show>");
        assert_eq!(page_list_is_closed_on_narrow_screens(route, &open).len(), 1);
        let flat = built(route, "").replace("theme-doc-sidebar-container", "list");
        assert_eq!(page_list_is_closed_on_narrow_screens(route, &flat).len(), 1);
    }
    /// UXF-006: every page-list link is a target at least 24 px high (WCAG 2.5.8, axe
    /// `target-size`); the site stylesheet states it and the build loads that stylesheet.
    #[test]
    fn page_list_links_are_targets_of_at_least_24_pixels() {
        let root = crate::repo::root().unwrap();
        let css = read_public(&root, "website/src/css/custom.css").unwrap();
        assert!(
            css.contains(".menu__link {\n  min-height: 24px;"),
            "target size"
        );
        let config = read_public(&root, "website/docusaurus.config.ts").unwrap();
        assert!(config.contains("customCss: './src/css/custom.css'"));
    }
    /// UXF-004: every table scrolls inside a focusable region named by the heading above it, in
    /// the server-rendered HTML, before any script runs.
    #[test]
    fn every_table_scrolls_inside_a_focusable_named_region() {
        let route = "/substrate/docs/concepts/adapters/";
        let heading = "<h3 class=anchor id=families-by-adapter>Families by adapter<a href=#families-by-adapter class=hash-link aria-label=\"Direct link to Families by adapter\">\u{200b}</a></h3>";
        let table = "<table><tr><td>x</td></tr></table></div>";
        let framed = |wrap: &str| built(route, &format!("{heading}{wrap}{table}"));
        let labelled = "<div class=b10x-table-wrap role=region tabindex=0 aria-label=\"Families by adapter table\">";
        assert!(
            tables_scroll_in_a_frame(route, &framed(labelled)).is_empty(),
            "{:?}",
            tables_scroll_in_a_frame(route, &framed(labelled))
        );
        let quoted = "<div class=\"b10x-table-wrap\" role=\"region\" tabindex=\"0\" aria-label=\"Families by adapter table\" aria-describedby=\"x\">";
        assert!(tables_scroll_in_a_frame(route, &framed(quoted)).is_empty());
        let entities = built(
            route,
            &format!(
                "<h2 id=a>Read <code>a</code> &amp; write</h2><div class=b10x-table-wrap role=region tabindex=0 aria-label=\"Read a &amp; write table\">{table}"
            ),
        );
        assert!(tables_scroll_in_a_frame(route, &entities).is_empty());
        let unheaded = built(
            route,
            &format!("<div class=b10x-table-wrap role=region tabindex=0 aria-label=Table>{table}"),
        );
        assert!(tables_scroll_in_a_frame(route, &unheaded).is_empty());
        for wrong in [
            // The docs-system frame before hydration: no region, no tab stop, no name.
            "<div class=b10x-table-wrap>",
            "<div class=b10x-table-wrap tabindex=0 aria-label=\"Families by adapter table\">",
            "<div class=b10x-table-wrap role=region aria-label=\"Families by adapter table\">",
            "<div class=b10x-table-wrap role=region tabindex=0>",
            "<div class=b10x-table-wrap role=region tabindex=0 aria-label=Table>",
            "<div class=b10x-table-wrap role=region tabindex=0 aria-label=\"Commands table\">",
            "<div class=other role=region tabindex=0 aria-label=\"Families by adapter table\">",
            "",
        ] {
            assert_eq!(
                tables_scroll_in_a_frame(route, &framed(wrong)).len(),
                1,
                "{wrong}"
            );
        }
    }
    /// The route inventory publishes the authored heading IDs and the page anchors, and none of the
    /// IDs React or Docusaurus generate for their own use, which change from build to build.
    #[test]
    fn the_inventory_publishes_authored_anchors_and_no_framework_ids() {
        let html = "<div id=__docusaurus><a id=__docusaurus_skipToContent_fallback></a><svg><symbol id=theme-svg-external-link></symbol></svg><div id=main><h1 id=run-one-confined-command>R</h1><h2 id=pod>P</h2><h2 id=install>I</h2><p id=b10x-table-_R_5q7dalbah_-instructions></p><p id=b10x-table-«r1»-instructions></p><p id=b10x-table-:r2:-instructions></p><p id=\":r3:\"></p></div></div>";
        assert_eq!(
            published_anchors(html).into_iter().collect::<Vec<_>>(),
            ["install", "main", "pod", "run-one-confined-command"]
        );
    }
    /// A built file that names the builder's home or checkout directory publishes it.
    #[test]
    fn a_built_file_carrying_a_home_or_checkout_path_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let site = dir.path().join("build");
        fs::create_dir_all(site.join("assets/js")).unwrap();
        fs::write(site.join("index.html"), "<main id=main></main>").unwrap();
        fs::write(
            site.join("assets/js/main.js"),
            "staticDirectories:[\"static\",\"node_modules/x/static\"]",
        )
        .unwrap();
        let checkout = Path::new("/srv/build/checkout");
        assert!(absolute_paths(&site, checkout).unwrap().is_empty());
        for leaked in [
            format!(
                "staticDirectories:[\"{}someone/x/website/node_modules/x/static\"]",
                HOME_PREFIXES[0]
            ),
            format!(
                "staticDirectories:[\"{}someone/x/static\"]",
                HOME_PREFIXES[1]
            ),
            "staticDirectories:[\"/srv/build/checkout/website/static\"]".to_owned(),
        ] {
            fs::write(site.join("assets/js/main.js"), &leaked).unwrap();
            assert_eq!(
                absolute_paths(&site, checkout).unwrap().len(),
                1,
                "{leaked}"
            );
        }
    }
    /// The quickstart's setup link leads to the daemon walkthrough its code sample needs: the
    /// private socket is started there, not in Getting started.
    #[test]
    fn the_start_page_sends_its_setup_link_to_the_daemon_walkthrough() {
        let root = crate::repo::root().unwrap();
        let html = read_public(&root, START_PAGE).unwrap();
        assert!(html.contains(
            "<a href=\"/substrate/docs/guides/run-a-command#start-with-an-observable-machine\">Complete setup and workspace round trip →</a>"
        ));
    }
    /// The two diagrams stay `mermaid` fences and the site keeps the Mermaid theme that renders them.
    #[test]
    fn the_diagrams_stay_mermaid_fences_and_the_theme_stays_on() {
        fn sources<'a>(pages: &'a [(&'a str, String)]) -> Vec<(&'a str, &'a str)> {
            pages.iter().map(|(p, s)| (*p, s.as_str())).collect()
        }
        let root = crate::repo::root().unwrap();
        let config = read_public(&root, "website/docusaurus.config.ts").unwrap();
        let pages: Vec<(&str, String)> = MERMAID_PAGES
            .iter()
            .map(|page| {
                (
                    *page,
                    read_public(&root, &format!("website/docs/{page}.md")).unwrap(),
                )
            })
            .collect();
        assert_eq!(MERMAID_PAGES, ["concepts/model", "concepts/operations"]);
        assert!(
            mermaid_failures(&sources(&pages), &config).is_empty(),
            "{:?}",
            mermaid_failures(&sources(&pages), &config)
        );
        let mut text = pages.clone();
        text[0].1 = text[0].1.replace("```mermaid", "```text");
        assert_eq!(mermaid_failures(&sources(&text), &config).len(), 1);
        for off in [
            config.replace("themes: ['@docusaurus/theme-mermaid'],", ""),
            config.replace("mermaid: true,", "mermaid: false,"),
        ] {
            assert_eq!(mermaid_failures(&sources(&pages), &off).len(), 1);
        }
    }
    #[test]
    fn the_releases_page_ends_with_the_newest_released_changelog_section() {
        let changelog = "# Changelog\n\n## [Unreleased]\n\n## [1.2.0] — 2026-01-02\n\nText.\n\n### Added\n\n- [a file](docs/a.md) and [b](https://example.com/b) and [c](adr/c.md#x).\n\n## [1.1.0] — 2026-01-01\n\n### Added\n\n- older\n";
        let section = newest_release(changelog).unwrap();
        assert!(section.starts_with("## 1.2.0 (2026-01-02)\n"), "{section}");
        assert!(section.contains("### Added in 1.2.0\n"), "{section}");
        assert!(!section.contains("older"), "{section}");
        assert!(
            section.contains(&format!("[a file]({SOURCE_BLOB}docs/a.md)")),
            "{section}"
        );
        assert!(section.contains("[b](https://example.com/b)"), "{section}");
        assert!(
            section.contains(&format!("[c]({SOURCE_BLOB}adr/c.md#x)")),
            "{section}"
        );
        assert!(newest_release("## [1.0.0] — 2026-01-01\n\n- [up](../x.md)\n").is_err());
        assert!(newest_release("## [1.0.0] — 2026-01-01\n\n- [r][x]\n\n[x]: docs/a.md\n").is_err());
        assert!(newest_release("# Changelog\n\n## [Unreleased]\n").is_err());
    }
    #[test]
    fn text_blocks_and_tables_are_measured_under_their_heading() {
        let rendered = render(
            "index",
            "## Two words\n\none two three\n\n- four five\n\n| A | B |\n|---|---|\n| `x` | y z |\n",
            &mut BTreeSet::new(),
        )
        .unwrap();
        assert_eq!(rendered.blocks, [3, 2, 1, 1, 1, 2]);
        assert_eq!(
            rendered.tables,
            [(
                "two-words".to_owned(),
                vec![
                    vec!["A".to_owned(), "B".to_owned()],
                    vec!["x".to_owned(), "y z".to_owned()]
                ]
            )]
        );
    }
}
