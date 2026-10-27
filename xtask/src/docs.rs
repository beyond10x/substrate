//! Standalone public site: explicit public inputs, static HTML, no shared-site dependency.
use anyhow::{Context, Result, ensure};
use clap::Args;
use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd, html};
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
    ("reference/contracts", "Contracts overview", "Reference"),
    ("reference/contract", "Wire contract", "Reference"),
    ("releases", "Release notes", "Reference"),
    ("status", "Status", "Project"),
    ("security", "Security", "Project"),
];
/// The navigation groups, in the order the sidebar shows them.
const GROUPS: [&str; 5] = ["Start", "Concepts", "Guides", "Reference", "Project"];
/// The page that renders the newest released section of `CHANGELOG.md` after its own text.
const RELEASES: &str = "releases";
/// Where a repository-relative link inside the changelog points once it is published.
const SOURCE_BLOB: &str = "https://github.com/beyond10x/substrate/blob/main/";

#[derive(Debug, Args)]
pub struct BuildArgs {
    #[arg(long)]
    out: PathBuf,
    #[arg(long)]
    commit: String,
}

/// One rendered Markdown source: its HTML, the word count of each text block (a paragraph, a list
/// item's own text, a table cell) and its tables, each under the ID of the heading above it.
#[derive(Debug, Default)]
pub(crate) struct Rendered {
    pub html: String,
    pub blocks: Vec<usize>,
    pub tables: Vec<(String, Vec<Vec<String>>)>,
}

/// How a link that is not `https://` or `#…` resolves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Links {
    /// Relative to the page, and it must name a published guide.
    Guides,
    /// Relative to the repository root, published as a link into the source on GitHub.
    Repository,
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
pub(crate) fn route(page: &str) -> String {
    format!(
        "/substrate/docs/{}/",
        if page == "index" { "" } else { page }
    )
    .replace("//", "/")
}
fn slug(text: &str) -> String {
    text.to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == ' ' || *c == '-')
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join("-")
}
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
            _ => anyhow::bail!("invalid public link"),
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
    render(page, body, Links::Guides)
}
#[allow(clippy::too_many_lines)] // One pass over the events, each arm one element kind, in source order.
fn render(page: &str, source: &str, links: Links) -> Result<Rendered> {
    let mut events = Parser::new_ext(
        source,
        Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH,
    )
    .peekable();
    let mut output = Vec::new();
    let mut ids = BTreeSet::new();
    let mut rendered = Rendered::default();
    // Open text blocks, innermost last, each with its word count so far.
    let mut open: Vec<usize> = Vec::new();
    let mut heading = (String::new(), String::new());
    let mut table: Option<Vec<Vec<String>>> = None;
    let mut cell: Option<String> = None;
    let words = |text: &str| text.split_whitespace().count();
    while let Some(event) = events.next() {
        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                let mut children = Vec::new();
                let mut text = String::new();
                for event in events.by_ref() {
                    if matches!(event, Event::End(TagEnd::Heading(_))) {
                        break;
                    }
                    if let Event::Text(t) | Event::Code(t) = &event {
                        text.push_str(t);
                    }
                    children.push(event);
                }
                let id = slug(&text);
                ensure!(ids.insert(id.clone()), "duplicate heading {id} in {page}");
                output.push(Event::Html(format!("<{level} id=\"{id}\">").into()));
                output.extend(children);
                output.push(Event::Html(format!("</{level}>").into()));
                heading = (id, text);
            }
            Event::Start(Tag::Link {
                link_type,
                dest_url,
                title,
                id,
            }) => output.push(Event::Start(Tag::Link {
                link_type,
                dest_url: match links {
                    Links::Guides => link(page, &dest_url)?,
                    Links::Repository => repository_link(&dest_url)?,
                }
                .into(),
                title,
                id,
            })),
            Event::Start(Tag::CodeBlock(kind)) => {
                let language = match kind {
                    pulldown_cmark::CodeBlockKind::Fenced(l) => l.into_string(),
                    pulldown_cmark::CodeBlockKind::Indented => "text".into(),
                };
                let mut code = String::new();
                for event in events.by_ref() {
                    if matches!(event, Event::End(TagEnd::CodeBlock)) {
                        break;
                    }
                    if let Event::Text(t) = event {
                        code.push_str(&t);
                    }
                }
                output.push(Event::Html(format!("<div class=\"code-caption\">{}</div><pre tabindex=\"0\" aria-label=\"{} example\"><code>{}</code></pre>", escape(&language), escape(&language), highlight(&code, &language)).into()));
            }
            Event::Html(_) | Event::InlineHtml(_) => {
                anyhow::bail!("raw HTML is not admitted in public Markdown: {page}")
            }
            Event::Start(Tag::Image { .. }) => anyhow::bail!("undeclared public image in {page}"),
            Event::Start(Tag::Table(alignments)) => {
                // A wide table scrolls inside its own region, which a keyboard can reach.
                let label = if heading.1.is_empty() {
                    "Table".to_owned()
                } else {
                    format!("{} table", heading.1)
                };
                output.push(Event::Html(
                    format!(
                        "<div class=\"table-scroll\" role=\"region\" tabindex=\"0\" aria-label=\"{}\">",
                        escape(&label)
                    )
                    .into(),
                ));
                output.push(Event::Start(Tag::Table(alignments)));
                table = Some(Vec::new());
            }
            Event::End(TagEnd::Table) => {
                output.push(Event::End(TagEnd::Table));
                output.push(Event::Html("</div>".into()));
                if let Some(rows) = table.take() {
                    rendered.tables.push((heading.0.clone(), rows));
                }
            }
            Event::Start(Tag::TableHead | Tag::TableRow) => {
                if let Some(rows) = table.as_mut() {
                    rows.push(Vec::new());
                }
                output.push(event);
            }
            Event::Start(tag @ (Tag::Paragraph | Tag::Item | Tag::TableCell)) => {
                if matches!(tag, Tag::TableCell) {
                    cell = Some(String::new());
                }
                open.push(0);
                output.push(Event::Start(tag));
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
                output.push(Event::End(end));
            }
            Event::Text(ref text) | Event::Code(ref text) => {
                if let Some(count) = open.last_mut() {
                    *count += words(text);
                }
                if let Some(cell) = cell.as_mut() {
                    cell.push_str(text);
                }
                output.push(event);
            }
            other => output.push(other),
        }
    }
    html::push_html(&mut rendered.html, output.into_iter());
    Ok(rendered)
}
// Preserve every source character. Highlight only quoted strings and whole-line comments;
// this deliberately makes no claim to parse the various shell/Rust/YAML grammars.
fn highlight(code: &str, language: &str) -> String {
    let mut result = String::new();
    for line in code.split_inclusive('\n') {
        if (matches!(language, "bash" | "yaml") && line.trim_start().starts_with('#'))
            || (language == "rust" && line.trim_start().starts_with("//"))
        {
            write!(
                result,
                "<span class=\"token-comment\">{}</span>",
                escape(line)
            )
            .expect("String write");
            continue;
        }
        let mut quote = None;
        let mut escaped = false;
        for c in line.chars() {
            if (c == '\'' || c == '"') && !escaped {
                if quote == Some(c) {
                    result.push_str(&escape(&c.to_string()));
                    result.push_str("</span>");
                    quote = None;
                    continue;
                }
                if quote.is_none() {
                    result.push_str("<span class=\"token-string\">");
                    quote = Some(c);
                }
            }
            result.push_str(&escape(&c.to_string()));
            escaped = c == '\\' && !escaped;
        }
        if quote.is_some() {
            result.push_str("</span>");
        }
    }
    result
}
fn shell(title: &str, path: &str, body: &str) -> String {
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\"><meta name=\"description\" content=\"Substrate: confined execution, durable operations, observed outcomes.\"><title>{} · Substrate</title><link rel=\"canonical\" href=\"https://beyond10x.github.io{path}\"><link rel=\"stylesheet\" href=\"/substrate/styles.css\"></head><body><a class=\"skip\" href=\"#main\">Skip to content</a><header><a class=\"brand\" href=\"/substrate/\">◈ Substrate</a><nav aria-label=\"Main\"><a href=\"/substrate/docs/getting-started/\">Get started</a><a href=\"/substrate/docs/concepts/adapters/\">Adapters</a><a href=\"/substrate/docs/reference/contracts/\">Contracts</a><a href=\"/substrate/docs/releases/\">Releases</a><a href=\"https://github.com/beyond10x/substrate\">Source ↗</a></nav></header><main id=\"main\" tabindex=\"-1\">{body}</main><footer><span>Substrate · Apache-2.0</span><a href=\"/substrate/docs/security/\">Security</a><a href=\"/substrate/docs/status/\">Status and limitations</a></footer></body></html>",
        escape(title)
    )
}
/// The grouped page list, with the current page marked for assistive technology and the eye.
fn navigation(current: &str) -> Result<String> {
    let mut html = String::new();
    for group in GROUPS {
        write!(html, "<p class=\"nav-group\">{group}</p><ul>")?;
        for (page, title, _) in PAGES.iter().filter(|(_, _, g)| *g == group) {
            let marked = if *page == current {
                " aria-current=\"page\""
            } else {
                ""
            };
            write!(
                html,
                "<li><a href=\"{}\"{marked}>{title}</a></li>",
                route(page)
            )?;
        }
        html.push_str("</ul>");
    }
    Ok(html)
}
/// A docs page: the page list as a sidebar on wide screens and as a closed disclosure above the
/// article on narrow ones, so the article starts in the first screen.
fn docs_page(page: &str, title: &str, article: &str) -> Result<String> {
    let navigation = navigation(page)?;
    Ok(format!(
        "<div class=\"docs-layout\"><details class=\"page-list\"><summary>All pages <span>· {}</span></summary><nav aria-label=\"Documentation\">{navigation}</nav></details><aside><nav aria-label=\"Documentation\">{navigation}</nav></aside><article>{article}</article></div>",
        escape(title)
    ))
}
pub(crate) fn attributes<'a>(html: &'a str, name: &str) -> Vec<&'a str> {
    html.split(&format!(" {name}=\""))
        .skip(1)
        .filter_map(|s| s.split('"').next())
        .collect()
}
fn validate(pages: &BTreeMap<String, String>) -> Result<()> {
    for (path, page) in pages {
        let ids = attributes(page, "id");
        ensure!(
            ids.len() == ids.iter().collect::<BTreeSet<_>>().len(),
            "duplicate id at {path}"
        );
        ensure!(!page.contains("<script"), "script at {path}");
        for href in attributes(page, "href") {
            if href.starts_with("https://") || href == "/substrate/styles.css" {
                continue;
            }
            let (target, anchor) = href.split_once('#').unwrap_or((href, ""));
            let destination = pages
                .get(if target.is_empty() {
                    path.as_str()
                } else {
                    target
                })
                .with_context(|| format!("broken route {href} at {path}"))?;
            ensure!(
                anchor.is_empty() || attributes(destination, "id").contains(&anchor),
                "broken anchor {href} at {path}"
            );
        }
    }
    Ok(())
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
/// version, so the sections of one page never share a heading ID.
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
    Ok(section)
}
/// Every rendered Markdown page, keyed by its path below `website/docs/`.
pub(crate) fn rendered(root: &Path) -> Result<BTreeMap<&'static str, Rendered>> {
    let mut rendered = BTreeMap::new();
    for (page, _, _) in PAGES {
        let mut page_rendered = markdown(
            page,
            &read_public(root, &format!("website/docs/{page}.md"))?,
        )?;
        if *page == RELEASES {
            let release = render(
                page,
                &newest_release(&read_public(root, "CHANGELOG.md")?)?,
                Links::Repository,
            )?;
            // The changelog's text is not the site's to shorten (UXF-007), so its blocks stay out of
            // the page's reading load and a release never moves the UX baseline.
            page_rendered.html.push_str(&release.html);
        }
        rendered.insert(*page, page_rendered);
    }
    Ok(rendered)
}
fn pages(root: &Path) -> Result<BTreeMap<String, String>> {
    pages_from(root, &rendered(root)?)
}
pub(crate) fn pages_from(
    root: &Path,
    rendered: &BTreeMap<&'static str, Rendered>,
) -> Result<BTreeMap<String, String>> {
    let mut pages = BTreeMap::new();
    pages.insert(
        "/substrate/".into(),
        shell(
            "Confined execution. Observed outcomes.",
            "/substrate/",
            &read_public(root, "website/index.html")?,
        ),
    );
    for (page, title, _) in PAGES {
        let article = &rendered
            .get(page)
            .with_context(|| format!("{page} was not rendered"))?
            .html;
        pages.insert(
            route(page),
            shell(title, &route(page), &docs_page(page, title, article)?),
        );
    }
    validate(&pages)?;
    Ok(pages)
}
pub fn check(root: &Path) -> Result<crate::report::Report> {
    let rendered = rendered(root)?;
    let pages = pages_from(root, &rendered)?;
    read_public(root, "website/styles.css")?;
    let mut failures = crate::site_drift::check(root, &rendered)?;
    failures.extend(crate::site_ux::check(root, &rendered, &pages)?);
    if !failures.is_empty() {
        return Ok(crate::report::Report::failed(failures));
    }
    Ok(crate::report::Report::passed(format!(
        "Substrate documentation: {} pages, routes and anchors valid, derived tables match the specification, the UX ratchet holds",
        pages.len()
    )))
}
pub fn build(args: &BuildArgs) -> Result<ExitCode> {
    ensure!(
        args.commit.len() == 40
            && args.commit.bytes().all(|b| b.is_ascii_hexdigit())
            && args.commit != "0".repeat(40),
        "commit must be a nonzero full Git revision"
    );
    let root = crate::repo::root()?;
    let pages = pages(&root)?;
    ensure!(
        !args.out.exists() || fs::read_dir(&args.out)?.next().is_none(),
        "output must be empty to exclude stale/private files"
    );
    fs::create_dir_all(args.out.join(".well-known"))?;
    for (route, html) in &pages {
        let dir = args
            .out
            .join(route.strip_prefix("/substrate/").context("site base")?);
        fs::create_dir_all(&dir)?;
        fs::write(dir.join("index.html"), html)?;
    }
    fs::write(
        args.out.join("styles.css"),
        read_public(&root, "website/styles.css")?,
    )?;
    fs::write(args.out.join(".nojekyll"), "")?;
    fs::write(
        args.out.join(".well-known/b10x-site.json"),
        serde_json::to_vec_pretty(
            &json!({"schema":"b10x-project-site/v1","repository":"substrate","commit":args.commit,"baseUrl":"/substrate/"}),
        )?,
    )?;
    fs::write(
        args.out.join(".well-known/b10x-routes.json"),
        inventory(&pages, &args.commit)?,
    )?;
    println!("Substrate documentation built at {}", args.out.display());
    Ok(ExitCode::SUCCESS)
}
/// The published route and anchor inventory: every page with its sorted rendered element IDs,
/// stamped with the same commit as the site provenance. The organization Website reads it to
/// redirect the former `/docs/substrate/` pages and to check links into this site.
fn inventory(pages: &BTreeMap<String, String>, commit: &str) -> Result<Vec<u8>> {
    let routes: Vec<_> = pages
        .iter()
        .map(|(path, page)| {
            let anchors: BTreeSet<_> = attributes(page, "id").into_iter().collect();
            json!({"path": path, "anchors": anchors})
        })
        .collect();
    Ok(serde_json::to_vec_pretty(&json!({
        "schema": "b10x-project-routes/v1",
        "repository": "substrate",
        "commit": commit,
        "baseUrl": "/substrate/",
        "routes": routes,
    }))?)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn links_cannot_escape_the_public_allowlist() {
        assert!(link("index", "../../AGENTS.md").is_err());
        assert!(link("index", "../../.engineering/planning/story/private.md").is_err());
        assert_eq!(
            link("concepts/model", "../getting-started.md#prerequisites").unwrap(),
            "/substrate/docs/getting-started/#prerequisites"
        );
    }
    #[test]
    fn broken_anchors_are_not_silently_published() {
        let mut pages = BTreeMap::new();
        pages.insert(
            "/substrate/".into(),
            "<a href=\"#missing\">broken</a>".into(),
        );
        assert!(validate(&pages).is_err());
    }
    #[test]
    fn occupied_output_is_refused_without_changing_its_files() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("private.txt"), "retain").unwrap();
        let args = BuildArgs {
            out: dir.path().into(),
            commit: "a".repeat(40),
        };
        assert!(
            build(&args)
                .unwrap_err()
                .to_string()
                .contains("output must be empty")
        );
        assert_eq!(
            fs::read_to_string(dir.path().join("private.txt")).unwrap(),
            "retain"
        );
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
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
    fn the_build_publishes_every_route_with_its_anchors_at_the_built_commit() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("site");
        let commit = "b".repeat(40);
        build(&BuildArgs {
            out: out.clone(),
            commit: commit.clone(),
        })
        .unwrap();
        let read = |path: &str| -> serde_json::Value {
            serde_json::from_slice(&fs::read(out.join(path)).unwrap()).unwrap()
        };
        let site = read(".well-known/b10x-site.json");
        let inventory = read(".well-known/b10x-routes.json");
        assert_eq!(inventory["schema"], "b10x-project-routes/v1");
        for key in ["repository", "commit", "baseUrl"] {
            assert_eq!(inventory[key], site[key], "{key}");
        }
        assert_eq!(inventory["commit"], commit.as_str());
        let routes = inventory["routes"].as_array().unwrap();
        let paths: Vec<_> = routes.iter().map(|r| r["path"].as_str().unwrap()).collect();
        let mut expected: Vec<_> = PAGES.iter().map(|(page, _, _)| route(page)).collect();
        expected.push("/substrate/".into());
        expected.sort();
        assert_eq!(paths, expected);
        let pages = pages(&crate::repo::root().unwrap()).unwrap();
        for entry in routes {
            let path = entry["path"].as_str().unwrap();
            let html = fs::read_to_string(
                out.join(path.strip_prefix("/substrate/").unwrap())
                    .join("index.html"),
            )
            .unwrap();
            assert_eq!(html, pages[path]);
            let anchors: Vec<_> = entry["anchors"]
                .as_array()
                .unwrap()
                .iter()
                .map(|a| a.as_str().unwrap())
                .collect();
            let mut rendered = attributes(&html, "id");
            rendered.sort_unstable();
            assert_eq!(anchors, rendered, "{path}");
            assert!(anchors.contains(&"main"), "{path}");
        }
    }
    #[test]
    fn public_markdown_refuses_executable_html() {
        assert!(markdown("index", "---\ntitle: Test\n---\n<script>alert(1)</script>").is_err());
    }
    /// UXF-001: the page list names its groups and marks the page the reader is on.
    #[test]
    fn every_page_marks_itself_current_in_the_grouped_page_list() {
        let pages = pages(&crate::repo::root().unwrap()).unwrap();
        for (page, title, _) in PAGES {
            let html = &pages[&route(page)];
            let current = format!(
                "<a href=\"{}\" aria-current=\"page\">{title}</a>",
                route(page)
            );
            // Once in the sidebar, once in the narrow-screen disclosure, and nowhere else.
            assert_eq!(html.matches("aria-current=").count(), 2, "{page}");
            assert_eq!(html.matches(&current).count(), 2, "{page}");
            for group in GROUPS {
                assert!(
                    html.contains(&format!("<p class=\"nav-group\">{group}</p>")),
                    "{page}"
                );
            }
        }
    }
    /// UXF-002: on a narrow screen the page list is a closed disclosure, not 20 links above the
    /// article; the stylesheet hides the sidebar there and the disclosure on wide screens.
    #[test]
    fn the_narrow_screen_page_list_is_a_closed_disclosure() {
        let root = crate::repo::root().unwrap();
        let pages = pages(&root).unwrap();
        for (page, _, _) in PAGES {
            let html = &pages[&route(page)];
            assert!(
                html.contains("<details class=\"page-list\"><summary>"),
                "{page}"
            );
            assert!(
                !html.contains("<details class=\"page-list\" open"),
                "{page}"
            );
        }
        let css = read_public(&root, "website/styles.css").unwrap();
        assert!(
            css.contains(".page-list{display:none"),
            "wide screens hide the disclosure"
        );
        assert!(
            css.contains(".docs-layout aside{display:none"),
            "narrow screens hide the sidebar"
        );
    }
    /// UXF-006: every page-list link is a target at least 24 px high (WCAG 2.5.8, axe
    /// `target-size`), and a sidebar taller than the viewport scrolls instead of being cut off.
    #[test]
    fn page_list_links_are_targets_of_at_least_24_pixels() {
        let css = read_public(&crate::repo::root().unwrap(), "website/styles.css").unwrap();
        assert!(
            css.contains(".docs-layout nav a{display:block;min-height:24px"),
            "target size"
        );
        assert!(
            css.contains("max-height:calc(100vh - 3rem);overflow-y:auto"),
            "a tall sidebar scrolls"
        );
    }
    /// UXF-004: a table that scrolls sideways is a named region a keyboard can focus.
    #[test]
    fn every_table_scrolls_inside_a_focusable_named_region() {
        let pages = pages(&crate::repo::root().unwrap()).unwrap();
        let mut tables = 0;
        for (path, html) in &pages {
            for (index, _) in html.match_indices("<table>") {
                tables += 1;
                let before = &html[..index];
                let region = before
                    .rfind("<div class=\"table-scroll\"")
                    .unwrap_or_else(|| panic!("a table outside a scroll region at {path}"));
                let opening = &before[region..];
                assert!(
                    opening.contains("role=\"region\" tabindex=\"0\" aria-label=\""),
                    "{path}"
                );
                assert!(!opening.contains("</div>"), "{path}");
            }
        }
        assert!(tables > 0);
    }
    #[test]
    fn the_releases_page_renders_the_newest_released_changelog_section() {
        let changelog = "# Changelog\n\n## [Unreleased]\n\n## [1.2.0] — 2026-01-02\n\nText.\n\n### Added\n\n- [a file](docs/a.md) and [b](https://example.com/b).\n\n## [1.1.0] — 2026-01-01\n\n### Added\n\n- older\n";
        let section = newest_release(changelog).unwrap();
        assert!(section.starts_with("## 1.2.0 (2026-01-02)\n"), "{section}");
        assert!(section.contains("### Added in 1.2.0\n"), "{section}");
        assert!(!section.contains("older"), "{section}");
        let html = render(RELEASES, &section, Links::Repository).unwrap().html;
        assert!(
            html.contains(&format!("href=\"{SOURCE_BLOB}docs/a.md\"")),
            "{html}"
        );
        assert!(html.contains("href=\"https://example.com/b\""), "{html}");
        assert!(html.contains("id=\"added-in-120\""), "{html}");
        assert!(render(RELEASES, "[up](../x.md)", Links::Repository).is_err());
        assert!(newest_release("# Changelog\n\n## [Unreleased]\n").is_err());
    }
    #[test]
    fn text_blocks_and_tables_are_measured_under_their_heading() {
        let rendered = render(
            "index",
            "## Two words\n\none two three\n\n- four five\n\n| A | B |\n|---|---|\n| `x` | y z |\n",
            Links::Guides,
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
