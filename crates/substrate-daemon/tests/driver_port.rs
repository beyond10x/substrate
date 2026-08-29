//! Structural proof of `AGENTS.md` invariant 4 — "Drivers implement one substrate contract and
//! expose verified capability facts. Clients do not branch on driver internals" — and of the
//! second entry criterion in `docs/plan/03-container-driver.md` § *Entry criteria*, "Driver ports
//! contain no host library types".
//!
//! The port trait is `substrate_host::Driver`, and it is declared inside the host crate itself
//! (`crates/substrate-host/src/lib.rs:171`). The criterion therefore cannot be read literally as
//! "no `substrate_host` name reaches the daemon": the port's own vocabulary — `DriverError`,
//! `DispatchOutcome`, `ExecObservation` — is declared in that same crate, and every caller of the
//! trait is forced to name it. The checkable claim that carries the invariant is narrower:
//!
//! > Outside the composition root, `substrate-daemon` names nothing from `substrate_host` except
//! > the port trait and the types the port trait's own signature forces on a caller.
//!
//! The host *implementation* — `HostDriver`, `HostConfig`, the bubblewrap and cgroup mechanics —
//! is nameable only where the driver is constructed. A second driver written against `Driver`
//! then cannot find that the daemon has already been shaped around the first one.
//!
//! Scope is `crates/substrate-daemon/src`. `crates/substrate-daemon/tests` is out of scope on
//! purpose: an integration test constructs the driver it runs against, so every file there is a
//! composition root by construction.

use std::fs;
use std::path::{Path, PathBuf};

/// Every crossing this test looks for. `::substrate_host::` contains it, so a leading path
/// qualifier is caught too.
const HOST_CRATE: &str = "substrate_host::";

/// Files allowed to name anything from `substrate_host`, each with the reason it is a
/// composition root. Two entries is one more than the invariant wants: `src/app/tests.rs` is an
/// in-crate test harness, not a second production wiring point.
const COMPOSITION_ROOTS: &[(&str, &str)] = &[
    (
        // The composition root: the only production code that picks a driver, at
        // `HostConfig::minimum` (src/runtime.rs:355) and `HostDriver::open` (src/runtime.rs:360).
        "src/runtime.rs",
        "the composition root: builds HostConfig and opens HostDriver (src/runtime.rs:355, :360)",
    ),
    (
        // The in-crate `#[cfg(test)]` harness declared at src/app.rs:28-29. It builds its own
        // HostDriver (src/app/tests.rs:8) and names DriverError (src/app/tests.rs:393), so it is
        // a test composition root rather than a client of the port.
        "src/app/tests.rs",
        "the in-crate #[cfg(test)] harness (src/app.rs:28) builds its own HostDriver",
    ),
];

/// Items of `substrate_host` that the port trait's signature forces on any caller, each with the
/// declaration that forces it. Nothing else may appear outside a composition root.
///
/// This is exactly what HEAD needs and no more. An item that falls out of use is a line to
/// delete; an item the port starts forcing is a line to add together with its citation.
/// `PipeFrame` (`crates/substrate-host/src/lib.rs:26`, returned by `Driver::read_pipe_session` at
/// `crates/substrate-host/src/lib.rs:325`) is forced by the port but is not named in
/// `substrate-daemon/src` at HEAD, so it is deliberately absent.
const PORT_ITEMS: &[(&str, &str)] = &[
    (
        "Driver",
        "the port trait itself (crates/substrate-host/src/lib.rs:171)",
    ),
    (
        "DispatchOutcome",
        "the port's dispatch result (crates/substrate-host/src/lib.rs:103), returned by \
         Driver::create_workspace (crates/substrate-host/src/lib.rs:186)",
    ),
    (
        "DriverError",
        "the port's error type (crates/substrate-host/src/lib.rs:95), in the Result of \
         Driver::workspace_root_identity (crates/substrate-host/src/lib.rs:179)",
    ),
    (
        "DriverErrorClass",
        "the public field DriverError::class (crates/substrate-host/src/lib.rs:84, :96), so \
         reading a refusal's class forces it",
    ),
    (
        "WorkspaceDestroyProgress",
        "returned by Driver::destroy_workspace (crates/substrate-host/src/lib.rs:111, :287)",
    ),
    (
        "ExecObservation",
        "returned by Driver::start_exec and Driver::observe_exec \
         (crates/substrate-host/src/lib.rs:26, :293, :346)",
    ),
    (
        "PipeStream",
        "the public field PipeFrame::stream (crates/substrate-host/src/process.rs:34, :41); \
         PipeFrame is returned by Driver::read_pipe_session \
         (crates/substrate-host/src/lib.rs:325)",
    ),
];

#[test]
fn driver_port_has_no_host_types() {
    let crate_root = Path::new(env!("CARGO_MANIFEST_DIR"));

    for (relative, reason) in COMPOSITION_ROOTS {
        assert!(
            crate_root.join(relative).is_file(),
            "the allowlist still exempts {relative} ({reason}), but that file is gone: \
             shrink the allowlist in this test"
        );
    }

    let mut sources = Vec::new();
    collect_rust_sources(&crate_root.join("src"), &mut sources);
    sources.sort();
    assert!(
        !sources.is_empty(),
        "found no Rust source under {}: this test would pass vacuously",
        crate_root.join("src").display()
    );

    let mut crossings = Vec::new();
    for path in &sources {
        let relative = path
            .strip_prefix(crate_root)
            .unwrap_or(path.as_path())
            .to_string_lossy()
            .into_owned();
        if COMPOSITION_ROOTS
            .iter()
            .any(|(allowed, _)| *allowed == relative)
        {
            continue;
        }
        let source = fs::read_to_string(path)
            .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
        let code = mask_comments_and_literals(&source);
        for (offset, item) in host_items(&code) {
            if PORT_ITEMS.iter().any(|(allowed, _)| *allowed == item) {
                continue;
            }
            let line = line_of(&code, offset);
            crossings.push(format!(
                "crates/substrate-daemon/{relative}:{line}: substrate_host::{item}"
            ));
        }
    }

    let report = crossings.join("\n");
    assert!(
        crossings.is_empty(),
        "a substrate-host type crosses the driver port outside the composition root:\n{report}\n\
         \n\
         AGENTS.md invariant 4: a client of the port must not name the host driver's internals. \
         Either express this through substrate_wire or the port trait, or — if the port genuinely \
         forces the name — widen PORT_ITEMS in crates/substrate-daemon/tests/driver_port.rs with \
         the file:line in substrate-host that forces it."
    );
}

fn collect_rust_sources(directory: &Path, found: &mut Vec<PathBuf>) {
    let entries = fs::read_dir(directory)
        .unwrap_or_else(|error| panic!("read directory {}: {error}", directory.display()));
    for entry in entries {
        let path = entry
            .unwrap_or_else(|error| panic!("read entry under {}: {error}", directory.display()))
            .path();
        if path.is_dir() {
            collect_rust_sources(&path, found);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            found.push(path);
        }
    }
}

/// Every item named directly under `substrate_host::`, as `(byte offset, item)`.
///
/// Handles a plain path (`substrate_host::DriverError`), a brace group spanning any number of
/// lines (`substrate_host::{A, B}`), a nested group (only its first segment is the item), a
/// rename (`A as B` yields `A`) and a glob (`*`, which is never allowlisted).
fn host_items(code: &str) -> Vec<(usize, String)> {
    let bytes = code.as_bytes();
    let mut items = Vec::new();
    let mut cursor = 0usize;
    while let Some(found) = code[cursor..].find(HOST_CRATE) {
        let at = cursor + found;
        cursor = at + HOST_CRATE.len();
        if at > 0 && is_identifier_byte(bytes[at - 1]) {
            continue;
        }
        collect_items(bytes, cursor, &mut items);
    }
    items
}

fn collect_items(bytes: &[u8], start: usize, items: &mut Vec<(usize, String)>) {
    let opening = skip_whitespace(bytes, start);
    if bytes.get(opening) != Some(&b'{') {
        push_leading_item(bytes, opening, items);
        return;
    }
    let mut depth = 0usize;
    let mut segment = opening + 1;
    let mut cursor = opening;
    while cursor < bytes.len() {
        match bytes[cursor] {
            b'{' => {
                depth += 1;
                if depth == 1 {
                    segment = cursor + 1;
                }
            }
            b'}' => {
                if depth <= 1 {
                    push_leading_item(bytes, segment, items);
                    return;
                }
                depth -= 1;
            }
            b',' if depth == 1 => {
                push_leading_item(bytes, segment, items);
                segment = cursor + 1;
            }
            _ => {}
        }
        cursor += 1;
    }
}

fn push_leading_item(bytes: &[u8], start: usize, items: &mut Vec<(usize, String)>) {
    let begin = skip_whitespace(bytes, start);
    match bytes.get(begin) {
        None => {}
        Some(&b'*') => items.push((begin, "*".to_owned())),
        Some(_) => {
            let mut end = begin;
            while end < bytes.len() && is_identifier_byte(bytes[end]) {
                end += 1;
            }
            if end > begin {
                items.push((
                    begin,
                    String::from_utf8_lossy(&bytes[begin..end]).into_owned(),
                ));
            }
        }
    }
}

fn skip_whitespace(bytes: &[u8], start: usize) -> usize {
    let mut cursor = start;
    while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
        cursor += 1;
    }
    cursor
}

fn is_identifier_byte(byte: u8) -> bool {
    byte == b'_' || byte.is_ascii_alphanumeric()
}

fn line_of(code: &str, offset: usize) -> usize {
    let mut line = 1usize;
    for byte in &code.as_bytes()[..offset] {
        if *byte == b'\n' {
            line += 1;
        }
    }
    line
}

/// Blanks out comments, string literals and character literals, keeping every byte offset and
/// newline in place so a reported line number is the source's own.
///
/// A comment that discusses `substrate_host::HostDriver` is prose, not a crossing; a string that
/// happens to contain `//` must not swallow the code after it. Raw strings (`r"…"`, `br#"…"#`)
/// are handled; a raw byte string is the same shape.
fn mask_comments_and_literals(source: &str) -> String {
    let bytes = source.as_bytes();
    let mut masked = vec![b' '; bytes.len()];
    let mut cursor = 0usize;
    while cursor < bytes.len() {
        let byte = bytes[cursor];
        cursor = if byte == b'\n' {
            masked[cursor] = b'\n';
            cursor + 1
        } else if byte == b'/' && bytes.get(cursor + 1) == Some(&b'/') {
            skip_line_comment(bytes, cursor)
        } else if byte == b'/' && bytes.get(cursor + 1) == Some(&b'*') {
            skip_block_comment(bytes, cursor, &mut masked)
        } else if byte == b'"' {
            skip_string(bytes, cursor, &mut masked)
        } else if let Some(after) = raw_string_end(bytes, cursor, &mut masked) {
            after
        } else if byte == b'\'' && is_character_literal(bytes, cursor) {
            skip_character_literal(bytes, cursor)
        } else {
            masked[cursor] = byte;
            cursor + 1
        };
    }
    String::from_utf8(masked).expect("masking replaces whole ASCII delimiters only")
}

fn skip_line_comment(bytes: &[u8], start: usize) -> usize {
    let mut cursor = start;
    while cursor < bytes.len() && bytes[cursor] != b'\n' {
        cursor += 1;
    }
    cursor
}

fn skip_block_comment(bytes: &[u8], start: usize, masked: &mut [u8]) -> usize {
    let mut depth = 1usize;
    let mut cursor = start + 2;
    while cursor < bytes.len() && depth > 0 {
        if bytes[cursor] == b'\n' {
            masked[cursor] = b'\n';
            cursor += 1;
        } else if bytes[cursor] == b'/' && bytes.get(cursor + 1) == Some(&b'*') {
            depth += 1;
            cursor += 2;
        } else if bytes[cursor] == b'*' && bytes.get(cursor + 1) == Some(&b'/') {
            depth -= 1;
            cursor += 2;
        } else {
            cursor += 1;
        }
    }
    cursor
}

fn skip_string(bytes: &[u8], start: usize, masked: &mut [u8]) -> usize {
    let mut cursor = start + 1;
    while cursor < bytes.len() {
        match bytes[cursor] {
            b'\n' => {
                masked[cursor] = b'\n';
                cursor += 1;
            }
            b'\\' => cursor += 2,
            b'"' => return cursor + 1,
            _ => cursor += 1,
        }
    }
    cursor
}

/// Masks `r"…"`, `r#"…"#`, `br"…"` and friends, returning the offset just past the literal.
/// Returns `None` when the cursor is not at a raw-string prefix.
fn raw_string_end(bytes: &[u8], start: usize, masked: &mut [u8]) -> Option<usize> {
    if start > 0 && is_identifier_byte(bytes[start - 1]) {
        return None;
    }
    let mut cursor = start;
    if bytes.get(cursor) == Some(&b'b') {
        cursor += 1;
    }
    if bytes.get(cursor) != Some(&b'r') {
        return None;
    }
    cursor += 1;
    let hash_start = cursor;
    while bytes.get(cursor) == Some(&b'#') {
        cursor += 1;
    }
    let hashes = cursor - hash_start;
    if bytes.get(cursor) != Some(&b'"') {
        return None;
    }
    cursor += 1;
    while cursor < bytes.len() {
        if bytes[cursor] == b'\n' {
            masked[cursor] = b'\n';
        } else if bytes[cursor] == b'"'
            && bytes.len() >= cursor + 1 + hashes
            && bytes[cursor + 1..cursor + 1 + hashes]
                .iter()
                .all(|byte| *byte == b'#')
        {
            return Some(cursor + 1 + hashes);
        }
        cursor += 1;
    }
    Some(cursor)
}

/// Distinguishes a character literal from a lifetime: `'a'` and `'\n'` are literals, `'a` in
/// `&'a str` is not.
fn is_character_literal(bytes: &[u8], start: usize) -> bool {
    bytes.get(start + 1) == Some(&b'\\') || bytes.get(start + 2) == Some(&b'\'')
}

fn skip_character_literal(bytes: &[u8], start: usize) -> usize {
    let mut cursor = start + 1;
    while cursor < bytes.len() {
        match bytes[cursor] {
            b'\\' => cursor += 2,
            b'\'' => return cursor + 1,
            _ => cursor += 1,
        }
    }
    cursor
}

/// The check above is only as good as its parser: a parser that sees nothing passes vacuously.
#[test]
fn parser_reads_every_form_a_crossing_can_take() {
    let source = "use substrate_host::HostDriver;\n\
                  use substrate_host::{HostConfig, Driver as Port};\n\
                  use substrate_host::{process::PipeFrame, probe::*};\n\
                  use substrate_host::*;\n\
                  let value = ::substrate_host::HostConfig::minimum(root);\n\
                  let other = not_substrate_host::Ignored;\n";
    let found: Vec<(usize, String)> = host_items(&mask_comments_and_literals(source))
        .into_iter()
        .map(|(offset, item)| (line_of(source, offset), item))
        .collect();
    let expected = vec![
        (1, "HostDriver"),
        (2, "HostConfig"),
        (2, "Driver"),
        (3, "process"),
        (3, "probe"),
        (4, "*"),
        (5, "HostConfig"),
    ];
    assert_eq!(
        found,
        expected
            .into_iter()
            .map(|(line, item)| (line, item.to_owned()))
            .collect::<Vec<_>>()
    );
}

#[test]
fn masking_keeps_prose_out_and_code_in() {
    let source = "// discusses substrate_host::HostDriver\n\
                  /* and substrate_host::HostConfig */\n\
                  let url = \"scheme://substrate_host::HostConfig\";\n\
                  let quote = '\\'';\n\
                  use substrate_host::HostDriver;\n";
    let code = mask_comments_and_literals(source);
    let found: Vec<(usize, String)> = host_items(&code)
        .into_iter()
        .map(|(offset, item)| (line_of(source, offset), item))
        .collect();
    assert_eq!(found, vec![(5, "HostDriver".to_owned())]);
}
