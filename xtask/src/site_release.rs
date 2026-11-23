//! UXF-009: what the public pages and `STATUS.md` say is current, held to its source.
//!
//! A page names the current release, its publication date, the advertised wire bundle or the
//! newest bundle directory only in one of the sentence forms of `FORMS`. `check-docs` fills each
//! form from the repository and compares it with the page, so a release bump fails the gate with
//! the file, the line, what the line reads and what it must read. A sentence in another form is
//! history and is not read: "Release `0.16.1` was the first to advertise …" stays as written.
//!
//! The sources: `[workspace.package] version` in `Cargo.toml`, the date of that version's section
//! in `CHANGELOG.md`, `ADVERTISED_CONTRACT_BUNDLE` in the wire crate, the newest directory under
//! `contracts/substrate-wire/`, and `FACT_ORDER` in the host crate for the fact list.
use crate::docs::{PAGES, RELEASES_MARKER, read_public};
use anyhow::{Context, Result, ensure};
use std::path::Path;

const MANIFEST: &str = "Cargo.toml";
const CHANGELOG: &str = "CHANGELOG.md";
const WIRE: &str = "crates/substrate-wire/src/lib.rs";
const BUNDLES: &str = "contracts/substrate-wire";
const FACTS: &str = "crates/substrate-host/src/confinement/facts.rs";
const SOURCE: &str = "https://github.com/beyond10x/substrate/";
/// The page whose list of facts is `FACT_ORDER`, and the words that open the list.
const FACT_PAGE: &str = "guides/confinement-profiles";
/// The repository status file. It is no site page, and its statements of the latest release are
/// held like the site's (a review of release 0.24.0 found it naming 0.16.1 as latest).
const STATUS_FILE: &str = "STATUS.md";
const FACT_LIST: &str = "what the adapter proved, in this order: ";

/// Every sentence form that states what is current: the page that must hold it (or `STATUS.md`),
/// and its text on one line of that file's Markdown. `{release}` is the workspace version,
/// `{published}` its changelog date as `22 November 2026`, `{advertised}` the advertised bundle's
/// version and `{newest}` the newest bundle directory. A form is read in its own file.
const FORMS: &[(&str, &str)] = &[
    (
        STATUS_FILE,
        "Release [{release}](https://github.com/beyond10x/substrate/releases/tag/{release}), published on {published},",
    ),
    (
        STATUS_FILE,
        "Release `{release}` advertises `substrate-wire/{advertised}`",
    ),
    (STATUS_FILE, "The latest annotated tag is `{release}`"),
    (STATUS_FILE, "the latest release is `{release}`"),
    ("getting-started", "version={release}"),
    (
        "getting-started",
        "--tag {release} b10x-substrate-cli --locked",
    ),
    (
        "status",
        "Release [{release}](https://github.com/beyond10x/substrate/releases/tag/{release}), published on {published},",
    ),
    (
        "status",
        "signed `substrate-wire/{advertised}` development bundle.",
    ),
    (
        "status",
        "keyless-signed {release} daemon, MCP and executor images, chart and binaries, and the signed {advertised} development bundle",
    ),
    ("status", "Release {release} adds:"),
    (
        "status",
        "Release `{release}` advertises `substrate-wire/{advertised}`",
    ),
    (
        "releases",
        "The newest release is {release}, published on {published}.",
    ),
    ("reference/contracts", "newest `{newest}`,"),
    (
        "reference/contract",
        "Release `{release}` advertises `substrate-wire/{advertised}`",
    ),
    (
        "reference/contract",
        "[{advertised} bundle](https://github.com/beyond10x/substrate/tree/{release}/contracts/substrate-wire/{advertised})",
    ),
    (
        "guides/rust-sdk",
        "Release {release} verifies the explicitly promoted `substrate-wire/{advertised}` name",
    ),
    (
        "concepts/model",
        "the implementation in release `{release}`, which advertises the `substrate-wire/{advertised}` development contract",
    ),
];
/// The forms read on every page. The present tense of "advertises" belongs to the current
/// release, so the same sentence about an earlier release fails wherever it stands.
const EVERYWHERE: &[&str] = &["Release `{release}` advertises `substrate-wire/{advertised}`"];
/// The file a form's owner names: `STATUS.md` itself, or a site page's Markdown.
fn file(owner: &str) -> String {
    if owner == STATUS_FILE {
        owner.to_owned()
    } else {
        format!("website/docs/{owner}.md")
    }
}

/// What the repository says is current.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Current {
    release: String,
    published: String,
    advertised: String,
    newest: String,
}
impl Current {
    fn value(&self, name: &str) -> Option<&str> {
        Some(match name {
            "release" => self.release.as_str(),
            "published" => self.published.as_str(),
            "advertised" => self.advertised.as_str(),
            "newest" => self.newest.as_str(),
            _ => return None,
        })
    }
    /// Where a value comes from, for the failure line.
    fn sources(&self) -> String {
        format!(
            "{MANIFEST} has version {}, {CHANGELOG} dates it {}, {WIRE} advertises substrate-wire/{}, the newest directory of {BUNDLES} is {}",
            self.release, self.published, self.advertised, self.newest
        )
    }
}

/// One piece of a form: text that must stand as written, or a value.
#[derive(Debug, PartialEq, Eq)]
enum Piece<'a> {
    Text(&'a str),
    Value(&'a str),
}
fn pieces(form: &str) -> Vec<Piece<'_>> {
    let mut found = Vec::new();
    let mut rest = form;
    while let Some((text, tail)) = rest.split_once('{') {
        let Some((name, after)) = tail.split_once('}') else {
            break;
        };
        if !text.is_empty() {
            found.push(Piece::Text(text));
        }
        found.push(Piece::Value(name));
        rest = after;
    }
    if !rest.is_empty() {
        found.push(Piece::Text(rest));
    }
    found
}
/// `form` with the current values in it.
fn filled(form: &str, current: &Current) -> String {
    pieces(form)
        .iter()
        .map(|piece| match piece {
            Piece::Text(text) => *text,
            Piece::Value(name) => current.value(name).unwrap_or_default(),
        })
        .collect()
}
/// How many bytes at the start of `text` are a version, `1.2.3`.
fn version_len(text: &str) -> Option<usize> {
    let mut at = 0;
    for part in 0..3 {
        let digits = text[at..].bytes().take_while(u8::is_ascii_digit).count();
        if digits == 0 {
            return None;
        }
        at += digits;
        if part < 2 {
            if text.as_bytes().get(at) != Some(&b'.') {
                return None;
            }
            at += 1;
        }
    }
    Some(at)
}
const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];
/// How many bytes at the start of `text` are a date, `22 November 2026`.
fn date_len(text: &str) -> Option<usize> {
    let day = text.bytes().take_while(u8::is_ascii_digit).count();
    let rest = text.get(day..)?.strip_prefix(' ')?;
    let month = MONTHS.iter().find(|month| rest.starts_with(**month))?;
    let year = rest[month.len()..].strip_prefix(' ')?;
    let digits = year.bytes().take_while(u8::is_ascii_digit).count();
    ((1..=2).contains(&day) && digits == 4).then_some(day + 1 + month.len() + 1 + digits)
}
/// The part of `line` that has the shape of `form`, whatever values it carries.
fn shaped<'a>(line: &'a str, form: &str) -> Option<&'a str> {
    let pieces = pieces(form);
    let Some(Piece::Text(first)) = pieces.first() else {
        return None;
    };
    'starts: for (start, _) in line.match_indices(first) {
        let mut at = start + first.len();
        for piece in &pieces[1..] {
            let rest = &line[at..];
            let len = match piece {
                Piece::Text(text) => rest.starts_with(text).then_some(text.len()),
                Piece::Value("published") => date_len(rest),
                Piece::Value(_) => version_len(rest),
            };
            let Some(len) = len else {
                continue 'starts;
            };
            at += len;
        }
        return Some(&line[start..at]);
    }
    None
}
/// A GitHub link into this repository's source that is pinned to a version tag, as its version.
fn pinned_tags(line: &str) -> Vec<&str> {
    let mut found = Vec::new();
    for kind in ["tree/", "blob/", "releases/download/"] {
        let prefix = format!("{SOURCE}{kind}");
        for (at, _) in line.match_indices(&prefix) {
            let rest = &line[at + prefix.len()..];
            if let Some(len) = version_len(rest)
                && rest[len..].starts_with('/')
            {
                found.push(&rest[..len]);
            }
        }
    }
    found
}

/// Every statement of `pages` (a page and its authored Markdown) that names, as current, a value
/// other than `current`'s, and every form its page no longer holds.
fn stale_statements(current: &Current, pages: &[(&str, &str)]) -> Vec<String> {
    let mut failures = Vec::new();
    let mut held = vec![false; FORMS.len()];
    for (page, source) in pages {
        for (number, line) in source.lines().enumerate() {
            let at = format!("{}:{}", file(page), number + 1);
            for (index, (owner, form)) in FORMS.iter().enumerate() {
                let read = owner == page || EVERYWHERE.contains(form);
                let Some(found) = shaped(line, form).filter(|_| read) else {
                    continue;
                };
                let expected = filled(form, current);
                // A stale line on the form's own page is that form's line: it is reported here,
                // with its line, and not a second time as a form the page lost.
                held[index] |= owner == page;
                if found != expected {
                    failures.push(format!("{at}: reads \"{found}\", write \"{expected}\""));
                }
            }
            for tag in pinned_tags(line) {
                if tag != current.release {
                    failures.push(format!(
                        "{at}: a source link is pinned to tag {tag}, write {}",
                        current.release
                    ));
                }
            }
        }
    }
    for ((page, form), held) in FORMS.iter().zip(held) {
        if !held {
            failures.push(format!(
                "{}: no line reads \"{}\"",
                file(page),
                filled(form, current)
            ));
        }
    }
    // Two forms can share a line; one line is reported once per reading.
    failures.dedup();
    if !failures.is_empty() {
        failures.push(format!("current values: {}", current.sources()));
    }
    failures
}

/// The facts a page lists after the words of `FACT_LIST`, up to the sentence's full stop.
fn listed_facts(source: &str) -> Option<(usize, Vec<String>)> {
    let at = source.find(FACT_LIST)?;
    let line = source[..at].lines().count().max(1);
    let list = &source[at + FACT_LIST.len()..];
    let end = [list.find(".\n"), list.find(". ")]
        .into_iter()
        .flatten()
        .min()?;
    let list = &list[..end];
    Some((
        line,
        list.split(',')
            .map(|name| name.trim().trim_matches('`').to_owned())
            .collect(),
    ))
}
/// The page's fact list is `facts`, in order.
fn fact_list(facts: &[String], source: &str) -> Vec<String> {
    let file = file(FACT_PAGE);
    match listed_facts(source) {
        None => vec![format!("{file}: no fact list after \"{FACT_LIST}\"")],
        Some((_, listed)) if listed == facts => Vec::new(),
        Some((line, listed)) => {
            let missing: Vec<&String> = facts.iter().filter(|f| !listed.contains(f)).collect();
            let extra: Vec<&String> = listed.iter().filter(|f| !facts.contains(f)).collect();
            vec![format!(
                "{file}:{line}: the fact list has {} facts, FACT_ORDER in {FACTS} has {}: missing {missing:?}, not in FACT_ORDER {extra:?}; write {}",
                listed.len(),
                facts.len(),
                facts
                    .iter()
                    .map(|fact| format!("`{fact}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            )]
        }
    }
}

/// `[workspace.package] version` of the root manifest.
fn workspace_version(manifest: &str) -> Result<String> {
    manifest
        .lines()
        .skip_while(|line| line.trim() != "[workspace.package]")
        .skip(1)
        .take_while(|line| !line.starts_with('['))
        .find_map(|line| {
            let value = line.strip_prefix("version = \"")?.strip_suffix('"')?;
            (version_len(value) == Some(value.len())).then(|| value.to_owned())
        })
        .with_context(|| format!("{MANIFEST} has no [workspace.package] version"))
}
/// The date of `release`'s section of the changelog, `2026-11-22`, as `22 November 2026`.
fn published(changelog: &str, release: &str) -> Result<String> {
    let heading = format!("## [{release}] — ");
    let date = changelog
        .lines()
        .find_map(|line| line.strip_prefix(&heading))
        .with_context(|| format!("{CHANGELOG} has no released section for {release}"))?
        .trim();
    let parts: Vec<&str> = date.split('-').collect();
    let number = |part: &str| part.parse::<usize>().ok();
    let (Some(year), Some(month), Some(day)) = (
        parts.first().copied().and_then(number),
        parts.get(1).copied().and_then(number),
        parts.get(2).copied().and_then(number),
    ) else {
        anyhow::bail!("{CHANGELOG}: {release} is dated {date:?}, not YYYY-MM-DD");
    };
    ensure!(
        parts.len() == 3 && (1..=12).contains(&month) && (1..=31).contains(&day),
        "{CHANGELOG}: {release} is dated {date:?}, not YYYY-MM-DD"
    );
    Ok(format!("{day} {} {year}", MONTHS[month - 1]))
}
/// The version in `ADVERTISED_CONTRACT_BUNDLE`.
fn advertised(wire: &str) -> Result<String> {
    wire.lines()
        .find_map(|line| {
            line.strip_prefix("pub const ADVERTISED_CONTRACT_BUNDLE: &str = \"substrate-wire/")?
                .strip_suffix("\";")
        })
        .filter(|version| version_len(version) == Some(version.len()))
        .map(str::to_owned)
        .with_context(|| format!("{WIRE} has no ADVERTISED_CONTRACT_BUNDLE line"))
}
/// The newest bundle directory, by version.
fn newest_bundle(root: &Path) -> Result<String> {
    let mut versions = Vec::new();
    let listing = || format!("listing the bundle directories of {BUNDLES}");
    for entry in std::fs::read_dir(root.join(BUNDLES)).with_context(listing)? {
        let entry = entry.with_context(listing)?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let kind = entry.file_type().with_context(listing)?;
        if !kind.is_dir() || version_len(&name) != Some(name.len()) {
            continue;
        }
        let key: Vec<u64> = name.split('.').filter_map(|n| n.parse().ok()).collect();
        versions.push((key, name));
    }
    versions.sort();
    versions
        .pop()
        .map(|(_, name)| name)
        .with_context(|| format!("{BUNDLES} holds no bundle directory"))
}
/// `FACT_ORDER` as the pages write a fact: `Fact::StdinPipe` is `stdin_pipe`.
fn fact_order(facts: &str) -> Result<Vec<String>> {
    let mut lines = facts
        .lines()
        .skip_while(|line| !line.starts_with("pub const FACT_ORDER: [Fact; "));
    let declared: usize = lines
        .next()
        .and_then(|line| line.strip_prefix("pub const FACT_ORDER: [Fact; "))
        .and_then(|rest| rest.strip_suffix("] = ["))
        .and_then(|count| count.parse().ok())
        .with_context(|| format!("{FACTS} has no FACT_ORDER"))?;
    let order: Vec<String> = lines
        .take_while(|line| line.trim() != "];")
        .filter_map(|line| line.trim().strip_prefix("Fact::")?.strip_suffix(','))
        .map(|name| {
            let mut snake = String::new();
            for c in name.chars() {
                if c.is_ascii_uppercase() && !snake.is_empty() {
                    snake.push('_');
                }
                snake.push(c.to_ascii_lowercase());
            }
            snake
        })
        .collect();
    ensure!(
        order.len() == declared,
        "{FACTS}: FACT_ORDER declares {declared} facts and lists {}",
        order.len()
    );
    Ok(order)
}

fn current(root: &Path) -> Result<Current> {
    let release = workspace_version(&read_public(root, MANIFEST)?)?;
    Ok(Current {
        published: published(&read_public(root, CHANGELOG)?, &release)?,
        advertised: advertised(&read_public(root, WIRE)?)?,
        newest: newest_bundle(root)?,
        release,
    })
}

/// Every failure of the pages against the repository's current values, one line each.
pub(crate) fn check(root: &Path) -> Result<Vec<String>> {
    let current = current(root)?;
    let mut sources = Vec::new();
    for (page, _, _) in PAGES {
        let source = read_public(root, &file(page))?;
        // The generated tail of the release notes is the changelog's text, which is history.
        let authored = source
            .split_once(RELEASES_MARKER)
            .map_or(source.as_str(), |(authored, _)| authored)
            .to_owned();
        sources.push((*page, authored));
    }
    sources.push((STATUS_FILE, read_public(root, STATUS_FILE)?));
    let pages: Vec<(&str, &str)> = sources.iter().map(|(p, s)| (*p, s.as_str())).collect();
    let mut failures = stale_statements(&current, &pages);
    let facts = fact_order(&read_public(root, FACTS)?)?;
    let profiles = pages
        .iter()
        .find(|(page, _)| *page == FACT_PAGE)
        .with_context(|| format!("{FACT_PAGE} is not a public page"))?;
    failures.extend(fact_list(&facts, profiles.1));
    Ok(failures)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn current() -> Current {
        Current {
            release: "0.24.0".into(),
            published: "22 November 2026".into(),
            advertised: "0.27.0".into(),
            newest: "0.27.0".into(),
        }
    }
    /// Every form on its page with `current`'s values, one per line.
    fn pages(current: &Current) -> Vec<(&'static str, String)> {
        let mut pages: Vec<(&'static str, String)> = Vec::new();
        for (page, form) in FORMS {
            let line = format!("Text. {} More.\n", filled(form, current));
            match pages.iter_mut().find(|(p, _)| *p == *page) {
                Some((_, source)) => source.push_str(&line),
                None => pages.push((*page, line)),
            }
        }
        pages
    }
    fn stale(current: &Current, pages: &[(&'static str, String)]) -> Vec<String> {
        let pages: Vec<(&str, &str)> = pages.iter().map(|(p, s)| (*p, s.as_str())).collect();
        stale_statements(current, &pages)
    }

    /// UXF-009: the committed pages name the current release, its date, the advertised and the
    /// newest bundle and every fact of `FACT_ORDER`.
    #[test]
    fn the_pages_name_the_current_release_bundle_and_facts() {
        let failures = check(&crate::repo::root().unwrap()).unwrap();
        assert!(failures.is_empty(), "{failures:#?}");
    }
    /// A release bump names every line to change and what to write there.
    #[test]
    fn a_release_bump_names_each_stale_line_and_its_replacement() {
        let before = current();
        let written = pages(&before);
        assert!(stale(&before, &written).is_empty());
        let after = Current {
            release: "0.25.0".into(),
            published: "1 December 2026".into(),
            advertised: "0.28.0".into(),
            newest: "0.29.0".into(),
        };
        let failures = stale(&after, &written);
        // One line per form, one more for the tag in the bundle link, and the closing line with
        // the sources.
        assert_eq!(failures.len(), FORMS.len() + 2, "{failures:#?}");
        assert!(
            failures.contains(
                &"website/docs/getting-started.md:1: reads \"version=0.24.0\", write \"version=0.25.0\""
                    .to_owned()
            ),
            "{failures:#?}"
        );
        assert!(
            failures.contains(
                &"website/docs/reference/contracts.md:1: reads \"newest `0.27.0`,\", write \"newest `0.29.0`,\""
                    .to_owned()
            ),
            "{failures:#?}"
        );
        assert!(
            failures
                .last()
                .unwrap()
                .contains("Cargo.toml has version 0.25.0")
        );
        assert!(stale(&after, &pages(&after)).is_empty());
    }
    /// History is not rewritten: a sentence in no form passes, a form of one page is not read on
    /// another, the present tense of "advertises" fails on every page, and a page that lost its
    /// form fails.
    #[test]
    fn history_passes_and_the_present_tense_of_an_earlier_release_fails_on_any_page() {
        let now = current();
        let mut written = pages(&now);
        written.push((
            "guides/deployment",
            "From 0.16.1 a store refuses it. Release `0.16.1` was the first to advertise `substrate-wire/0.25.0`.\nBundle `0.23.0` added the header.\n`application/openmetrics-text; version=1.0.0`\n".into(),
        ));
        assert!(stale(&now, &written).is_empty());
        written.push((
            "security",
            "Line.\nRelease `0.16.1` advertises `substrate-wire/0.25.0` today.\n".into(),
        ));
        let failures = stale(&now, &written);
        assert_eq!(failures.len(), 2, "{failures:#?}");
        assert!(
            failures[0].starts_with("website/docs/security.md:2: reads \"Release `0.16.1` advertises `substrate-wire/0.25.0`\""),
            "{failures:#?}"
        );
        let mut lost = pages(&now);
        lost.retain(|(page, _)| *page != "releases");
        let failures = stale(&now, &lost);
        assert_eq!(
            failures[0],
            "website/docs/releases.md: no line reads \"The newest release is 0.24.0, published on 22 November 2026.\""
        );
    }
    /// `STATUS.md` is held like a site page: a line that calls another release the latest is
    /// refused with the file and its line, a history sentence passes, and a status file that
    /// lost its release line fails.
    #[test]
    fn a_status_file_naming_another_release_is_refused_with_file_and_line() {
        let now = current();
        let status = |text: &str| {
            let mut written = pages(&now);
            written.retain(|(owner, _)| *owner != STATUS_FILE);
            written.push((STATUS_FILE, text.to_owned()));
            stale(&now, &written)
        };
        let held = "# Repository status\n\nRelease [0.24.0](https://github.com/beyond10x/substrate/releases/tag/0.24.0), published on 22 November 2026,\nis the latest. Release `0.24.0` advertises `substrate-wire/0.27.0`.\nRelease `0.16.1` was published on 29 October 2026.\n\nThe latest annotated tag is `0.24.0` at `85da2e2`.\n| Source | the latest release is `0.24.0` |\n";
        assert!(status(held).is_empty(), "{:#?}", status(held));
        let earlier = held
            .replace("[0.24.0]", "[0.16.1]")
            .replace("tag/0.24.0", "tag/0.16.1")
            .replace("22 November 2026,", "29 October 2026,")
            .replace(
                "the latest release is `0.24.0`",
                "the latest release is `0.16.1`",
            );
        let failures = status(&earlier);
        assert_eq!(
            failures[..2],
            [
                "STATUS.md:3: reads \"Release [0.16.1](https://github.com/beyond10x/substrate/releases/tag/0.16.1), published on 29 October 2026,\", write \"Release [0.24.0](https://github.com/beyond10x/substrate/releases/tag/0.24.0), published on 22 November 2026,\"",
                "STATUS.md:8: reads \"the latest release is `0.16.1`\", write \"the latest release is `0.24.0`\"",
            ],
            "{failures:#?}"
        );
        assert_eq!(
            failures.len(),
            3,
            "two lines and the sources: {failures:#?}"
        );
        let lost = status("# Repository status\n\n`0.16.1` is the latest release (2026-10-29).\n");
        assert_eq!(lost.len(), 5, "four forms and the sources: {lost:#?}");
        assert_eq!(
            lost[1],
            "STATUS.md: no line reads \"Release `0.24.0` advertises `substrate-wire/0.27.0`\""
        );
    }
    /// A source link pinned to a tag is pinned to the current one; `main` and a release page of an
    /// earlier version are not pins of the source.
    #[test]
    fn a_source_link_pinned_to_an_earlier_tag_fails() {
        let now = current();
        let mut written = pages(&now);
        written.push((
            "security",
            "[a](https://github.com/beyond10x/substrate/blob/main/LICENSE) [b](https://github.com/beyond10x/substrate/tree/0.24.0/crates) [c](https://github.com/beyond10x/substrate/releases/tag/0.16.1)\n".into(),
        ));
        assert!(stale(&now, &written).is_empty());
        written.push((
            "index",
            "x\n[d](https://github.com/beyond10x/substrate/blob/0.16.1/crates/a.rs) [e](https://github.com/beyond10x/substrate/tree/0.16.1/crates)\n".into(),
        ));
        let failures = stale(&now, &written);
        assert_eq!(failures.len(), 2, "{failures:#?}");
        assert_eq!(
            failures[0],
            "website/docs/index.md:2: a source link is pinned to tag 0.16.1, write 0.24.0"
        );
    }
    #[test]
    fn the_fact_list_is_fact_order() {
        let facts: Vec<String> = ["declared_insecure", "stdin_pipe", "output_live"]
            .map(str::to_owned)
            .into();
        let page = |list: &str| {
            format!(
                "---\ntitle: T\n---\n\n- `served_families`: served, in this order: `a`, `b`. More.\n- `facts`: what the adapter proved, in this order: {list}.\n  `proves(&fact)` asks for one.\n"
            )
        };
        assert!(
            fact_list(
                &facts,
                &page("`declared_insecure`, `stdin_pipe`,\n  `output_live`")
            )
            .is_empty()
        );
        let short = fact_list(&facts, &page("`declared_insecure`"));
        assert_eq!(short.len(), 1);
        assert!(
            short[0].starts_with(
                "website/docs/guides/confinement-profiles.md:6: the fact list has 1 facts"
            ) && short[0].contains("missing [\"stdin_pipe\", \"output_live\"]"),
            "{short:?}"
        );
        assert_eq!(
            fact_list(
                &facts,
                &page("`stdin_pipe`, `declared_insecure`, `output_live`")
            )
            .len(),
            1,
            "order"
        );
        assert_eq!(fact_list(&facts, "no list").len(), 1);
    }
    #[test]
    fn the_sources_are_read_in_their_written_form() {
        assert_eq!(
            workspace_version("[package]\nversion = \"9.9.9\"\n[workspace.package]\nedition = \"2024\"\nversion = \"0.24.0\"\n").unwrap(),
            "0.24.0"
        );
        assert!(workspace_version("[package]\nversion = \"1.0.0\"\n").is_err());
        let changelog = "## [Unreleased]\n\n## [0.24.0] — 2026-11-22\n\n## [0.23.0] — 2026-11-01\n";
        assert_eq!(published(changelog, "0.24.0").unwrap(), "22 November 2026");
        assert_eq!(published(changelog, "0.23.0").unwrap(), "1 November 2026");
        assert!(published(changelog, "0.25.0").is_err());
        assert_eq!(
            advertised("pub const ADVERTISED_CONTRACT_BUNDLE: &str = \"substrate-wire/0.27.0\";\n")
                .unwrap(),
            "0.27.0"
        );
        assert!(advertised("pub const OTHER: &str = \"substrate-wire/0.27.0\";\n").is_err());
        assert_eq!(
            fact_order("pub const FACT_ORDER: [Fact; 2] = [\n    Fact::DeclaredInsecure,\n    Fact::IpcSocketAllowlist,\n];\n").unwrap(),
            ["declared_insecure", "ipc_socket_allowlist"]
        );
        assert!(fact_order("pub const FACT_ORDER: [Fact; 3] = [\n    Fact::A,\n];\n").is_err());
        let dir = tempfile::tempdir().unwrap();
        for name in ["0.9.0", "0.10.0", "0.2.0", "notes"] {
            std::fs::create_dir_all(dir.path().join(BUNDLES).join(name)).unwrap();
        }
        assert_eq!(newest_bundle(dir.path()).unwrap(), "0.10.0");
    }
    #[test]
    fn a_form_is_matched_by_shape_with_any_version_or_date() {
        let form = "Release [{release}](x/{release}), published on {published},";
        assert_eq!(
            shaped(
                "a Release [0.1.2](x/0.1.2), published on 9 May 2026, b",
                form
            ),
            Some("Release [0.1.2](x/0.1.2), published on 9 May 2026,")
        );
        assert_eq!(
            shaped("Release [0.1](x/0.1), published on 9 May 2026,", form),
            None
        );
        assert_eq!(
            shaped("Release [0.1.2](x/0.1.2), published in May,", form),
            None
        );
        assert_eq!(shaped("version=$version", "version={release}"), None);
        assert_eq!(
            shaped("version=10.20.30", "version={release}"),
            Some("version=10.20.30")
        );
    }
}
