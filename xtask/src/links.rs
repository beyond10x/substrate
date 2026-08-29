//! Reject machine-local Markdown links and broken repository-relative targets.

use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

use crate::report::Report;

/// The only schemes a document in this repository may link out with.
const EXTERNAL: [&str; 3] = ["http", "https", "mailto"];
/// Prefixes that name a place on one machine rather than a place in the repository.
const LOCAL_PREFIXES: [&str; 4] = ["/", "~/", "file://", "vscode://"];

pub fn check(root: &Path) -> Result<Report> {
    let root = root
        .canonicalize()
        .with_context(|| format!("cannot resolve {}", root.display()))?;
    let mut failures = Vec::new();

    for relative in markdown_documents(&root)? {
        let document = root.join(&relative);
        if !document.is_file() {
            continue;
        }
        let source = fs::read_to_string(&document)
            .with_context(|| format!("cannot read {}", document.display()))?;
        let directory = document.parent().unwrap_or(&root).to_path_buf();
        for (index, line) in source.lines().enumerate() {
            let number = index + 1;
            for raw in link_targets(line) {
                let target = target_text(raw);
                if target.is_empty() || target.starts_with('#') {
                    continue;
                }
                if is_machine_local(target) {
                    failures.push(format!("{relative}:{number}: machine-local link: {target}"));
                    continue;
                }
                let (scheme, path) = split_scheme_and_path(target);
                if !scheme.is_empty() {
                    if !EXTERNAL.contains(&scheme.as_str()) {
                        failures.push(format!(
                            "{relative}:{number}: unsupported link scheme: {target}"
                        ));
                    }
                    continue;
                }
                let decoded = percent_decode(path);
                if decoded.is_empty() {
                    continue;
                }
                let resolved = resolve(&directory, &decoded);
                if !resolved.starts_with(&root) {
                    failures.push(format!(
                        "{relative}:{number}: link escapes repository: {target}"
                    ));
                    continue;
                }
                if !resolved.exists() {
                    failures.push(format!(
                        "{relative}:{number}: missing link target: {target}"
                    ));
                }
            }
        }
    }

    if failures.is_empty() {
        return Ok(Report::passed("markdown links are repository-portable"));
    }
    Ok(Report::failed(failures))
}

/// Every tracked or untracked-but-not-ignored `*.md`, in sorted order.
fn markdown_documents(root: &Path) -> Result<Vec<String>> {
    let output = Command::new("git")
        .args([
            "ls-files",
            "--cached",
            "--others",
            "--exclude-standard",
            "--",
            "*.md",
        ])
        .current_dir(root)
        .output()
        .context("running `git ls-files`")?;
    if !output.status.success() {
        bail!(
            "`git ls-files` failed in {}: {}",
            root.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let listing =
        String::from_utf8(output.stdout).context("`git ls-files` printed invalid UTF-8")?;
    let mut documents: Vec<String> = listing
        .lines()
        .filter(|line| !line.is_empty())
        .map(ToOwned::to_owned)
        .collect();
    documents.sort();
    Ok(documents)
}

/// The target of every `[text](target)` and `![text](target)` on one line.
fn link_targets(line: &str) -> Vec<&str> {
    let bytes = line.as_bytes();
    let mut targets = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'[' {
            index += 1;
            continue;
        }
        let Some(offset) = line[index + 1..].find(']') else {
            break;
        };
        let close = index + 1 + offset;
        if bytes.get(close + 1) != Some(&b'(') {
            index += 1;
            continue;
        }
        let open = close + 2;
        let Some(width) = line[open..].find(')') else {
            break;
        };
        if width == 0 {
            index += 1;
            continue;
        }
        targets.push(&line[open..open + width]);
        index = open + width + 1;
    }
    targets
}

/// The destination itself: `<a b.md>` unwrapped, or the first whitespace-delimited word.
fn target_text(raw: &str) -> &str {
    let value = raw.trim();
    if let Some(rest) = value.strip_prefix('<')
        && let Some(end) = rest.find('>')
    {
        return &rest[..end];
    }
    value.split_whitespace().next().unwrap_or("")
}

fn is_machine_local(target: &str) -> bool {
    if LOCAL_PREFIXES
        .iter()
        .any(|prefix| target.starts_with(prefix))
    {
        return true;
    }
    let mut characters = target.chars();
    match (characters.next(), characters.next(), characters.next()) {
        (Some(letter), Some(':'), Some(separator)) => {
            letter.is_ascii_alphabetic() && (separator == '\\' || separator == '/')
        }
        _ => false,
    }
}

/// The scheme and the path of a URL reference, as `urllib.parse.urlsplit` reads them: the
/// authority, the query and the fragment are dropped, so an anchor never reaches the filesystem.
fn split_scheme_and_path(target: &str) -> (String, &str) {
    let mut scheme = String::new();
    let mut rest = target;
    if let Some(colon) = target.find(':')
        && colon > 0
    {
        let candidate = &target[..colon];
        let starts_well = candidate
            .chars()
            .next()
            .is_some_and(|first| first.is_ascii_alphabetic());
        let continues_well = candidate.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '+' | '-' | '.')
        });
        if starts_well && continues_well {
            scheme = candidate.to_ascii_lowercase();
            rest = &target[colon + 1..];
        }
    }
    if let Some(authority) = rest.strip_prefix("//") {
        let end = authority.find(['/', '?', '#']).unwrap_or(authority.len());
        rest = &authority[end..];
    }
    if let Some(fragment) = rest.find('#') {
        rest = &rest[..fragment];
    }
    if let Some(query) = rest.find('?') {
        rest = &rest[..query];
    }
    (scheme, rest)
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut decoded: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && let (Some(high), Some(low)) =
                (hex_digit(bytes[index + 1]), hex_digit(bytes[index + 2]))
        {
            decoded.push(high * 16 + low);
            index += 3;
            continue;
        }
        decoded.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// `directory / target`, with `.` and `..` removed and symlinks resolved where the path exists —
/// the escape check and the existence check both need the real location.
fn resolve(directory: &Path, target: &str) -> PathBuf {
    let mut normalised = PathBuf::new();
    for component in directory.join(target).components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalised.pop() {
                    normalised.push("..");
                }
            }
            other => normalised.push(other.as_os_str()),
        }
    }
    fs::canonicalize(&normalised).unwrap_or(normalised)
}

#[cfg(test)]
mod tests {
    use super::check;
    use std::fs;
    use std::path::Path;
    use std::process::Command;
    use tempfile::TempDir;

    /// A fixture repository: `git ls-files --others` is what the check lists, so the tree only
    /// needs to be a git repository, not one with commits.
    fn repository(documents: &[(&str, &str)]) -> TempDir {
        let directory = tempfile::tempdir().expect("temporary directory");
        for (relative, contents) in documents {
            write(directory.path(), relative, contents);
        }
        let status = Command::new("git")
            .args(["init", "-q"])
            .current_dir(directory.path())
            .status()
            .expect("git init");
        assert!(status.success(), "git init failed");
        directory
    }

    fn write(root: &Path, relative: &str, contents: &str) {
        let path = root.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create fixture directory");
        }
        fs::write(path, contents).expect("write fixture");
    }

    #[test]
    fn a_portable_tree_passes() {
        let directory = repository(&[
            (
                "README.md",
                concat!(
                    "[guide](docs/guide.md)\n",
                    "[anchor](#section)\n",
                    "[anchored](docs/guide.md#top)\n",
                    "[external](https://example.invalid/page)\n",
                    "[mail](mailto:nobody@example.invalid)\n",
                    "[encoded](docs/a%20space.md)\n",
                    "![image](docs/guide.md)\n",
                    "[angled](<docs/guide.md>)\n",
                ),
            ),
            ("docs/guide.md", "# Guide\n"),
            ("docs/a space.md", "# Spaced\n"),
        ]);
        let report = check(directory.path()).expect("check runs");
        assert_eq!(report.failures(), &[] as &[String]);
        assert_eq!(report.summary(), "markdown links are repository-portable");
    }

    #[test]
    fn machine_local_links_are_rejected() {
        let directory = repository(&[(
            "README.md",
            concat!(
                "[absolute](/etc/hostname)\n",
                "[home](~/notes.md)\n",
                "[file](file:///etc/hostname)\n",
                "[editor](vscode://file/home/someone/notes.md)\n",
                "[drive](C:\\notes.md)\n",
            ),
        )]);
        let report = check(directory.path()).expect("check runs");
        let text = report.failure_text();
        assert!(
            text.contains("README.md:1: machine-local link: /etc/hostname"),
            "{text}"
        );
        assert!(
            text.contains("README.md:2: machine-local link: ~/notes.md"),
            "{text}"
        );
        assert!(
            text.contains("README.md:3: machine-local link: file:///etc/hostname"),
            "{text}"
        );
        assert!(
            text.contains("README.md:4: machine-local link: vscode://file/home/someone/notes.md"),
            "{text}"
        );
        assert!(
            text.contains("README.md:5: machine-local link: C:\\notes.md"),
            "{text}"
        );
        assert_eq!(report.failures().len(), 5, "{text}");
    }

    #[test]
    fn an_unsupported_scheme_is_rejected() {
        let directory = repository(&[(
            "README.md",
            "[ftp](ftp://example.invalid/x)\n[git](git+ssh://example.invalid/x.git)\n",
        )]);
        let report = check(directory.path()).expect("check runs");
        let text = report.failure_text();
        assert!(
            text.contains("README.md:1: unsupported link scheme: ftp://example.invalid/x"),
            "{text}"
        );
        assert!(
            text.contains("README.md:2: unsupported link scheme: git+ssh://example.invalid/x.git"),
            "{text}"
        );
        assert_eq!(report.failures().len(), 2, "{text}");
    }

    #[test]
    fn a_link_that_escapes_the_repository_is_rejected() {
        let directory = repository(&[
            ("docs/page.md", "[sibling](../../elsewhere/notes.md)\n"),
            ("README.md", "[up](../outside.md)\n"),
        ]);
        let report = check(directory.path()).expect("check runs");
        let text = report.failure_text();
        assert!(
            text.contains("README.md:1: link escapes repository: ../outside.md"),
            "{text}"
        );
        assert!(
            text.contains("docs/page.md:1: link escapes repository: ../../elsewhere/notes.md"),
            "{text}"
        );
        assert_eq!(report.failures().len(), 2, "{text}");
    }

    #[test]
    fn a_missing_target_is_rejected() {
        let directory = repository(&[("README.md", "text\n[gone](docs/absent.md)\n")]);
        let report = check(directory.path()).expect("check runs");
        assert_eq!(
            report.failure_text(),
            "README.md:2: missing link target: docs/absent.md"
        );
    }

    #[test]
    fn an_empty_or_anchor_only_target_is_skipped() {
        let directory = repository(&[("README.md", "[empty]()\n[anchor](#top)\n[blank](   )\n")]);
        let report = check(directory.path()).expect("check runs");
        assert_eq!(report.failures(), &[] as &[String]);
    }
}
