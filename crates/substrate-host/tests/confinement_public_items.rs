//! The public items of `substrate_host::confinement`, held to the committed list
//! `tests/confinement-public-items.txt` (`story:confinement-consumer-surface`).
//!
//! The list is what loom-sdk and takt build against (atlas ADR 0109 decision 6). An item added,
//! removed or changed in signature without the list fails here; a change that is meant updates the
//! list in the same commit, so the review sees it:
//!
//! ```console
//! SUBSTRATE_BLESS_PUBLIC_ITEMS=1 cargo test -p b10x-substrate-host --test confinement_public_items
//! ```
//!
//! The items are read from the source, which `cargo fmt --check` keeps in one layout, by a scanner
//! with no dependency beyond the standard library. It records, per file of `src/confinement/` and
//! for the three types the module re-exports from `src/generated/confinement.rs`:
//!
//! - every `pub` item and `pub` field, the signature without its body, a constant with its value;
//! - every variant of a `pub` enum, with its fields;
//! - every method, constant and type of a `pub` trait, without a default body;
//! - every trait implementation's header;
//! - the `derive`, `serde`, `cfg`, `repr`, `non_exhaustive` and `async_trait` attributes of each.
//!
//! It reads every `cfg` arm, so the list is the same on every OS. It also records `pub` items of a
//! private module, which a consumer cannot reach: over-inclusive, so a change there is flagged
//! rather than missed. `#[cfg(test)]` modules and function bodies are skipped. Whitespace and line
//! breaks are normalized, so reformatting a signature changes nothing.

use std::path::{Path, PathBuf};

const LIST: &str = "tests/confinement-public-items.txt";
const BLESS: &str = "SUBSTRATE_BLESS_PUBLIC_ITEMS";
const GENERATED: &str = "generated/confinement.rs";
const KEPT_ATTRIBUTES: [&str; 6] = [
    "#[derive",
    "#[serde",
    "#[cfg",
    "#[repr",
    "#[non_exhaustive",
    "#[async_trait",
];

/// Replaces comments, and the contents of string and character literals, with spaces, keeping
/// every newline and the character count of every line. Returns (code, code with literals kept).
fn mask(source: &str) -> (Vec<char>, Vec<char>) {
    let chars: Vec<char> = source.chars().collect();
    let mut code = chars.clone();
    let mut kept = chars.clone();
    let blank = |out: &mut Vec<char>, index: usize| {
        if out[index] != '\n' {
            out[index] = ' ';
        }
    };
    let ident = |c: char| c.is_alphanumeric() || c == '_';
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        if c == '/' && next == Some('/') {
            while i < chars.len() && chars[i] != '\n' {
                blank(&mut code, i);
                blank(&mut kept, i);
                i += 1;
            }
        } else if c == '/' && next == Some('*') {
            let mut depth = 0;
            while i < chars.len() {
                if chars[i] == '/' && chars.get(i + 1) == Some(&'*') {
                    depth += 1;
                    blank(&mut code, i);
                    blank(&mut kept, i);
                    i += 1;
                } else if chars[i] == '*' && chars.get(i + 1) == Some(&'/') {
                    depth -= 1;
                    blank(&mut code, i);
                    blank(&mut kept, i);
                    i += 1;
                }
                blank(&mut code, i);
                blank(&mut kept, i);
                i += 1;
                if depth == 0 {
                    break;
                }
            }
        } else if c == 'r'
            && (next == Some('"') || next == Some('#'))
            && (i == 0 || !ident(chars[i - 1]) || chars[i - 1] == 'b')
        {
            let mut j = i + 1;
            let mut hashes = 0;
            while chars.get(j) == Some(&'#') {
                hashes += 1;
                j += 1;
            }
            if chars.get(j) != Some(&'"') {
                i += 1;
                continue;
            }
            j += 1;
            while j < chars.len() {
                if chars[j] == '"' && (1..=hashes).all(|k| chars.get(j + k) == Some(&'#')) {
                    break;
                }
                blank(&mut code, j);
                j += 1;
            }
            i = j + 1 + hashes;
        } else if c == '"' {
            i += 1;
            while i < chars.len() && chars[i] != '"' {
                if chars[i] == '\\' {
                    blank(&mut code, i);
                    i += 1;
                }
                blank(&mut code, i);
                i += 1;
            }
            i += 1;
        } else if c == '\'' {
            let close = if next == Some('\\') {
                // The escaped character is at i + 2, so the closing quote is at i + 3 or later.
                (i + 3..chars.len()).find(|&j| chars[j] == '\'')
            } else if chars.get(i + 2) == Some(&'\'') {
                Some(i + 2)
            } else {
                None
            };
            match close {
                Some(end) => {
                    for j in i + 1..end {
                        blank(&mut code, j);
                    }
                    i = end + 1;
                }
                None => i += 1,
            }
        } else {
            i += 1;
        }
    }
    (code, kept)
}

fn normalize(text: &str) -> String {
    let mut out = text.split_whitespace().collect::<Vec<_>>().join(" ");
    for (from, to) in [
        ("( ", "("),
        (" )", ")"),
        ("[ ", "["),
        (" ]", "]"),
        ("{ ", "{"),
        (" }", "}"),
        (",)", ")"),
        (",]", "]"),
        (",}", "}"),
        (",>", ">"),
    ] {
        while out.contains(from) {
            out = out.replace(from, to);
        }
    }
    out
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Mod,
    Impl,
    Trait,
    Enum,
    Struct,
    Variant,
    Fn,
    Use,
    Const,
    Type,
    Field,
    Other,
}

struct Frame {
    kind: Kind,
    label: String,
    public: bool,
    /// Skipped wholesale: a function body or a `#[cfg(test)]` module.
    skip: bool,
    depth: i64,
}

/// What a statement is, and whether it is plain `pub`.
fn classify(statement: &str, container: Kind) -> (Kind, bool, String) {
    let mut rest = statement.trim_start();
    let mut public = false;
    if let Some(after) = rest.strip_prefix("pub(") {
        rest = after
            .split_once(')')
            .map_or("", |(_, tail)| tail)
            .trim_start();
    } else if let Some(after) = rest.strip_prefix("pub ") {
        public = true;
        rest = after.trim_start();
    }
    loop {
        let before = rest;
        for qualifier in ["async ", "unsafe ", "default ", "extern \"C\" "] {
            if let Some(after) = rest.strip_prefix(qualifier) {
                rest = after.trim_start();
            }
        }
        if rest.starts_with("const fn ") {
            rest = &rest["const ".len()..];
        }
        if rest == before {
            break;
        }
    }
    let word: String = rest
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '!')
        .collect();
    let name = || {
        rest[word.len()..]
            .trim_start()
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect::<String>()
    };
    let kind = match word.as_str() {
        "fn" => Kind::Fn,
        "impl" => Kind::Impl,
        "trait" => Kind::Trait,
        "struct" => Kind::Struct,
        "enum" => Kind::Enum,
        "mod" => Kind::Mod,
        "use" => Kind::Use,
        "const" | "static" => Kind::Const,
        "type" => Kind::Type,
        _ if container == Kind::Enum => Kind::Variant,
        _ if container == Kind::Struct => Kind::Field,
        _ => Kind::Other,
    };
    let label = match kind {
        Kind::Variant => word,
        Kind::Impl => String::new(),
        _ => name(),
    };
    (kind, public, label)
}

/// The text before the first `{` outside parentheses, by the masked code's positions.
fn before_body(code: &[char], kept: &[char]) -> String {
    let mut depth = 0;
    for (index, c) in code.iter().enumerate() {
        match c {
            '(' | '[' => depth += 1,
            ')' | ']' => depth -= 1,
            '{' if depth == 0 => return kept[..index].iter().collect(),
            _ => {}
        }
    }
    kept.iter().collect()
}

/// Every public item of one file, in source order, as `<file>: <context> > <item>`.
#[allow(clippy::too_many_lines)] // One pass over the lines keeps the brace depth in one place.
fn scan(file: &str, source: &str) -> Vec<(Vec<String>, String)> {
    let (code, kept) = mask(source);
    let lines = |chars: &[char]| -> Vec<Vec<char>> {
        chars.split(|c| *c == '\n').map(<[char]>::to_vec).collect()
    };
    let (code_lines, kept_lines) = (lines(&code), lines(&kept));
    let mut stack = vec![Frame {
        kind: Kind::Mod,
        label: String::new(),
        public: true,
        skip: false,
        depth: 0,
    }];
    let mut depth: i64 = 0;
    let mut out = Vec::new();
    let mut attributes: Vec<String> = Vec::new();
    let mut attribute: Option<(Vec<char>, Vec<char>)> = None;
    let mut statement: Option<(Vec<char>, Vec<char>)> = None;
    for (code_line, kept_line) in code_lines.iter().zip(&kept_lines) {
        let braces: i64 = code_line
            .iter()
            .map(|c| match c {
                '{' => 1,
                '}' => -1,
                _ => 0,
            })
            .sum();
        let trimmed: String = code_line.iter().collect::<String>().trim().to_owned();
        let top_skip = stack.last().is_some_and(|frame| frame.skip);
        if top_skip || (statement.is_none() && attribute.is_none() && trimmed.is_empty()) {
            depth += braces;
            while stack.last().is_some_and(|frame| depth < frame.depth) {
                stack.pop();
            }
            continue;
        }
        if statement.is_none() && (attribute.is_some() || trimmed.starts_with('#')) {
            let (code_text, kept_text) = attribute.get_or_insert_with(Default::default);
            code_text.extend(code_line.iter().chain(['\n'].iter()));
            kept_text.extend(kept_line.iter().chain(['\n'].iter()));
            let balance = code_text
                .iter()
                .map(|c| match c {
                    '[' | '(' => 1,
                    ']' | ')' => -1,
                    _ => 0,
                })
                .sum::<i64>();
            if balance == 0 {
                let (_, kept_text) = attribute.take().expect("attribute");
                let text = normalize(&kept_text.iter().collect::<String>());
                if !text.starts_with("#!") {
                    attributes.push(text);
                }
            }
            continue;
        }
        if statement.is_none() && trimmed.starts_with('}') {
            depth += braces;
            while stack.last().is_some_and(|frame| depth < frame.depth) {
                stack.pop();
            }
            attributes.clear();
            continue;
        }
        let (code_text, kept_text) = statement.get_or_insert_with(Default::default);
        code_text.extend(code_line.iter().chain(['\n'].iter()));
        kept_text.extend(kept_line.iter().chain(['\n'].iter()));
        depth += braces;
        let paren: i64 = code_text
            .iter()
            .map(|c| match c {
                '(' | '[' => 1,
                ')' | ']' => -1,
                _ => 0,
            })
            .sum();
        let brace: i64 = code_text
            .iter()
            .map(|c| match c {
                '{' => 1,
                '}' => -1,
                _ => 0,
            })
            .sum();
        let container = stack.last().expect("root frame");
        let (kind, public, label) = classify(&code_text.iter().collect::<String>(), container.kind);
        let last = trimmed.chars().last();
        let header = matches!(
            kind,
            Kind::Fn | Kind::Impl | Kind::Trait | Kind::Struct | Kind::Enum | Kind::Mod
        );
        // A variant is folded into one record whatever its layout, so it never opens a frame.
        let opens_frame = paren == 0 && brace > 0 && last == Some('{') && header;
        let complete = opens_frame
            || (paren == 0
                && brace == 0
                && match kind {
                    Kind::Use | Kind::Const | Kind::Type => last == Some(';'),
                    _ if header => matches!(last, Some(';' | '}')),
                    _ => matches!(last, Some(';' | ',' | '}')),
                });
        if !complete {
            continue;
        }
        let (code_text, kept_text) = statement.take().expect("statement");
        let enclosing_public = container.public;
        let container_kind = container.kind;
        let signature = if kind == Kind::Fn || opens_frame {
            before_body(&code_text, &kept_text)
        } else {
            kept_text.iter().collect()
        };
        let signature = normalize(&signature);
        let record = match kind {
            Kind::Variant => enclosing_public,
            Kind::Fn | Kind::Const | Kind::Type if container_kind == Kind::Trait => {
                enclosing_public
            }
            Kind::Impl => signature.contains(" for "),
            _ => public,
        };
        if record {
            let kept_attributes: Vec<&String> = attributes
                .iter()
                .filter(|attribute| {
                    KEPT_ATTRIBUTES
                        .iter()
                        .any(|kept| attribute.starts_with(kept))
                })
                .collect();
            let mut text = String::new();
            for attribute in kept_attributes {
                text.push_str(attribute);
                text.push(' ');
            }
            text.push_str(&signature);
            let context = stack[1..]
                .iter()
                .map(|frame| frame.label.clone())
                .collect::<Vec<_>>();
            out.push((context, text));
        }
        if opens_frame {
            let test_module = kind == Kind::Mod
                && attributes
                    .iter()
                    .any(|attribute| attribute == "#[cfg(test)]");
            let label = match kind {
                Kind::Mod => format!("mod {label}"),
                Kind::Impl => signature.clone(),
                Kind::Trait => format!("trait {label}"),
                Kind::Enum => format!("enum {label}"),
                Kind::Struct => format!("struct {label}"),
                _ => String::new(),
            };
            stack.push(Frame {
                kind,
                label,
                public: match kind {
                    Kind::Mod | Kind::Impl => true,
                    _ => public,
                },
                skip: kind == Kind::Fn || test_module,
                depth,
            });
        }
        while stack.last().is_some_and(|frame| depth < frame.depth) {
            stack.pop();
        }
        attributes.clear();
    }
    assert_eq!(depth, 0, "{file}: the scanner lost track of the braces");
    assert!(statement.is_none(), "{file}: a statement never ended");
    out
}

/// The names `mod.rs` re-exports from the generated types.
fn reexported(module: &str) -> Vec<String> {
    let (_, list) = module
        .split_once("pub use crate::confinement_types::{")
        .expect("confinement re-exports the generated types");
    let (list, _) = list.split_once('}').expect("closed re-export list");
    list.split(',')
        .map(|name| name.trim().to_owned())
        .filter(|name| !name.is_empty())
        .collect()
}

fn rust_files(directory: &Path, into: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(directory).expect("source directory") {
        let path = entry.expect("entry").path();
        if path.is_dir() {
            rust_files(&path, into);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            into.push(path);
        }
    }
}

/// The module's sources: `(path beneath src/, text)`, sorted, the generated file last.
fn sources() -> Vec<(String, String)> {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rust_files(&src.join("confinement"), &mut files);
    files.sort();
    files.push(src.join(GENERATED));
    files
        .into_iter()
        .map(|path| {
            let name = path
                .strip_prefix(&src)
                .expect("beneath src")
                .to_str()
                .expect("UTF-8 path")
                .to_owned();
            let text = std::fs::read_to_string(&path).expect("source");
            (name, text)
        })
        .collect()
}

fn items(sources: &[(String, String)]) -> Vec<String> {
    let module = &sources
        .iter()
        .find(|(name, _)| name == "confinement/mod.rs")
        .expect("confinement/mod.rs")
        .1;
    let generated: Vec<String> = reexported(module)
        .iter()
        .map(|name| format!("enum {name}"))
        .collect();
    let mut out = Vec::new();
    for (name, text) in sources {
        for (context, item) in scan(name, text) {
            if name == GENERATED {
                let top = context.first().map_or(item.as_str(), String::as_str);
                if !generated
                    .iter()
                    .any(|enumeration| top.contains(enumeration.as_str()))
                {
                    continue;
                }
            }
            let mut line = format!("{name}: ");
            for frame in &context {
                line.push_str(frame);
                line.push_str(" > ");
            }
            line.push_str(&item);
            out.push(line);
        }
    }
    out
}

fn committed() -> Option<Vec<String>> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(LIST);
    let text = std::fs::read_to_string(path).ok()?;
    Some(
        text.lines()
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .map(str::to_owned)
            .collect(),
    )
}

fn render(items: &[String]) -> String {
    let mut text = String::from(
        "# The public items of substrate_host::confinement, checked by\n\
         # tests/confinement_public_items.rs. Regenerate with\n\
         # SUBSTRATE_BLESS_PUBLIC_ITEMS=1 cargo test -p b10x-substrate-host --test confinement_public_items\n",
    );
    for item in items {
        text.push_str(item);
        text.push('\n');
    }
    text
}

/// Lines only in `left`.
fn missing(left: &[String], right: &[String]) -> Vec<String> {
    left.iter()
        .filter(|line| !right.contains(line))
        .cloned()
        .collect()
}

#[test]
fn the_public_items_of_the_confinement_module_are_the_committed_list() {
    let found = items(&sources());
    assert!(
        found.len() > 100,
        "the scanner found only {} items",
        found.len()
    );
    if std::env::var_os(BLESS).is_some_and(|value| value == "1") {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(LIST);
        std::fs::write(&path, render(&found)).expect("write the list");
        println!(
            "confinement public items: {} written to {LIST}",
            found.len()
        );
        return;
    }
    let listed = committed()
        .unwrap_or_else(|| panic!("{LIST} is missing; write it with {BLESS}=1 and review it"));
    let added = missing(&found, &listed);
    let removed = missing(&listed, &found);
    assert!(
        added.is_empty() && removed.is_empty() && found == listed,
        "the confinement module's public items differ from {LIST}.\n\
         In the source and not in the list:\n  {}\n\
         In the list and not in the source:\n  {}\n\
         Update the list with {BLESS}=1 in the same commit as the change.",
        added.join("\n  "),
        removed.join("\n  ")
    );
    println!(
        "confinement public items: {} checked against {LIST}",
        found.len()
    );
}

/// The scanner sees a rename, an addition, a removal and a changed signature, and does not see a
/// reformat: each edit is applied to the real `mod.rs` in memory and the lists compared.
#[test]
fn the_list_changes_with_a_renamed_added_or_changed_item_and_not_with_a_reformat() {
    let original = sources();
    let base = items(&original);
    let edited = |from: &str, to: &str| {
        let mut sources = original.clone();
        let module = sources
            .iter_mut()
            .find(|(name, _)| name == "confinement/mod.rs")
            .expect("mod.rs");
        assert!(module.1.contains(from), "mod.rs no longer holds {from:?}");
        module.1 = module.1.replacen(from, to, 1);
        let changed = items(&sources);
        (missing(&changed, &base), missing(&base, &changed))
    };

    let (added, removed) = edited("pub fn check_open(", "pub fn check_opened(");
    assert_eq!(added.len(), 1, "{added:?}");
    assert!(added[0].contains("pub fn check_opened("), "{added:?}");
    assert_eq!(removed.len(), 1, "{removed:?}");

    let (added, removed) = edited(
        "    ReservedFieldUnserved { field: ReservedField },\n",
        "    ReservedFieldUnserved { field: ReservedField },\n    Renamed,\n",
    );
    assert_eq!(
        added,
        ["confinement/mod.rs: enum ConfinementError > Renamed,"]
    );
    assert!(removed.is_empty(), "{removed:?}");

    let (added, removed) = edited(
        "    pub fn as_str(&self) -> &str {\n        &self.0\n    }\n}\n\n/// A profile",
        "}\n\n/// A profile",
    );
    assert!(added.is_empty(), "{added:?}");
    assert_eq!(
        removed,
        ["confinement/mod.rs: impl ProfileId > pub fn as_str(&self) -> &str"]
    );

    let (added, removed) = edited(
        "pub fn admit_unopened(adapter: &AdapterName, profile: &ConfinementProfile)",
        "pub fn admit_unopened(adapter: AdapterName, profile: &ConfinementProfile)",
    );
    assert_eq!(added.len(), 1, "{added:?}");
    assert_eq!(removed.len(), 1, "{removed:?}");

    let (added, removed) = edited(
        "    async fn wait(&self, run: &RunId)",
        "    async fn wait(&self, run: &RunId, again: bool)",
    );
    assert_eq!(added.len(), 1, "a trait method's signature: {added:?}");
    assert_eq!(removed.len(), 1, "{removed:?}");

    let (added, removed) = edited(
        "pub fn admit_unopened(adapter: &AdapterName, profile: &ConfinementProfile)",
        "pub fn admit_unopened(\n    adapter: &AdapterName,\n    profile: &ConfinementProfile,\n)",
    );
    assert!(
        added.is_empty() && removed.is_empty(),
        "a reformat: {added:?} {removed:?}"
    );

    let (added, removed) = edited(
        "    FactUnserved { adapter: AdapterName, fact: Fact },\n",
        "    FactUnserved {\n        adapter: AdapterName,\n        fact: Fact,\n    },\n",
    );
    assert!(
        added.is_empty() && removed.is_empty(),
        "a variant reformatted: {added:?} {removed:?}"
    );

    let (added, removed) = edited(
        "    FactUnserved { adapter: AdapterName, fact: Fact },\n",
        "    FactUnserved { adapter: AdapterName, fact: Option<Fact> },\n",
    );
    assert_eq!(added.len(), 1, "a variant's field type: {added:?}");
    assert_eq!(removed.len(), 1, "{removed:?}");
}
