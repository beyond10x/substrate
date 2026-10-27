//! No writable subtree may hold a submodule the workspace registers (design 29 § 2).
//!
//! A submodule's `.git` is a `gitdir:` file in its checkout. Under `Scoped`, a run that may write
//! the checkout can replace that file with a repository whose configuration it chose, and a host
//! `git status` in the workspace root runs Git in every submodule, so it runs what that
//! configuration names (`core.fsmonitor`). That is the workspace's own `.git` reached through a
//! file the run may write, so it earns the same refusal as a subtree at or beneath `.git`:
//! `confinement.writable-subtree-git` (422).
//!
//! The registry is the workspace root's `.gitmodules`, read through the pinned workspace
//! descriptor with `openat2` (beneath, no symlinks) and bounded. Only its `path` keys are read.
//! A registered submodule that contains a writable subtree has its own `.gitmodules` read the same
//! way, from the pinned descriptor, so a nested submodule is held to the same rule. Each
//! submodule is descended into once, and at most [`MAX_GITMODULES_FILES`] registries are read, so
//! workspace content cannot make the walk repeat itself. A `.gitmodules` that is present but
//! cannot be read that way (a symlink, not a regular file, over the bound), or a walk past that
//! many registries, refuses the start rather than being taken for an empty registry.

use std::collections::BTreeSet;
use std::os::fd::OwnedFd;

use substrate_wire::WorkspaceAccess;

use crate::DriverError;
use crate::confinement::WRITABLE_SUBTREE_GIT;

/// The most bytes of one `.gitmodules` that are read.
pub(crate) const MAX_GITMODULES_BYTES: u64 = 1 << 20;

/// The most `.gitmodules` files one start reads, the root's included.
pub(crate) const MAX_GITMODULES_FILES: usize = 64;

const ADDRESS: &str = "workspace_access.writable_subtrees";

/// Refuses a `Scoped` access whose writable subtree is, or contains, a submodule registered in
/// the `.gitmodules` beneath `pinned`, the workspace directory the run binds.
pub(crate) fn refuse_registered_submodules(
    pinned: &OwnedFd,
    access: &WorkspaceAccess,
) -> Result<(), DriverError> {
    let Some(subtrees) = access.writable_subtrees() else {
        return Ok(());
    };
    let subtrees: Vec<Vec<&str>> = subtrees
        .iter()
        .map(|subtree| subtree.split('/').collect())
        .collect();
    check_directory(pinned, &[], &subtrees, &mut Walk::default())
}

/// What one start's walk has read so far.
#[derive(Default)]
struct Walk {
    /// Registries read, the root's included.
    read: usize,
    /// Each submodule already descended into, lowercased, so a registry named twice, or reached
    /// through two parents, is read once.
    visited: BTreeSet<Vec<String>>,
}

/// Checks the `.gitmodules` of the directory at `prefix` (components beneath the workspace),
/// open as `directory`, and descends into each registered submodule that contains a subtree.
/// Each descent is strictly towards a subtree, so the depth is bounded by the subtree's own.
fn check_directory(
    directory: &OwnedFd,
    prefix: &[String],
    subtrees: &[Vec<&str>],
    walk: &mut Walk,
) -> Result<(), DriverError> {
    walk.read += 1;
    if walk.read > MAX_GITMODULES_FILES {
        return Err(DriverError::refused(
            WRITABLE_SUBTREE_GIT,
            format!(
                "The workspace nests more than {MAX_GITMODULES_FILES} submodule registries on the \
                 way to its writable subtrees, so no writable subtree can be shown to hold no \
                 submodule."
            ),
            ADDRESS,
        ));
    }
    let shown = |path: &[String]| {
        if path.is_empty() {
            ".gitmodules".to_owned()
        } else {
            format!("{}/.gitmodules", path.join("/"))
        }
    };
    let bytes = crate::fs::read_regular_beneath(directory, ".gitmodules", MAX_GITMODULES_BYTES)
        .map_err(|error| {
            DriverError::refused(
                WRITABLE_SUBTREE_GIT,
                format!(
                    "The workspace's {} could not be read beneath the workspace within {} bytes \
                     ({}), so no writable subtree can be shown to hold no submodule.",
                    shown(prefix),
                    MAX_GITMODULES_BYTES,
                    error.code
                ),
                ADDRESS,
            )
        })?;
    let Some(bytes) = bytes else {
        return Ok(());
    };
    for path in registered_paths(&String::from_utf8_lossy(&bytes)) {
        let mut full = prefix.to_vec();
        full.extend(path);
        if full.len() <= prefix.len() {
            continue;
        }
        if let Some(subtree) = subtrees.iter().find(|subtree| beneath(&full, subtree)) {
            return Err(DriverError::refused(
                WRITABLE_SUBTREE_GIT,
                format!(
                    "The writable subtree {} holds the submodule {} that {} registers; its .git \
                     belongs to the workspace's own Git.",
                    subtree.join("/"),
                    full.join("/"),
                    shown(prefix)
                ),
                ADDRESS,
            ));
        }
        let key: Vec<String> = full.iter().map(|part| part.to_ascii_lowercase()).collect();
        if subtrees.iter().any(|subtree| beneath(subtree, &full)) && walk.visited.insert(key) {
            let mut submodule: Option<OwnedFd> = None;
            for component in &full[prefix.len()..] {
                let parent = submodule.as_ref().unwrap_or(directory);
                let next =
                    crate::fs::open_component_beneath(parent, component).map_err(|error| {
                        DriverError::refused(
                            WRITABLE_SUBTREE_GIT,
                            format!(
                                "The submodule {} that contains a writable subtree could not be \
                                 opened beneath the workspace ({}).",
                                full.join("/"),
                                error.code
                            ),
                            ADDRESS,
                        )
                    })?;
                submodule = Some(next);
            }
            if let Some(submodule) = submodule {
                check_directory(&submodule, &full, subtrees, walk)?;
            }
        }
    }
    Ok(())
}

/// Whether `path` equals `ancestor` or lies beneath it, component by component, compared ASCII
/// case-insensitively as the `.git` rule is, so a case-folding directory cannot slip past it.
fn beneath<A: AsRef<str>, B: AsRef<str>>(path: &[A], ancestor: &[B]) -> bool {
    path.len() >= ancestor.len()
        && path
            .iter()
            .zip(ancestor)
            .all(|(left, right)| left.as_ref().eq_ignore_ascii_case(right.as_ref()))
}

/// The value of every `path` key in a `.gitmodules` text, as components with `.` and empty ones
/// dropped and `..` applied. A value that climbs above its directory registers nothing beneath it
/// and is skipped. Git's syntax as far as a `path` needs it: a section header may share its line
/// with a key, keys are case-insensitive, a value may be quoted, escaped, continued with a
/// trailing backslash, and ends at an unquoted `#` or `;`.
fn registered_paths(text: &str) -> Vec<Vec<String>> {
    let mut paths = Vec::new();
    for line in join_continuations(text).lines() {
        let mut rest = line.trim_end_matches('\r').trim_start();
        if rest.starts_with('[') {
            let Some(end) = section_end(rest) else {
                continue;
            };
            rest = rest[end + 1..].trim_start();
        }
        let key_len = rest
            .find(|character: char| !(character.is_ascii_alphanumeric() || character == '-'))
            .unwrap_or(rest.len());
        if !rest[..key_len].eq_ignore_ascii_case("path") {
            continue;
        }
        let Some(value) = rest[key_len..].trim_start().strip_prefix('=') else {
            continue;
        };
        if let Some(path) = components(&config_value(value)) {
            paths.push(path);
        }
    }
    paths
}

/// `text` with every unescaped backslash before a line end removed together with that line end,
/// in one pass: an escaped pair (`\\`, `\"`) is copied whole, so the backslash after it is judged
/// on its own, and the work stays linear whatever the text holds.
fn join_continuations(text: &str) -> String {
    let mut joined = String::with_capacity(text.len());
    let mut characters = text.chars().peekable();
    while let Some(character) = characters.next() {
        if character != '\\' {
            joined.push(character);
            continue;
        }
        match characters.peek() {
            Some('\n') => {
                characters.next();
            }
            Some('\r') => {
                characters.next();
                if characters.peek() == Some(&'\n') {
                    characters.next();
                } else {
                    joined.push_str("\\\r");
                }
            }
            Some(_) => {
                joined.push('\\');
                joined.extend(characters.next());
            }
            None => joined.push('\\'),
        }
    }
    joined
}

/// The byte index of the `]` that closes a section header, skipping a quoted subsection name.
fn section_end(header: &str) -> Option<usize> {
    let mut quoted = false;
    let mut escaped = false;
    for (index, character) in header.char_indices() {
        match character {
            _ if escaped => escaped = false,
            '\\' => escaped = true,
            '"' => quoted = !quoted,
            ']' if !quoted => return Some(index),
            _ => {}
        }
    }
    None
}

/// A config value: quotes removed, escapes resolved, cut at an unquoted comment, trimmed.
fn config_value(raw: &str) -> String {
    let mut value = String::new();
    let mut quoted = false;
    let mut characters = raw.chars();
    while let Some(character) = characters.next() {
        match character {
            '\\' => match characters.next() {
                Some('n') => value.push('\n'),
                Some('t') => value.push('\t'),
                Some('b') => value.push('\u{8}'),
                Some(other) => value.push(other),
                None => {}
            },
            '"' => quoted = !quoted,
            '#' | ';' if !quoted => break,
            other => value.push(other),
        }
    }
    value.trim().to_owned()
}

/// A registered path as components beneath its directory, or `None` when it names nothing there.
fn components(path: &str) -> Option<Vec<String>> {
    let mut components: Vec<String> = Vec::new();
    for component in path.split('/') {
        match component {
            "" | "." => {}
            ".." => {
                components.pop()?;
            }
            other => components.push(other.to_owned()),
        }
    }
    (!components.is_empty()).then_some(components)
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::symlink;
    use std::path::Path;

    use super::*;

    fn scoped(subtrees: &[&str]) -> WorkspaceAccess {
        WorkspaceAccess::Scoped {
            writable_subtrees: subtrees
                .iter()
                .map(|subtree| (*subtree).to_owned())
                .collect(),
        }
    }

    fn check(root: &Path, access: &WorkspaceAccess) -> Result<(), DriverError> {
        let pinned = crate::fs::pin_workspace_directory(root).expect("pinned");
        refuse_registered_submodules(&pinned, access)
    }

    fn workspace(gitmodules: &str) -> tempfile::TempDir {
        let root = tempfile::tempdir().expect("workspace");
        std::fs::write(root.path().join(".gitmodules"), gitmodules).expect(".gitmodules");
        root
    }

    fn refused(error: &DriverError) {
        assert_eq!(error.code, WRITABLE_SUBTREE_GIT, "{error:?}");
        assert_eq!(error.class, crate::DriverErrorClass::Refused, "{error:?}");
    }

    const VENDOR: &str = "[submodule \"vendor\"]\n\tpath = out/vendor\n\turl = ../vendor\n";

    #[test]
    fn a_subtree_holding_or_naming_a_registered_submodule_is_refused() {
        let root = workspace(VENDOR);
        for subtrees in [&["out"][..], &["out/vendor"], &["build", "out"]] {
            let error = check(root.path(), &scoped(subtrees)).expect_err("refused");
            refused(&error);
            assert!(error.message.contains("out/vendor"), "{error:?}");
        }
    }

    #[test]
    fn a_subtree_beside_or_inside_a_submodule_is_admitted() {
        let root = workspace(VENDOR);
        std::fs::create_dir_all(root.path().join("out/vendor/src")).expect("checkout");
        for subtrees in [
            &["build"][..],
            &["output"],
            &["ou"],
            &["out/vendor/src"],
            &["out/v"],
        ] {
            check(root.path(), &scoped(subtrees)).expect("admitted");
        }
    }

    #[test]
    fn every_spelling_git_reads_as_the_path_key_is_found() {
        for gitmodules in [
            "[submodule \"v\"]\npath=out/vendor\n",
            "[submodule \"v\"]\n  PATH = out/vendor   \n",
            "[submodule \"v\"]\n\tpath = \"out/vendor\"\n",
            "[submodule \"v\"]\n\tpath = out/vendor # a comment\n",
            "[submodule \"v\"]\n\tpath = out/vendor; a comment\n",
            "[submodule \"v\"]\n\tpath = ./out/vendor/\n",
            "[submodule \"v\"]\n\tpath = out//vendor\n",
            "[submodule \"v\"]\n\tpath = x/../out/vendor\n",
            "[submodule \"v\"] path = out/vendor\n",
            "[submodule \"a]b\"] path = out/vendor\n",
            "[submodule \"v\"]\n\tpath = \"out\\/vendor\"\n",
            "[submodule \"v\"]\n\tpath = out/ven\\\ndor\n",
            "[submodule \"v\"]\r\n\tpath = out/vendor\r\n",
            "[submodule \"v\"]\n\tpath = Out/Vendor\n",
        ] {
            let root = workspace(gitmodules);
            let error = check(root.path(), &scoped(&["out"])).expect_err(gitmodules);
            refused(&error);
        }
    }

    #[test]
    fn comments_other_keys_and_bare_paths_register_nothing() {
        let root = workspace(
            "# path = out/vendor\n; path = out/vendor\n[submodule \"out/vendor\"]\n\
             \turl = out/vendor\n\tpathname = out/vendor\n\tpath\n",
        );
        check(root.path(), &scoped(&["out"])).expect("admitted");
    }

    #[test]
    fn without_a_registry_or_a_narrower_access_nothing_is_read() {
        let root = tempfile::tempdir().expect("workspace");
        check(root.path(), &scoped(&["out"])).expect("no .gitmodules");
        let target = root.path().join("elsewhere");
        std::fs::write(&target, VENDOR).expect("target");
        symlink(&target, root.path().join(".gitmodules")).expect("symlink");
        for access in [WorkspaceAccess::ReadWrite, WorkspaceAccess::ReadOnly] {
            check(root.path(), &access).expect("nothing to check");
        }
    }

    #[test]
    fn a_registry_that_cannot_be_read_beneath_the_workspace_refuses() {
        let linked = tempfile::tempdir().expect("workspace");
        let target = linked.path().join("elsewhere");
        std::fs::write(&target, "").expect("target");
        symlink(&target, linked.path().join(".gitmodules")).expect("symlink");
        refused(&check(linked.path(), &scoped(&["out"])).expect_err("a symlink"));

        let directory = tempfile::tempdir().expect("workspace");
        std::fs::create_dir(directory.path().join(".gitmodules")).expect("directory");
        refused(&check(directory.path(), &scoped(&["out"])).expect_err("a directory"));

        let large = tempfile::tempdir().expect("workspace");
        let filler = "# filler\n".repeat(usize::try_from(MAX_GITMODULES_BYTES).expect("fits") / 9);
        std::fs::write(
            large.path().join(".gitmodules"),
            format!("{filler}{filler}"),
        )
        .expect("large");
        refused(&check(large.path(), &scoped(&["out"])).expect_err("over the bound"));
        std::fs::write(large.path().join(".gitmodules"), &filler).expect("at the bound");
        check(large.path(), &scoped(&["out"])).expect("within the bound");
    }

    /// A chain of nested registries, each naming the next level, `depth` deep beneath `top`.
    fn chain(root: &Path, top: &str, depth: usize, every_descendant: bool) -> String {
        let mut path = top.to_owned();
        let mut registry = format!("path = {top}\n");
        let mut levels = vec![top.to_owned()];
        for _ in 1..depth {
            path.push_str("/n");
            levels.push(path.clone());
        }
        std::fs::create_dir_all(root.join(&path).join("out")).expect("chain");
        for (index, level) in levels.iter().enumerate() {
            let below: Vec<String> = levels[index + 1..]
                .iter()
                .map(|deeper| deeper[level.len() + 1..].to_owned())
                .collect();
            let named = if every_descendant {
                &below[..]
            } else {
                &below[..below.len().min(1)]
            };
            let mut text = String::new();
            for name in named {
                text.push_str("path = ");
                text.push_str(name);
                text.push('\n');
            }
            std::fs::write(root.join(level).join(".gitmodules"), text).expect("registry");
            if every_descendant && index == 0 {
                for name in &below {
                    registry.push_str("path = ");
                    registry.push_str(top);
                    registry.push('/');
                    registry.push_str(name);
                    registry.push('\n');
                }
            }
        }
        registry.push_str(&"path = elsewhere\n".repeat(1000));
        registry
    }

    #[test]
    fn a_registry_reached_twice_is_read_once_and_the_walk_stays_bounded() {
        let root = tempfile::tempdir().expect("workspace");
        let registry = chain(root.path(), "a", 12, true);
        std::fs::write(root.path().join(".gitmodules"), registry).expect("root registry");
        let subtree = format!("a{}/out", "/n".repeat(11));
        check(root.path(), &scoped(&[&subtree])).expect("admitted, each registry read once");
    }

    #[test]
    fn more_registries_than_the_walk_reads_refuses() {
        let root = tempfile::tempdir().expect("workspace");
        let mut registry = chain(root.path(), "a", 40, false);
        registry.push_str(&chain(root.path(), "b", 40, false));
        std::fs::write(root.path().join(".gitmodules"), registry).expect("root registry");
        let deep = |top: &str| format!("{top}{}/out", "/n".repeat(39));
        let error = check(root.path(), &scoped(&[&deep("a"), &deep("b")])).expect_err("refused");
        refused(&error);
        assert!(error.message.contains("registries"), "{error:?}");
    }

    /// Every physical line ends in an escaped backslash and a continuation, so the whole run is
    /// one logical line; the empty line ends it. Read in one pass, it ends in milliseconds, and
    /// the section after it still registers its submodule.
    #[test]
    fn a_registry_of_continued_lines_is_read_in_one_pass() {
        let continued = "\\\\\\\n".repeat(250_000);
        let root = workspace(&format!(
            "{continued}\n[submodule \"v\"]\n\tpath = out/vendor\n"
        ));
        refused(&check(root.path(), &scoped(&["out"])).expect_err("refused"));
    }

    #[test]
    fn a_submodule_registered_inside_a_submodule_is_held_to_the_rule() {
        let root = workspace("[submodule \"lib\"]\n\tpath = lib\n");
        std::fs::create_dir_all(root.path().join("lib/out/x")).expect("lib/out/x");
        std::fs::write(
            root.path().join("lib/.gitmodules"),
            "[submodule \"x\"]\n\tpath = out/x\n",
        )
        .expect("lib/.gitmodules");
        refused(&check(root.path(), &scoped(&["lib/out"])).expect_err("nested"));
        check(root.path(), &scoped(&["lib/src"])).expect("beside the nested one");
    }
}
