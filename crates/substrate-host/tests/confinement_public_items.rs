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
//! with no dependency beyond the standard library. It records, per file of `src/confinement/`:
//!
//! - every `pub` item and `pub` field, the signature without its body, a constant with its value;
//! - one `(private fields)` line for a `pub` struct with any field that is not `pub`, so the first
//!   such field, which stops a consumer from writing the struct as a literal, changes the list;
//! - every variant of a `pub` enum, with its fields;
//! - every method, constant and type of a `pub` trait, without a default body;
//! - every trait implementation's header;
//! - the `derive`, `serde`, `cfg`, `repr`, `non_exhaustive` and `async_trait` attributes of each.
//!
//! Every name the module re-exports from outside `src/confinement/` (`pub use crate::…` and
//! `pub use substrate_wire::…`) is recorded the same way: its definition, its fields or variants,
//! and every `impl` block for it in its own crate. Each such name must resolve to exactly one
//! `pub` definition, and a re-export from any other crate fails the test until the scanner reads
//! that crate too.
//!
//! It reads every `cfg` arm, so the list is the same on every OS. It also records `pub` items of a
//! private module, which a consumer cannot reach: over-inclusive, so a change there is flagged
//! rather than missed. `#[cfg(test)]` modules and function bodies are skipped. Whitespace and line
//! breaks are normalized, so reformatting a signature changes nothing.

use std::path::{Path, PathBuf};

const LIST: &str = "tests/confinement-public-items.txt";
const BLESS: &str = "SUBSTRATE_BLESS_PUBLIC_ITEMS";
const MODULE: &str = "confinement/";
/// The source name prefix of `substrate-wire`'s files; the host crate's are beneath its `src/`.
const WIRE: &str = "substrate-wire/src/";
/// Marks a `pub` struct with a field that is not `pub`.
const PRIVATE_FIELDS: &str = "(private fields)";
/// Marks a re-exported name without exactly one `pub` definition in its crate.
const UNRESOLVED: &str = "(unresolved re-export)";
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
    /// The type a struct, enum or trait defines, or an `impl` is for.
    subject: Option<String>,
    /// A struct frame whose `(private fields)` line is written.
    private_marked: bool,
}

/// One recorded item: the frames it sits in, its text, and the type it belongs to.
struct Record {
    context: Vec<String>,
    text: String,
    subject: String,
}

/// The type an `impl` header is for, by its last path segment: `Signal` for
/// `impl<'de> Deserialize<'de> for Signal` and for `impl Signal`.
fn self_type(signature: &str) -> String {
    let mut rest = signature.trim_start();
    rest = rest.strip_prefix("unsafe ").unwrap_or(rest).trim_start();
    rest = rest.strip_prefix("impl").unwrap_or(rest).trim_start();
    if rest.starts_with('<') {
        let mut depth = 0;
        for (index, c) in rest.char_indices() {
            match c {
                '<' => depth += 1,
                '>' => depth -= 1,
                _ => {}
            }
            if depth == 0 {
                rest = &rest[index + 1..];
                break;
            }
        }
    }
    if let Some(index) = rest.rfind(" for ") {
        rest = &rest[index + " for ".len()..];
    }
    let path: String = rest
        .trim_start()
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == ':')
        .collect();
    path.rsplit("::").next().unwrap_or_default().to_owned()
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
fn scan(file: &str, source: &str) -> Vec<Record> {
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
        subject: None,
        private_marked: false,
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
        let own_subject = match kind {
            Kind::Impl => self_type(&signature),
            _ => label.clone(),
        };
        let subject = stack[1..]
            .iter()
            .rev()
            .find_map(|frame| frame.subject.clone())
            .unwrap_or_else(|| own_subject.clone());
        if kind == Kind::Field && !public && enclosing_public && container_kind == Kind::Struct {
            let context = stack[1..]
                .iter()
                .map(|frame| frame.label.clone())
                .collect::<Vec<_>>();
            let frame = stack.last_mut().expect("the struct's frame");
            if !frame.private_marked {
                frame.private_marked = true;
                out.push(Record {
                    context,
                    text: PRIVATE_FIELDS.to_owned(),
                    subject: subject.clone(),
                });
            }
        }
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
            out.push(Record {
                context,
                text,
                subject,
            });
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
                subject: matches!(kind, Kind::Struct | Kind::Enum | Kind::Trait | Kind::Impl)
                    .then_some(own_subject),
                private_marked: false,
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

/// Where the definitions a `pub use` names live.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Origin {
    /// Inside `src/confinement/`, so they are scanned with the module.
    Module,
    /// The host crate outside `src/confinement/`.
    Host,
    /// `substrate-wire`.
    Wire,
}

/// The masked code of `text`: comments and literal contents blanked.
fn code_of(text: &str) -> String {
    mask(text).0.into_iter().collect()
}

/// Whether `text` starts with `word` followed by a character that cannot continue an identifier.
fn starts_with_word(text: &str, word: &str) -> bool {
    text.strip_prefix(word).is_some_and(|rest| {
        !rest
            .chars()
            .next()
            .is_some_and(|c| c.is_alphanumeric() || c == '_')
    })
}

/// Whether `text` holds `word` delimited by characters that cannot continue an identifier.
fn has_word(text: &str, word: &str) -> bool {
    let ident = |c: char| c.is_alphanumeric() || c == '_';
    text.match_indices(word).any(|(index, _)| {
        !text[..index].chars().next_back().is_some_and(ident)
            && !text[index + word.len()..].chars().next().is_some_and(ident)
    })
}

/// Strips a leading `pub` or `pub(…)`.
fn without_visibility(line: &str) -> &str {
    let line = line.trim_start();
    if let Some(rest) = line.strip_prefix("pub(") {
        return rest
            .split_once(')')
            .map_or("", |(_, tail)| tail)
            .trim_start();
    }
    line.strip_prefix("pub ").unwrap_or(line).trim_start()
}

/// Whether `text` declares a module named `name`, inline or in its own file.
fn declares_module(text: &str, name: &str) -> bool {
    code_of(text).lines().any(|line| {
        without_visibility(line)
            .strip_prefix("mod ")
            .is_some_and(|rest| starts_with_word(rest.trim_start(), name))
    })
}

/// Whether one line of masked code is the `pub` definition of `name`.
fn defines(line: &str, name: &str) -> bool {
    let line = line.trim_start();
    [
        "pub struct ",
        "pub enum ",
        "pub trait ",
        "pub type ",
        "pub union ",
        "pub fn ",
        "pub const fn ",
        "pub async fn ",
        "pub const ",
        "pub static ",
    ]
    .iter()
    .any(|keyword| {
        line.strip_prefix(keyword)
            .is_some_and(|rest| starts_with_word(rest, name))
    })
}

/// What one `pub use` record re-exports: `pub use substrate_wire::{A, B as C};` is `(Wire, [A, B])`,
/// each name as its source crate defines it.
fn reexport(text: &str, module: &[&(String, String)]) -> (Origin, Vec<String>) {
    let (_, path) = text.split_once("pub use ").expect("a pub use");
    let path = path.trim_end_matches(';').trim();
    assert!(
        !path.contains('*'),
        "{text}: a glob re-export is not read by the scanner"
    );
    let (prefix, names): (&str, Vec<&str>) = if let Some((prefix, list)) = path.split_once("::{") {
        let list = list
            .strip_suffix('}')
            .unwrap_or_else(|| panic!("{text}: an unclosed re-export list"));
        assert!(
            !list.contains('{'),
            "{text}: a nested re-export list is not read by the scanner"
        );
        (prefix, list.split(',').collect())
    } else {
        let (prefix, name) = path
            .rsplit_once("::")
            .unwrap_or_else(|| panic!("{text}: a re-export without a path"));
        (prefix, vec![name])
    };
    let names = names
        .into_iter()
        .map(|name| name.split(" as ").next().unwrap_or_default().trim())
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .collect();
    let segments: Vec<&str> = prefix.split("::").collect();
    let origin = match segments[0] {
        "crate" if segments.get(1) == Some(&"confinement") => Origin::Module,
        "crate" => Origin::Host,
        "substrate_wire" => Origin::Wire,
        "self" | "super" => Origin::Module,
        first
            if module
                .iter()
                .any(|(_, source)| declares_module(source, first)) =>
        {
            Origin::Module
        }
        first => panic!(
            "{text}: a re-export from {first}, whose definitions this scanner does not read; \
             extend sources() and Origin to that crate"
        ),
    };
    (origin, names)
}

/// Where `name` has a `pub` definition among `files`, as `file:line`.
fn definitions(files: &[&(String, String)], codes: &[Option<String>], name: &str) -> Vec<String> {
    files
        .iter()
        .zip(codes)
        .filter_map(|((file, _), code)| code.as_ref().map(|code| (file, code)))
        .flat_map(|(file, code)| {
            code.lines()
                .enumerate()
                .filter(|(_, line)| defines(line, name))
                .map(move |(index, _)| format!("{file}:{}", index + 1))
        })
        .collect()
}

/// The masked code of each file whose text holds any of `names`; a file that does not cannot
/// define one or name it in an `impl`, so it is not masked.
fn codes_naming(files: &[&(String, String)], names: &[String]) -> Vec<Option<String>> {
    files
        .iter()
        .map(|(_, text)| {
            names
                .iter()
                .any(|name| has_word(text, name))
                .then(|| code_of(text))
        })
        .collect()
}

/// The lines of `files` that belong to `names`: each name's one `pub` definition with its fields
/// or variants, and every `impl` for it. A name without exactly one definition is one
/// [`UNRESOLVED`] line, which the committed-list test refuses.
fn external(files: &[&(String, String)], names: &[String]) -> Vec<String> {
    let codes = codes_naming(files, names);
    let mut scanned = vec![false; files.len()];
    let mut out = Vec::new();
    for name in names {
        let found = definitions(files, &codes, name);
        if found.len() != 1 {
            out.push(format!(
                "{UNRESOLVED} {name}: {} pub definitions {found:?}",
                found.len()
            ));
            continue;
        }
        for (index, code) in codes.iter().enumerate() {
            if code.as_ref().is_some_and(|code| {
                code.lines().any(|line| {
                    defines(line, name) || (has_word(line, "impl") && has_word(line, name))
                })
            }) {
                scanned[index] = true;
            }
        }
    }
    for (index, (file, text)) in files.iter().enumerate() {
        if !scanned[index] {
            continue;
        }
        out.extend(
            scan(file, text)
                .iter()
                .filter(|record| names.contains(&record.subject))
                .map(|record| line(file, record)),
        );
    }
    out
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

/// Every source the list reads, as `(name, text)`: the module's files beneath `src/confinement/`,
/// then the host crate's other files beneath `src/`, then `substrate-wire`'s beneath [`WIRE`],
/// each group sorted.
fn sources() -> Vec<(String, String)> {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let src = manifest.join("src");
    let wire = manifest.join("..").join("substrate-wire").join("src");
    let mut host = Vec::new();
    rust_files(&src, &mut host);
    host.sort();
    let (mut ordered, rest): (Vec<PathBuf>, Vec<PathBuf>) = host
        .into_iter()
        .partition(|path| path.starts_with(src.join("confinement")));
    ordered.extend(rest);
    let mut wire_files = Vec::new();
    rust_files(&wire, &mut wire_files);
    wire_files.sort();
    let name = |path: &Path, base: &Path, prefix: &str| {
        let beneath = path
            .strip_prefix(base)
            .expect("beneath its src")
            .to_str()
            .expect("UTF-8 path");
        format!("{prefix}{beneath}")
    };
    ordered
        .iter()
        .map(|path| name(path, &src, ""))
        .chain(wire_files.iter().map(|path| name(path, &wire, WIRE)))
        .zip(ordered.iter().chain(&wire_files))
        .map(|(name, path)| (name, std::fs::read_to_string(path).expect("source")))
        .collect()
}

fn line(file: &str, record: &Record) -> String {
    let mut line = format!("{file}: ");
    for frame in &record.context {
        line.push_str(frame);
        line.push_str(" > ");
    }
    line.push_str(&record.text);
    line
}

fn items(sources: &[(String, String)]) -> Vec<String> {
    let module: Vec<&(String, String)> = sources
        .iter()
        .filter(|(name, _)| name.starts_with(MODULE))
        .collect();
    let mut out = Vec::new();
    let mut wanted: Vec<(Origin, String)> = Vec::new();
    for (name, text) in &module {
        for record in scan(name, text) {
            if record.text.starts_with("pub use ") || record.text.contains("] pub use ") {
                let (origin, names) = reexport(&record.text, &module);
                for reexported in names {
                    if origin != Origin::Module && !wanted.contains(&(origin, reexported.clone())) {
                        wanted.push((origin, reexported));
                    }
                }
            }
            out.push(line(name, &record));
        }
    }
    // `crate::X` for a wire type the crate root itself re-exports names the wire's definition.
    let root = sources
        .iter()
        .find(|(name, _)| name == "lib.rs")
        .expect("the host crate's lib.rs");
    let host_files: Vec<&(String, String)> = sources
        .iter()
        .filter(|(name, _)| !name.starts_with(MODULE) && !name.starts_with(WIRE))
        .collect();
    let host_names: Vec<String> = wanted
        .iter()
        .filter(|(from, _)| *from == Origin::Host)
        .map(|(_, name)| name.clone())
        .collect();
    let host_codes = codes_naming(&host_files, &host_names);
    let root_reexports: Vec<(Origin, Vec<String>)> = scan(&root.0, &root.1)
        .iter()
        .filter(|record| record.context.is_empty() && record.text.contains("pub use "))
        .map(|record| reexport(&record.text, &[root]))
        .collect();
    for (origin, name) in &mut wanted {
        if *origin == Origin::Host
            && definitions(&host_files, &host_codes, name).is_empty()
            && root_reexports
                .iter()
                .any(|(from, names)| *from == Origin::Wire && names.contains(name))
        {
            *origin = Origin::Wire;
        }
    }
    for origin in [Origin::Host, Origin::Wire] {
        let names: Vec<String> = wanted
            .iter()
            .filter(|(from, _)| *from == origin)
            .map(|(_, name)| name.clone())
            .collect();
        if names.is_empty() {
            continue;
        }
        let files: Vec<&(String, String)> = sources
            .iter()
            .filter(|(name, _)| {
                !name.starts_with(MODULE) && name.starts_with(WIRE) == (origin == Origin::Wire)
            })
            .collect();
        out.extend(external(&files, &names));
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
    let unresolved: Vec<&String> = found
        .iter()
        .filter(|line| line.starts_with(UNRESOLVED))
        .collect();
    assert!(
        unresolved.is_empty(),
        "confinement re-exports names whose definition the scanner cannot read: {unresolved:?}"
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

/// A re-exported type is held by its definition in its own crate: a method added to a wire type and
/// a first private field added to the host's configuration change the list, a second private field
/// does not, a wire type re-exported through the crate root still resolves to the wire, and a
/// re-export with no definition is a named line the committed-list test refuses.
#[test]
fn a_reexported_type_is_held_by_its_definition_in_its_own_crate() {
    let original = sources();
    let base = items(&original);
    let edited = |edits: &[(&str, &str, &str)]| {
        let mut sources = original.clone();
        for (file, from, to) in edits {
            let source = sources
                .iter_mut()
                .find(|(name, _)| name == file)
                .unwrap_or_else(|| panic!("no source {file}"));
            assert!(source.1.contains(from), "{file} no longer holds {from:?}");
            source.1 = source.1.replacen(from, to, 1);
        }
        let changed = items(&sources);
        (missing(&changed, &base), missing(&base, &changed))
    };
    let wire = format!("{WIRE}lib.rs");

    let (added, removed) = edited(&[(
        &wire,
        "impl Signal {\n",
        "impl Signal {\n    pub fn name(self) -> &'static str {\n        \"\"\n    }\n",
    )]);
    assert_eq!(
        added,
        ["substrate-wire/src/lib.rs: impl Signal > pub fn name(self) -> &'static str"]
    );
    assert!(removed.is_empty(), "{removed:?}");

    let last_field = "    pub writable_roots: Vec<DeclaredWritableRoot>,\n}";
    let (added, removed) = edited(&[(
        "lib.rs",
        last_field,
        "    pub writable_roots: Vec<DeclaredWritableRoot>,\n    reserved: u64,\n}",
    )]);
    assert_eq!(added, ["lib.rs: struct HostConfig > (private fields)"]);
    assert!(removed.is_empty(), "{removed:?}");
    let (added, removed) = edited(&[(
        "lib.rs",
        last_field,
        "    pub writable_roots: Vec<DeclaredWritableRoot>,\n    reserved: u64,\n    more: u64,\n}",
    )]);
    assert_eq!(added, ["lib.rs: struct HostConfig > (private fields)"]);
    assert!(removed.is_empty(), "{removed:?}");

    let wire_use = "pub use substrate_wire::{ReadOnlyFile, ReadOnlyRoot, SecretSlotRequest, Signal, WritableHostRoot};";
    let through_root = "pub use substrate_wire::{ReadOnlyFile, ReadOnlyRoot, SecretSlotRequest, WritableHostRoot};\npub use crate::Signal;";
    let (added, removed) = edited(&[
        ("confinement/mod.rs", wire_use, through_root),
        (
            "lib.rs",
            "pub use unsupported::HostDriver;",
            "pub use unsupported::HostDriver;\npub use substrate_wire::Signal;",
        ),
    ]);
    assert_eq!(added.len(), 2, "{added:?}");
    assert!(
        added
            .iter()
            .all(|line| line.starts_with("confinement/mod.rs: pub use ")),
        "the wire's Signal lines still stand: {added:?}"
    );
    assert_eq!(removed.len(), 1, "{removed:?}");

    let (added, _) = edited(&[("confinement/mod.rs", wire_use, through_root)]);
    assert!(
        added
            .iter()
            .any(|line| line.starts_with(&format!("{UNRESOLVED} Signal: 0 pub definitions"))),
        "{added:?}"
    );
}

/// Adversary probes (`story:confinement-consumer-surface`): each applies one consumer-breaking edit
/// to the real sources in memory and requires the list to change. Every edit is a compile-visible
/// change for a crate that builds against the module.
fn adversary_edits(edits: &[(&str, &str, &str)]) -> (Vec<String>, Vec<String>) {
    let original = sources();
    let base = items(&original);
    let mut changed = original.clone();
    for (file, from, to) in edits {
        let source = changed
            .iter_mut()
            .find(|(name, _)| name == file)
            .unwrap_or_else(|| panic!("no source {file}"));
        assert!(source.1.contains(from), "{file} no longer holds {from:?}");
        source.1 = source.1.replacen(from, to, 1);
    }
    let after = items(&changed);
    (missing(&after, &base), missing(&base, &after))
}

fn adversary_assert_changed(what: &str, edits: &[(&str, &str, &str)]) {
    let (added, removed) = adversary_edits(edits);
    assert!(
        !added.is_empty() || !removed.is_empty(),
        "{what}: the list did not change"
    );
}

/// A private field added to a struct whose fields are all `pub` makes every struct literal and
/// every `..Default::default()` update outside the crate fail to compile (E0451), which is how
/// `consumer_surface.rs` builds each profile and bound.
#[test]
fn adversary_a_private_field_added_to_an_all_public_struct_changes_the_list() {
    adversary_assert_changed(
        "a private field added to IoBounds",
        &[(
            "confinement/profile.rs",
            "pub struct IoBounds {\n    pub output_bytes: u64,\n}",
            "pub struct IoBounds {\n    pub output_bytes: u64,\n    reserved: u64,\n}",
        )],
    );
}

/// `confinement` re-exports `HostConfig`, `DeclaredWritableRoot` and five wire types so a consumer
/// builds every value from the module alone (commit 903ba141), and `consumer_surface.rs` builds
/// them. A public item of the module whose fields or methods are not in the list can change
/// without the list.
#[test]
fn adversary_the_list_records_the_shape_of_every_type_the_module_reexports() {
    let found = items(&sources());
    let absent: Vec<&str> = [
        "struct HostConfig",
        "fn minimum(",
        "struct DeclaredWritableRoot",
        "struct ReadOnlyRoot",
        "struct WritableHostRoot",
        "struct ReadOnlyFile",
        "struct SecretSlotRequest",
        "enum Signal",
    ]
    .into_iter()
    .filter(|shape| !found.iter().any(|line| line.contains(shape)))
    .collect();
    assert!(
        absent.is_empty(),
        "re-exported by confinement, with no record of their shape in the list: {absent:?}"
    );
}

#[test]
fn adversary_a_dropped_bound_on_an_impl_trait_argument_changes_the_list() {
    adversary_assert_changed(
        "AuditSink::new loses Sync",
        &[(
            "confinement/mod.rs",
            "pub fn new(sink: impl Fn(&AuditRecord) + Send + Sync + 'static) -> Self {",
            "pub fn new(sink: impl Fn(&AuditRecord) + Send + 'static) -> Self {",
        )],
    );
}

#[test]
fn adversary_a_dropped_supertrait_bound_changes_the_list() {
    adversary_assert_changed(
        "ConfinementAdapter loses Sync",
        &[(
            "confinement/mod.rs",
            "pub trait ConfinementAdapter: Send + Sync {",
            "pub trait ConfinementAdapter: Send {",
        )],
    );
}

#[test]
fn adversary_a_removed_non_exhaustive_changes_the_list() {
    adversary_assert_changed(
        "MacosRequest loses non_exhaustive",
        &[(
            "confinement/macos.rs",
            "#[non_exhaustive]\npub struct MacosRequest {}",
            "pub struct MacosRequest {}",
        )],
    );
}

#[test]
fn adversary_an_item_moved_between_files_changes_the_list() {
    let item = "pub const S2_FACTS: [Fact; 3] = [Fact::SealedPaths, Fact::ExecSubpaths, Fact::PrivateTmpdir];\n";
    adversary_assert_changed(
        "S2_FACTS moved from macos.rs to mod.rs",
        &[
            ("confinement/macos.rs", item, ""),
            (
                "confinement/mod.rs",
                "pub const ADAPTER_UNSERVED",
                &format!("{item}pub const ADAPTER_UNSERVED"),
            ),
        ],
    );
}

#[test]
fn adversary_a_return_type_on_its_own_line_changes_the_list() {
    adversary_assert_changed(
        "render returns another type",
        &[(
            "confinement/mod.rs",
            "    ) -> Result<RenderedCommand, ConfinementError>;",
            "    ) -> Result<Command, ConfinementError>;",
        )],
    );
}

#[test]
fn adversary_a_trait_method_losing_its_default_body_changes_the_list() {
    adversary_assert_changed(
        "check becomes required",
        &[(
            "confinement/mod.rs",
            "    fn check(&self, _profile: &ConfinementProfile) -> Result<(), ConfinementError> {\n        Ok(())\n    }",
            "    fn check(&self, _profile: &ConfinementProfile) -> Result<(), ConfinementError>;",
        )],
    );
}

#[test]
fn adversary_a_new_variant_in_a_submodule_enum_changes_the_list() {
    adversary_assert_changed(
        "InnerAdapter gains a variant",
        &[(
            "confinement/facts.rs",
            "    Local,\n    Linux,\n}",
            "    Local,\n    Linux,\n    Macos,\n}",
        )],
    );
}

#[test]
fn adversary_a_reexport_retargeted_changes_the_list() {
    adversary_assert_changed(
        "Signal re-exported from another path",
        &[(
            "confinement/mod.rs",
            "pub use substrate_wire::{ReadOnlyFile, ReadOnlyRoot, SecretSlotRequest, Signal, WritableHostRoot};",
            "pub use substrate_wire::{ReadOnlyFile, ReadOnlyRoot, SecretSlotRequest, WritableHostRoot};\npub use crate::Signal;",
        )],
    );
}
