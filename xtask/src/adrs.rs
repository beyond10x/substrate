//! Validate ADR identity, frontmatter, index agreement, and supersession links.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};

use crate::report::Report;

/// The only statuses an ADR may declare.
const STATUSES: [&str; 2] = ["accepted", "superseded"];

/// One ADR file, keyed elsewhere by its four-digit number.
struct Record {
    name: String,
    status: String,
    body: String,
}

pub fn check(root: &Path) -> Result<Report> {
    let mut failures = Vec::new();
    let (order, records) = read_records(root, &mut failures)?;
    let index = read_index(root, &mut failures)?;

    for number in &order {
        let record = &records[number];
        compare(number, record, &records, &index, &mut failures);
    }

    let mut orphans: Vec<&String> = index
        .keys()
        .filter(|number| !records.contains_key(*number))
        .collect();
    orphans.sort();
    for number in orphans {
        failures.push(format!("adr/README.md: row {number} has no ADR file"));
    }

    if failures.is_empty() {
        return Ok(Report::passed(format!(
            "ADR index and {} records are consistent",
            records.len()
        )));
    }
    Ok(Report::failed(failures))
}

/// Every `adr/NNNN-slug.md`, in filename order, with its frontmatter and heading checked.
fn read_records(
    root: &Path,
    failures: &mut Vec<String>,
) -> Result<(Vec<String>, HashMap<String, Record>)> {
    let mut order: Vec<String> = Vec::new();
    let mut records: HashMap<String, Record> = HashMap::new();

    for name in markdown_names(&root.join("adr")) {
        let Some(number) = adr_number(&name) else {
            continue;
        };
        if let Some(existing) = records.get(&number) {
            failures.push(format!(
                "adr/{name}: duplicate ADR number {number}; also used by {}",
                existing.name
            ));
            continue;
        }
        let path = root.join("adr").join(&name);
        let text =
            fs::read_to_string(&path).with_context(|| format!("cannot read {}", path.display()))?;
        let (fields, body) = frontmatter(&name, &text, failures);

        let status = fields.get("status").cloned().unwrap_or_default();
        if !STATUSES.contains(&status.as_str()) {
            failures.push(format!(
                "adr/{name}: status must be one of ['accepted', 'superseded']"
            ));
        }
        if !is_iso_date(fields.get("date").map_or("", String::as_str)) {
            failures.push(format!("adr/{name}: date must use YYYY-MM-DD"));
        }
        let heading = body
            .iter()
            .find(|line| !line.trim().is_empty())
            .map_or("", String::as_str);
        if heading_number(heading) != Some(number.as_str()) {
            failures.push(format!(
                "adr/{name}: first heading must be '# ADR {number}: \u{2026}'"
            ));
        }

        records.insert(
            number.clone(),
            Record {
                name,
                status,
                body: body.join("\n"),
            },
        );
        order.push(number);
    }
    Ok((order, records))
}

/// The rows of `adr/README.md`: number to (linked file, status text).
fn read_index(
    root: &Path,
    failures: &mut Vec<String>,
) -> Result<HashMap<String, (String, String)>> {
    let readme = root.join("adr").join("README.md");
    let text =
        fs::read_to_string(&readme).with_context(|| format!("cannot read {}", readme.display()))?;
    let mut index: HashMap<String, (String, String)> = HashMap::new();
    for (offset, line) in text.lines().enumerate() {
        let Some((number, file, status)) = index_row(line) else {
            continue;
        };
        if index.contains_key(&number) {
            let count = offset + 1;
            failures.push(format!("adr/README.md:{count}: duplicate ADR row {number}"));
        }
        index.insert(number, (file, status));
    }
    Ok(index)
}

/// One record against the index: the row exists, links this file, and agrees about supersession.
fn compare(
    number: &str,
    record: &Record,
    records: &HashMap<String, Record>,
    index: &HashMap<String, (String, String)>,
    failures: &mut Vec<String>,
) {
    let Some((file, status_text)) = index.get(number) else {
        failures.push(format!("adr/README.md: missing row for ADR {number}"));
        return;
    };
    if file != &record.name {
        failures.push(format!(
            "adr/README.md: ADR {number} links {file}, expected {}",
            record.name
        ));
    }
    if record.status == "accepted" && status_text != "accepted" {
        failures.push(format!(
            "adr/README.md: ADR {number} status disagrees with frontmatter"
        ));
    }
    if record.status != "superseded" {
        return;
    }
    let Some(successor) = superseded_by(status_text) else {
        failures.push(format!(
            "adr/README.md: superseded ADR {number} must say 'superseded by NNNN'"
        ));
        return;
    };
    if records
        .get(&successor)
        .is_none_or(|record| record.status != "accepted")
    {
        failures.push(format!(
            "adr/README.md: ADR {number} successor {successor} is not accepted"
        ));
    }
    if !has_linked_reference(&record.body, "superseded by", &successor) {
        failures.push(format!(
            "adr/{}: missing linked 'superseded by ADR {successor}'",
            record.name
        ));
    }
    if let Some(successor_record) = records.get(&successor)
        && !has_linked_reference(&successor_record.body, "supersedes", number)
    {
        failures.push(format!(
            "adr/{}: missing linked 'supersedes ADR {number}'",
            successor_record.name
        ));
    }
}

/// The `*.md` filenames in a directory, sorted; an absent directory has none.
fn markdown_names(directory: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
        .filter(|name| name.as_bytes().ends_with(b".md"))
        .collect();
    names.sort();
    names
}

/// A literal `.md` ending, case-sensitively, as the predecessor's `\.md$` was.
fn strip_md_suffix(name: &str) -> Option<&str> {
    let bytes = name.as_bytes();
    if bytes.ends_with(b".md") {
        return Some(&name[..name.len() - 3]);
    }
    None
}

/// The number of an `NNNN-slug.md` filename.
fn adr_number(name: &str) -> Option<String> {
    let rest = strip_md_suffix(name)?;
    let bytes = rest.as_bytes();
    if bytes.len() < 6 || !bytes[..4].iter().all(u8::is_ascii_digit) || bytes[4] != b'-' {
        return None;
    }
    Some(rest[..4].to_owned())
}

/// The YAML frontmatter fields and the body that follows the closing fence.
fn frontmatter(
    name: &str,
    text: &str,
    failures: &mut Vec<String>,
) -> (HashMap<String, String>, Vec<String>) {
    let lines: Vec<String> = text.lines().map(ToOwned::to_owned).collect();
    if lines.first().map_or("", String::as_str) != "---" {
        failures.push(format!(
            "adr/{name}: missing opening YAML frontmatter fence"
        ));
        return (HashMap::new(), lines);
    }
    let Some(end) = lines.iter().skip(1).position(|line| line == "---") else {
        failures.push(format!(
            "adr/{name}: missing closing YAML frontmatter fence"
        ));
        return (HashMap::new(), lines);
    };
    let end = end + 1;
    let mut fields = HashMap::new();
    for line in &lines[1..end] {
        if let Some((key, value)) = line.split_once(':') {
            fields.insert(key.trim().to_owned(), value.trim().to_owned());
        }
    }
    (fields, lines[end + 1..].to_vec())
}

fn is_iso_date(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 10
        && bytes[..4].iter().all(u8::is_ascii_digit)
        && bytes[4] == b'-'
        && bytes[5..7].iter().all(u8::is_ascii_digit)
        && bytes[7] == b'-'
        && bytes[8..].iter().all(u8::is_ascii_digit)
}

/// The number in a `# ADR NNNN: Title` heading.
fn heading_number(heading: &str) -> Option<&str> {
    let rest = heading.strip_prefix("# ADR ")?;
    let bytes = rest.as_bytes();
    if bytes.len() < 5 || !bytes[..4].iter().all(u8::is_ascii_digit) || bytes[4] != b':' {
        return None;
    }
    let mut after = rest[5..].chars();
    if !after.next()?.is_whitespace() || after.next().is_none() {
        return None;
    }
    Some(&rest[..4])
}

/// `| [NNNN](NNNN-slug.md) | decision | status |`.
fn index_row(line: &str) -> Option<(String, String, String)> {
    let rest = line.strip_prefix("| [")?;
    let bytes = rest.as_bytes();
    if bytes.len() < 4 || !bytes[..4].iter().all(u8::is_ascii_digit) {
        return None;
    }
    let number = rest[..4].to_owned();
    let rest = rest[4..].strip_prefix("](")?;
    let end = rest.find(')')?;
    let file = &rest[..end];
    let file_bytes = file.as_bytes();
    if file_bytes.len() < 5 || !file_bytes[..4].iter().all(u8::is_ascii_digit) {
        return None;
    }
    let tail = file[4..].strip_prefix('-')?;
    if tail.len() < 4 || !tail.as_bytes().ends_with(b".md") {
        return None;
    }
    let inner = rest[end + 1..].strip_prefix(" | ")?.strip_suffix(" |")?;
    let status = last_column(inner)?;
    Some((number, file.to_owned(), status.trim().to_owned()))
}

/// The status cell: the text after the last ` | ` that no pipe follows.
fn last_column(inner: &str) -> Option<&str> {
    let mut search = inner.len();
    while let Some(position) = inner[..search].rfind(" | ") {
        let decision = &inner[..position];
        let status = &inner[position + 3..];
        if !decision.is_empty() && !status.is_empty() && !status.contains('|') {
            return Some(status);
        }
        search = position + 2;
    }
    None
}

fn superseded_by(status_text: &str) -> Option<String> {
    let rest = status_text.strip_prefix("superseded by ")?;
    let bytes = rest.as_bytes();
    if bytes.len() == 4 && bytes.iter().all(u8::is_ascii_digit) {
        return Some(rest.to_owned());
    }
    None
}

/// `<phrase> [ADR NNNN](NNNN-slug.md)`, case-insensitively, anywhere in the body.
fn has_linked_reference(body: &str, phrase: &str, number: &str) -> bool {
    let haystack = body.to_ascii_lowercase();
    let needle = format!("{phrase} [adr {number}](");
    let prefix = format!("{number}-");
    let mut from = 0;
    while let Some(position) = haystack[from..].find(&needle) {
        let start = from + position + needle.len();
        if let Some(end) = haystack[start..].find(')') {
            let inside = &haystack[start..start + end];
            if let Some(tail) = inside.strip_prefix(&prefix)
                && tail.len() >= 4
                && tail.as_bytes().ends_with(b".md")
            {
                return true;
            }
        }
        from = from + position + 1;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::check;
    use std::fs;
    use std::path::Path;
    use tempfile::TempDir;

    fn record(number: &str, title: &str, status: &str, date: &str, body: &str) -> String {
        format!("---\nstatus: {status}\ndate: {date}\n---\n\n# ADR {number}: {title}\n\n{body}\n")
    }

    fn index(rows: &[String]) -> String {
        let mut text = String::from(
            "# Architecture decision records\n\n| ADR | Decision | Status |\n|---|---|---|\n",
        );
        for row in rows {
            text.push_str(row);
            text.push('\n');
        }
        text
    }

    fn row(number: &str, file: &str, decision: &str, status: &str) -> String {
        format!("| [{number}]({file}) | {decision} | {status} |")
    }

    fn tree(files: &[(&str, String)]) -> TempDir {
        let directory = tempfile::tempdir().expect("temporary directory");
        fs::create_dir_all(directory.path().join("adr")).expect("create adr directory");
        for (name, contents) in files {
            write(directory.path(), name, contents);
        }
        directory
    }

    fn write(root: &Path, name: &str, contents: &str) {
        fs::write(root.join("adr").join(name), contents).expect("write fixture");
    }

    fn accepted_pair() -> Vec<(&'static str, String)> {
        vec![
            (
                "0001-first.md",
                record("0001", "First", "accepted", "2026-01-01", "Body."),
            ),
            (
                "0002-second.md",
                record("0002", "Second", "accepted", "2026-02-02", "Body."),
            ),
            (
                "README.md",
                index(&[
                    row("0001", "0001-first.md", "First", "accepted"),
                    row("0002", "0002-second.md", "Second", "accepted"),
                ]),
            ),
        ]
    }

    #[test]
    fn a_consistent_tree_passes() {
        let directory = tree(&accepted_pair());
        let report = check(directory.path()).expect("check runs");
        assert_eq!(report.failures(), &[] as &[String]);
        assert_eq!(report.summary(), "ADR index and 2 records are consistent");
    }

    #[test]
    fn a_status_outside_the_set_is_rejected() {
        let directory = tree(&accepted_pair());
        write(
            directory.path(),
            "0002-second.md",
            &record("0002", "Second", "proposed", "2026-02-02", "Body."),
        );
        let report = check(directory.path()).expect("check runs");
        let text = report.failure_text();
        assert!(
            text.contains("adr/0002-second.md: status must be one of ['accepted', 'superseded']"),
            "{text}"
        );
    }

    #[test]
    fn a_date_that_is_not_iso_is_rejected() {
        let directory = tree(&accepted_pair());
        write(
            directory.path(),
            "0002-second.md",
            &record("0002", "Second", "accepted", "2026-2-2", "Body."),
        );
        let report = check(directory.path()).expect("check runs");
        assert_eq!(
            report.failure_text(),
            "adr/0002-second.md: date must use YYYY-MM-DD"
        );
    }

    #[test]
    fn a_duplicate_adr_number_is_rejected() {
        let mut files = accepted_pair();
        files.push((
            "0002-again.md",
            record("0002", "Again", "accepted", "2026-03-03", "Body."),
        ));
        let directory = tree(&files);
        let report = check(directory.path()).expect("check runs");
        let text = report.failure_text();
        assert!(
            text.contains(
                "adr/0002-second.md: duplicate ADR number 0002; also used by 0002-again.md"
            ),
            "{text}"
        );
    }

    #[test]
    fn an_index_that_disagrees_with_the_records_is_rejected() {
        let directory = tree(&accepted_pair());
        write(
            directory.path(),
            "README.md",
            &index(&[
                row("0001", "0001-renamed.md", "First", "accepted"),
                row("0003", "0003-absent.md", "Absent", "accepted"),
            ]),
        );
        let report = check(directory.path()).expect("check runs");
        let text = report.failure_text();
        assert!(
            text.contains("adr/README.md: ADR 0001 links 0001-renamed.md, expected 0001-first.md"),
            "{text}"
        );
        assert!(
            text.contains("adr/README.md: missing row for ADR 0002"),
            "{text}"
        );
        assert!(
            text.contains("adr/README.md: row 0003 has no ADR file"),
            "{text}"
        );
    }

    #[test]
    fn an_index_status_that_disagrees_with_the_frontmatter_is_rejected() {
        let directory = tree(&accepted_pair());
        write(
            directory.path(),
            "README.md",
            &index(&[
                row("0001", "0001-first.md", "First", "accepted"),
                row("0002", "0002-second.md", "Second", "superseded by 0003"),
            ]),
        );
        let report = check(directory.path()).expect("check runs");
        let text = report.failure_text();
        assert!(
            text.contains("adr/README.md: ADR 0002 status disagrees with frontmatter"),
            "{text}"
        );
    }

    #[test]
    fn a_duplicate_index_row_is_rejected() {
        let directory = tree(&accepted_pair());
        write(
            directory.path(),
            "README.md",
            &index(&[
                row("0001", "0001-first.md", "First", "accepted"),
                row("0001", "0001-first.md", "First", "accepted"),
                row("0002", "0002-second.md", "Second", "accepted"),
            ]),
        );
        let report = check(directory.path()).expect("check runs");
        let text = report.failure_text();
        assert!(
            text.contains("adr/README.md:6: duplicate ADR row 0001"),
            "{text}"
        );
    }

    #[test]
    fn a_missing_frontmatter_fence_is_rejected() {
        let directory = tree(&accepted_pair());
        write(
            directory.path(),
            "0002-second.md",
            "# ADR 0002: Second\n\nBody.\n",
        );
        let report = check(directory.path()).expect("check runs");
        let text = report.failure_text();
        assert!(
            text.contains("adr/0002-second.md: missing opening YAML frontmatter fence"),
            "{text}"
        );
    }

    #[test]
    fn a_heading_that_does_not_name_the_number_is_rejected() {
        let directory = tree(&accepted_pair());
        write(
            directory.path(),
            "0002-second.md",
            &record("0007", "Second", "accepted", "2026-02-02", "Body."),
        );
        let report = check(directory.path()).expect("check runs");
        assert_eq!(
            report.failure_text(),
            "adr/0002-second.md: first heading must be '# ADR 0002: …'"
        );
    }

    #[test]
    fn a_linked_supersession_pair_passes() {
        let directory = tree(&[
            (
                "0001-first.md",
                record(
                    "0001",
                    "First",
                    "superseded",
                    "2026-01-01",
                    "Superseded by [ADR 0002](0002-second.md).",
                ),
            ),
            (
                "0002-second.md",
                record(
                    "0002",
                    "Second",
                    "accepted",
                    "2026-02-02",
                    "Supersedes [ADR 0001](0001-first.md).",
                ),
            ),
            (
                "README.md",
                index(&[
                    row("0001", "0001-first.md", "First", "superseded by 0002"),
                    row("0002", "0002-second.md", "Second", "accepted"),
                ]),
            ),
        ]);
        let report = check(directory.path()).expect("check runs");
        assert_eq!(report.failures(), &[] as &[String]);
        assert_eq!(report.summary(), "ADR index and 2 records are consistent");
    }

    #[test]
    fn a_supersession_without_both_links_is_rejected() {
        let directory = tree(&[
            (
                "0001-first.md",
                record("0001", "First", "superseded", "2026-01-01", "Body."),
            ),
            (
                "0002-second.md",
                record("0002", "Second", "accepted", "2026-02-02", "Body."),
            ),
            (
                "README.md",
                index(&[
                    row("0001", "0001-first.md", "First", "superseded by 0002"),
                    row("0002", "0002-second.md", "Second", "accepted"),
                ]),
            ),
        ]);
        let report = check(directory.path()).expect("check runs");
        let text = report.failure_text();
        assert!(
            text.contains("adr/0001-first.md: missing linked 'superseded by ADR 0002'"),
            "{text}"
        );
        assert!(
            text.contains("adr/0002-second.md: missing linked 'supersedes ADR 0001'"),
            "{text}"
        );
    }

    #[test]
    fn a_supersession_row_that_is_not_the_required_phrase_is_rejected() {
        let directory = tree(&[
            (
                "0001-first.md",
                record("0001", "First", "superseded", "2026-01-01", "Body."),
            ),
            (
                "README.md",
                index(&[row("0001", "0001-first.md", "First", "superseded")]),
            ),
        ]);
        let report = check(directory.path()).expect("check runs");
        assert_eq!(
            report.failure_text(),
            "adr/README.md: superseded ADR 0001 must say 'superseded by NNNN'"
        );
    }
}
