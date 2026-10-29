//! The `v2` policy refuses every remount `v1` refuses, except one that adds `noatime` or
//! `nodiratime` (D180, condition 1). Read from the committed policy sources; no parser is run.
//!
//! Two checks, each able to fail on its own:
//!
//! - **Semantics.** Every remount rule of each policy is parsed into a path and a flag constraint,
//!   and every combination of the mount flags `AppArmor` names is asked of both, at each path a
//!   rule names and at the path the kernel denied. A remount `v2` admits must be one `v1` admits
//!   once `noatime` and `nodiratime` are removed from it, and every remount `v1` admits `v2` admits
//!   too. A flag rule is matched as `apparmor.d(5)` states: `options=(…)` is the exact flag set,
//!   `options in (…)` any subset of it, no options any flags; an option that clears a flag (`rw`,
//!   `suid`, …) sets none.
//! - **Text.** With each policy's own name replaced, the two sources are the same lines except the
//!   one `remount options in (…) /**,` rule, whose options in `v2` are exactly `v1`'s plus
//!   `noatime` and `nodiratime`. That holds every rule this module does not model, the `mount`
//!   rules whose remount behaviour is the parser's business included, identical by text, and it
//!   refuses an edit that changes no admitted set, such as listing `suid`.
//!
//! A rule, option or path this module cannot read fails the test rather than being skipped.

use super::{POLICY_V1, POLICY_V2};
use crate::profile_name::{V1, V2};

/// Every option `apparmor.d(5)` names that sets mount flags, with the Linux `MS_*` bits it sets.
const SETS: [(&str, u32); 37] = [
    ("ro", RDONLY),
    ("r", RDONLY),
    ("read-only", RDONLY),
    ("nosuid", 1 << 1),
    ("nodev", 1 << 2),
    ("noexec", 1 << 3),
    ("sync", 1 << 4),
    ("remount", REMOUNT),
    ("mand", 1 << 6),
    ("dirsync", 1 << 7),
    ("nosymfollow", 1 << 8),
    ("noatime", NOATIME),
    ("nodiratime", NODIRATIME),
    ("bind", BIND),
    ("B", BIND),
    ("move", 1 << 13),
    ("M", 1 << 13),
    ("rbind", BIND | REC),
    ("R", BIND | REC),
    ("silent", 1 << 15),
    ("acl", 1 << 16),
    ("unbindable", 1 << 17),
    ("runbindable", (1 << 17) | REC),
    ("make-unbindable", 1 << 17),
    ("private", 1 << 18),
    ("rprivate", (1 << 18) | REC),
    ("make-private", 1 << 18),
    ("slave", 1 << 19),
    ("rslave", (1 << 19) | REC),
    ("make-slave", 1 << 19),
    ("shared", 1 << 20),
    ("rshared", (1 << 20) | REC),
    ("make-shared", 1 << 20),
    ("relatime", 1 << 21),
    ("iversion", 1 << 23),
    ("strictatime", 1 << 24),
    ("lazytime", 1 << 25),
];

/// Every option `apparmor.d(5)` names that only clears a flag.
const CLEARS: [&str; 18] = [
    "rw",
    "w",
    "suid",
    "dev",
    "exec",
    "async",
    "nomand",
    "nodirsync",
    "symfollow",
    "atime",
    "diratime",
    "loud",
    "noacl",
    "norelatime",
    "noiversion",
    "nostrictatime",
    "nolazytime",
    "nouser",
];

const RDONLY: u32 = 1;
const REMOUNT: u32 = 1 << 5;
const NOATIME: u32 = 1 << 10;
const NODIRATIME: u32 = 1 << 11;
const BIND: u32 = 1 << 12;
const REC: u32 = 1 << 14;
/// The two flags D180 admits in `v2`.
const ATIME: u32 = NOATIME | NODIRATIME;

/// Every flag a remount request can carry beside `MS_REMOUNT`, which a remount rule implies.
fn universe() -> u32 {
    SETS.iter().fold(0, |all, (_, bits)| all | bits) & !REMOUNT
}

fn bits(option: &str) -> u32 {
    if let Some((_, bits)) = SETS.iter().find(|(name, _)| *name == option) {
        *bits
    } else if CLEARS.contains(&option) {
        0
    } else {
        panic!("unknown mount option {option:?}: this test cannot read it")
    }
}

fn names(mask: u32) -> String {
    let mut named = Vec::new();
    let mut seen = 0;
    for (name, bits) in SETS {
        if mask & bits == bits && bits & !seen != 0 && bits.is_power_of_two() {
            named.push(name);
            seen |= bits;
        }
    }
    if mask & REC != 0 {
        named.push("MS_REC");
    }
    named.join(",")
}

#[derive(Debug, Clone, Copy)]
enum Flags {
    Any,
    Exact(u32),
    Within(u32),
}

#[derive(Debug)]
struct Remount {
    flags: Flags,
    path: Option<String>,
}

impl Flags {
    fn admit(self, mask: u32) -> bool {
        match self {
            Self::Any => true,
            Self::Exact(set) => mask == set,
            Self::Within(set) => mask & !set == 0,
        }
    }
}

impl Remount {
    fn at(&self, path: &str) -> bool {
        match self.path.as_deref() {
            None | Some("/**") => path.starts_with('/'),
            Some(exact) => exact == path,
        }
    }

    fn admits(&self, mask: u32, path: &str) -> bool {
        self.at(path) && self.flags.admit(mask)
    }
}

/// The option list of a rule, `(a,b)` or one bare word, and what follows it.
fn option_list(rest: &str) -> (Vec<String>, &str) {
    let rest = rest.trim_start();
    if let Some(inner) = rest.strip_prefix('(') {
        let (list, after) = inner.split_once(')').expect("a closed option list");
        let options = list
            .split(',')
            .map(str::trim)
            .filter(|option| !option.is_empty())
            .map(str::to_owned)
            .collect();
        (options, after)
    } else {
        let end = rest
            .find(|c: char| c.is_whitespace() || c == ',')
            .unwrap_or(rest.len());
        (vec![rest[..end].to_owned()], &rest[end..])
    }
}

fn mask_of(options: &[String]) -> u32 {
    options.iter().fold(0, |mask, option| mask | bits(option)) & !REMOUNT
}

/// A rule's text, with its trailing comma, or `None` for a line that holds no rule.
fn rule(line: &str) -> Option<&str> {
    let line = line.trim();
    (!line.is_empty() && !line.starts_with('#') && line.ends_with(',')).then_some(line)
}

/// Every rule of `policy` that admits a remount.
fn remounts(policy: &[u8]) -> Vec<Remount> {
    let source = std::str::from_utf8(policy).unwrap();
    let mut found = Vec::new();
    for line in source.lines().filter_map(rule) {
        let body = line.strip_suffix(',').unwrap();
        let (keyword, rest) = body.split_once(' ').unwrap_or((body, ""));
        let rest = rest.trim();
        let remount = match keyword {
            "remount" => true,
            "mount" => {
                assert!(
                    !rest.contains("remount"),
                    "a mount rule naming remount is not read here: {line}"
                );
                false
            }
            _ => false,
        };
        if !remount {
            continue;
        }
        let (flags, rest) = if let Some(after) = rest.strip_prefix("options in") {
            let (options, rest) = option_list(after);
            (Flags::Within(mask_of(&options)), rest)
        } else if let Some(after) = rest.strip_prefix("options=") {
            let (options, rest) = option_list(after);
            (Flags::Exact(mask_of(&options)), rest)
        } else {
            (Flags::Any, rest)
        };
        let rest = rest.trim();
        let path = match rest {
            "" => None,
            path if path == "/**" || !path.contains(['*', '?', '[', '{', '@', ' ']) => {
                Some(path.to_owned())
            }
            other => panic!("a remount path this test cannot read: {other:?} in {line}"),
        };
        found.push(Remount { flags, path });
    }
    found
}

/// The paths every remount is asked at: each one a rule names, plus the one the kernel denied.
fn paths(rules: &[&[Remount]]) -> Vec<String> {
    let mut paths = vec![DENIED_PATH.to_owned(), "/".to_owned()];
    for rule in rules.iter().flat_map(|rules| rules.iter()) {
        if let Some(path) = rule.path.as_deref().filter(|path| *path != "/**") {
            paths.push(path.to_owned());
        }
    }
    paths.sort();
    paths.dedup();
    paths
}

const DENIED_PATH: &str = "/newroot/runtime/sentinel/";

fn any_admits(rules: &[Remount], mask: u32, path: &str) -> bool {
    rules.iter().any(|rule| rule.admits(mask, path))
}

/// The semantic half: every flag combination at every path.
fn widened_or_narrowed(v1: &[u8], v2: &[u8]) -> Vec<String> {
    let old = remounts(v1);
    let new = remounts(v2);
    assert!(!old.is_empty() && !new.is_empty(), "no remount rule read");
    let all = universe();
    let mut failures = Vec::new();
    for path in paths(&[&old, &new]) {
        let here = |rules: &[Remount]| -> Vec<Flags> {
            rules
                .iter()
                .filter(|rule| rule.at(&path))
                .map(|rule| rule.flags)
                .collect()
        };
        let (old, new) = (here(&old), here(&new));
        let admits = |rules: &[Flags], mask| rules.iter().any(|flags| flags.admit(mask));
        // Every subset of the universe, the empty one last.
        let mut mask = all;
        loop {
            let before = admits(&old, mask);
            let after = admits(&new, mask);
            if after && !admits(&old, mask & !ATIME) {
                failures.push(format!("v2 admits remount ({}) {path}", names(mask)));
            }
            if before && !after {
                failures.push(format!("v2 refuses remount ({}) {path}", names(mask)));
            }
            if failures.len() >= 8 || mask == 0 {
                break;
            }
            mask = (mask - 1) & all;
        }
    }
    failures
}

/// The textual half: the sources differ only in their own names and the atime options.
fn text_difference(v1: &[u8], v2: &[u8]) -> Vec<String> {
    let old = std::str::from_utf8(v1).unwrap().replace(V1, "@self");
    let new = std::str::from_utf8(v2).unwrap().replace(V2, "@self");
    let old: Vec<&str> = old.lines().collect();
    let new: Vec<&str> = new.lines().collect();
    if old.len() != new.len() {
        return vec![format!("v1 has {} lines, v2 {}", old.len(), new.len())];
    }
    let changed: Vec<(&str, &str)> = old
        .iter()
        .zip(&new)
        .filter(|(old, new)| old != new)
        .map(|(old, new)| (*old, *new))
        .collect();
    let [(old, new)] = changed.as_slice() else {
        return vec![format!("v2 changes {} lines: {changed:#?}", changed.len())];
    };
    let options = |line: &str| -> Option<(Vec<String>, String)> {
        let body = line.trim().strip_prefix("remount options in")?;
        let (options, rest) = option_list(body);
        Some((options, rest.trim().to_owned()))
    };
    let (Some((before, old_path)), Some((after, new_path))) = (options(old), options(new)) else {
        return vec![format!(
            "v2 changes a line that is not the remount rule: {new}"
        )];
    };
    let mut failures = Vec::new();
    if old_path != "/**," || new_path != old_path {
        failures.push(format!("the remount rule's path changed: {new}"));
    }
    let mut expected = before.clone();
    expected.extend(["noatime".to_owned(), "nodiratime".to_owned()]);
    let mut sorted_expected = expected.clone();
    sorted_expected.sort();
    let mut sorted_after = after.clone();
    sorted_after.sort();
    let mut unique = sorted_after.clone();
    unique.dedup();
    if before
        .iter()
        .any(|option| option == "noatime" || option == "nodiratime")
        || sorted_after != sorted_expected
        || unique != sorted_after
    {
        failures.push(format!(
            "v2's remount options are {after:?}, not v1's {before:?} plus noatime and nodiratime"
        ));
    }
    failures
}

/// The denied request from the decision, as the kernel logged it.
fn denied_mask(flags: &str) -> u32 {
    let options: Vec<String> = flags.split(',').map(|f| f.trim().to_owned()).collect();
    mask_of(&options)
}

#[test]
fn v2_refuses_every_remount_v1_refuses_except_those_adding_noatime_or_nodiratime() {
    let failures = widened_or_narrowed(POLICY_V1, POLICY_V2);
    assert!(failures.is_empty(), "{failures:#?}");
    let failures = text_difference(POLICY_V1, POLICY_V2);
    assert!(failures.is_empty(), "{failures:#?}");
}

/// The remount bubblewrap makes of a `noatime` bind (D180's kernel log), refused by `v1` and
/// admitted by `v2`, with `nodiratime` too; the same remount without them is admitted by both.
#[test]
fn the_remount_the_kernel_denied_under_v1_is_admitted_by_v2() {
    let v1 = remounts(POLICY_V1);
    let v2 = remounts(POLICY_V2);
    let denied = denied_mask("ro, nosuid, nodev, remount, noatime, bind, silent");
    assert_eq!(denied & ATIME, NOATIME);
    for mask in [denied, denied | NODIRATIME, denied & !NOATIME | NODIRATIME] {
        assert!(!any_admits(&v1, mask, DENIED_PATH), "{}", names(mask));
        assert!(any_admits(&v2, mask, DENIED_PATH), "{}", names(mask));
    }
    let relatime = denied & !ATIME;
    assert!(any_admits(&v1, relatime, DENIED_PATH));
    assert!(any_admits(&v2, relatime, DENIED_PATH));
}

/// The checker itself: a `v2` that admits another flag, loses one, or lists a no-op option fails.
#[test]
fn a_mutant_v2_fails_the_diff() {
    let source = std::str::from_utf8(POLICY_V2).unwrap();
    let rule = "remount options in (rw,ro,bind,nosuid,nodev,noexec,relatime,silent,noatime,nodiratime) /**,";
    assert!(source.contains(rule), "the v2 remount rule moved: {source}");
    for (mutant, semantic) in [
        (
            "(rw,ro,bind,nosuid,nodev,noexec,relatime,silent,noatime,nodiratime,strictatime)",
            true,
        ),
        (
            "(rw,ro,bind,nosuid,nodev,noexec,relatime,silent,noatime,nodiratime,suid,dev)",
            false,
        ),
        (
            "(rw,bind,nosuid,nodev,noexec,relatime,silent,noatime,nodiratime)",
            true,
        ),
        (
            "(rw,ro,bind,nodev,noexec,relatime,silent,noatime,nodiratime)",
            true,
        ),
        (
            "(rw,ro,bind,rbind,nosuid,nodev,noexec,relatime,silent,noatime,nodiratime)",
            true,
        ),
    ] {
        let mutated = source.replace(rule, &format!("remount options in {mutant} /**,"));
        assert_ne!(mutated, source);
        let semantics = widened_or_narrowed(POLICY_V1, mutated.as_bytes());
        let text = text_difference(POLICY_V1, mutated.as_bytes());
        assert_eq!(!semantics.is_empty(), semantic, "{mutant}: {semantics:#?}");
        assert!(!text.is_empty(), "{mutant} passes the text check");
    }
    let unbounded = source.replace(rule, "remount /**,");
    assert!(!widened_or_narrowed(POLICY_V1, unbounded.as_bytes()).is_empty());
    let added = source.replace(rule, &format!("{rule}\n  mount options=(rw,move),"));
    assert!(!text_difference(POLICY_V1, added.as_bytes()).is_empty());
}
