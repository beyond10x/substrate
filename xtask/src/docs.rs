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

const PAGES: &[(&str, &str)] = &[
    ("index", "Overview"),
    ("getting-started", "Get started"),
    ("concepts/boundary", "System boundary"),
    ("concepts/confinement", "Confinement"),
    ("concepts/operations", "Operations"),
    ("concepts/model", "System model"),
    ("guides/run-a-command", "Run a command"),
    ("guides/storage-and-metrics", "Storage and metrics"),
    ("guides/deployment", "Deployment"),
    ("guides/rust-sdk", "Rust SDK"),
    ("guides/mcp-adapter", "MCP adapter"),
    ("reference/contract", "Contract"),
    ("use-cases", "Use cases"),
    ("status", "Status"),
    ("security", "Security"),
];
#[derive(Debug, Args)]
pub struct BuildArgs {
    #[arg(long)]
    out: PathBuf,
    #[arg(long)]
    commit: String,
}
fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
fn route(page: &str) -> String {
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
        PAGES.iter().any(|(p, _)| *p == name),
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
fn markdown(page: &str, source: &str) -> Result<String> {
    let source = source
        .strip_prefix("---\n")
        .and_then(|s| s.split_once("\n---\n").map(|(_, body)| body))
        .context("missing document frontmatter")?;
    let mut events = Parser::new_ext(
        source,
        Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH,
    )
    .peekable();
    let mut output = Vec::new();
    let mut ids = BTreeSet::new();
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
            }
            Event::Start(Tag::Link {
                link_type,
                dest_url,
                title,
                id,
            }) => output.push(Event::Start(Tag::Link {
                link_type,
                dest_url: link(page, &dest_url)?.into(),
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
            other => output.push(other),
        }
    }
    let mut rendered = String::new();
    html::push_html(&mut rendered, output.into_iter());
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
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\"><meta name=\"description\" content=\"Substrate: confined execution, durable operations, observed outcomes.\"><title>{} · Substrate</title><link rel=\"canonical\" href=\"https://beyond10x.github.io{path}\"><link rel=\"stylesheet\" href=\"/substrate/styles.css\"></head><body><a class=\"skip\" href=\"#main\">Skip to content</a><header><a class=\"brand\" href=\"/substrate/\">◈ Substrate</a><nav aria-label=\"Main\"><a href=\"/substrate/docs/getting-started/\">Get started</a><a href=\"/substrate/#architecture\">Architecture</a><a href=\"/substrate/docs/reference/contract/\">Contract</a><a href=\"https://github.com/beyond10x/substrate\">Source ↗</a></nav></header><main id=\"main\" tabindex=\"-1\">{body}</main><footer><span>Substrate · Apache-2.0</span><a href=\"/substrate/docs/security/\">Security</a><a href=\"/substrate/docs/status/\">Status and limitations</a></footer></body></html>",
        escape(title)
    )
}
fn attributes<'a>(html: &'a str, name: &str) -> Vec<&'a str> {
    html.split(&format!("{name}=\""))
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
fn read_public(root: &Path, relative: &str) -> Result<String> {
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
fn pages(root: &Path) -> Result<BTreeMap<String, String>> {
    let mut pages = BTreeMap::new();
    pages.insert(
        "/substrate/".into(),
        shell(
            "Confined execution. Observed outcomes.",
            "/substrate/",
            &read_public(root, "website/index.html")?,
        ),
    );
    let mut navigation = String::new();
    for (p, title) in PAGES {
        write!(navigation, "<a href=\"{}\">{title}</a>", route(p))?;
    }
    for (page, title) in PAGES {
        let rendered = markdown(
            page,
            &read_public(root, &format!("website/docs/{page}.md"))?,
        )?;
        pages.insert(route(page), shell(title, &route(page), &format!("<div class=\"docs-layout\"><aside><nav aria-label=\"Guides\">{navigation}</nav></aside><article>{rendered}</article></div>")));
    }
    validate(&pages)?;
    Ok(pages)
}
pub fn check(root: &Path) -> Result<crate::report::Report> {
    let pages = pages(root)?;
    read_public(root, "website/styles.css")?;
    Ok(crate::report::Report::passed(format!(
        "Substrate documentation: {} pages, routes and anchors valid",
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
        let mut expected: Vec<_> = PAGES.iter().map(|(page, _)| route(page)).collect();
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
}
