#![forbid(unsafe_code)]
//! **The class**: the documents that state the confinement floor this crate enforces, read as the
//! specification they claim to be against the floor the crate now enforces.
//!
//! `story:confined-processes-cannot-nest-user-namespaces` added a clause to the floor — the child's
//! own user namespace is non-nestable, `--disable-userns` requests it and
//! `--assert-userns-disabled` observes it — and made that clause a **precondition of advertising
//! `exec` at all**: `probe_bubblewrap` puts `--assert-userns-disabled` in its argv
//! (`crates/substrate-host/src/probe.rs:381`) and answers `false` when a backend cannot honour it,
//! which withholds `exec.namespaces` and every fact gated on it
//! (`crates/substrate-host/src/probe.rs:49`).
//!
//! A floor clause is only a floor if the documents that state the floor state it. Round 1 of this
//! unit filed exactly that finding; the correction reached two documents. These cases read the
//! documents the repository itself nominates:
//!
//! | document | why it is nominated |
//! |---|---|
//! | `AGENTS.md`, the *enforced isolation set is a floor* bullet | `docs/design/15-docker-driver-entry-gate.md:147`: "The floor is `AGENTS.md:91-99` and design 04 § 7 (`:85-100`)" |
//! | `docs/design/04-security-and-isolation.md` § 7 | the same sentence, and § 7's own opening: "requires **all of the following** before it advertises `exec`" |
//! | `README.md` § *Serving exec* | the operator-facing list of what a Linux deployment must provide before exec is served |
//!
//! Sections are located by their headings and their opening words rather than by line number, so
//! that a document reshuffle fails loudly here instead of passing vacuously.

use std::path::{Path, PathBuf};

/// Any of the ways a document may state the clause. The option names are the specific form; the
/// two prose forms are what a document that describes rather than cites would say.
const CLAUSE: [&str; 4] = [
    "disable-userns",
    "assert-userns-disabled",
    "nested user namespace",
    "nest a user namespace",
];

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("the host crate sits two directories below the repository root")
        .to_path_buf()
}

fn document(relative: &str) -> String {
    let path = repository_root().join(relative);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
}

/// The block of `text` that begins at the first line `starts` accepts and ends before the next line
/// `ends` accepts.
///
/// The opening line is never offered to `ends`, so a section heading can be delimited by the next
/// heading of its own level.
fn block(text: &str, starts: impl Fn(&str) -> bool, ends: impl Fn(&str) -> bool) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let opening = lines
        .iter()
        .position(|line| starts(line))
        .expect("the nominated section is still in the document");
    let mut block = vec![lines[opening]];
    for line in &lines[opening + 1..] {
        if ends(line) {
            break;
        }
        block.push(line);
    }
    block.join("\n")
}

fn states_the_clause(section: &str) -> bool {
    CLAUSE.iter().any(|form| section.contains(form))
}

/// **Both documents design 15 names as the floor state the floor's user-namespace clause.**
///
/// `docs/design/15-docker-driver-entry-gate.md:147` names two, and its table below that line adds a
/// `no nested user namespace` row whose whole point is that the clause is part of the floor rather
/// than a host extra. A container driver is measured against that floor by reading these two
/// documents, so a clause in one and not the other is a floor with two different values.
#[test]
fn every_document_the_floor_is_named_in_states_the_no_nested_user_namespace_clause() {
    let agents = document("AGENTS.md");
    let isolation_set = block(
        &agents,
        |line| line.starts_with("- **The enforced isolation set is a floor"),
        |line| line.starts_with("- **"),
    );
    assert!(
        isolation_set.contains("openat2"),
        "the AGENTS.md floor bullet was located but does not read like the floor: {isolation_set}"
    );

    let design_04 = document("docs/design/04-security-and-isolation.md");
    let minimum_host_guarantee = block(
        &design_04,
        |line| line.starts_with("## 7. Minimum host guarantee"),
        |line| line.starts_with("## "),
    );
    assert!(
        minimum_host_guarantee.contains("bubblewrap"),
        "design 04 § 7 was located but does not read like the minimum host guarantee: \
         {minimum_host_guarantee}"
    );

    let silent: Vec<&str> = [
        (
            "AGENTS.md, the enforced-isolation-set bullet",
            &isolation_set,
        ),
        (
            "docs/design/04-security-and-isolation.md § 7, Minimum host guarantee",
            &minimum_host_guarantee,
        ),
    ]
    .into_iter()
    .filter(|(_, section)| !states_the_clause(section))
    .map(|(name, _)| name)
    .collect();

    assert_eq!(
        silent,
        Vec::<&str>::new(),
        "design 15:147 names these documents as the floor, and the host now withholds \
         `exec.namespaces` outright when a backend cannot prove a non-nestable user namespace \
         (probe.rs:381, :49). A floor document that does not state the clause states a weaker \
         floor than the one the code enforces, which is the shape round 1 finding 4 already \
         reported"
    );
}

/// **The operator-facing deployment list names the backend options exec now depends on.**
///
/// `README.md` § *Serving exec* is the enumeration an operator follows to get exec served: a
/// delegated cgroup subtree, a process-free delegation root, "the configured bubblewrap binary and
/// `/usr/bin/socat`", and `--cgroup-root`. After this unit a bubblewrap that does not accept
/// `--disable-userns` and `--assert-userns-disabled` fails `probe_bubblewrap`, which withholds
/// `exec.namespaces` and answers every exec `exec.sandbox-unavailable` — a correctly named refusal
/// (invariant 3) whose cause is nowhere in the list the operator was given.
#[test]
fn the_deployment_list_names_the_backend_options_exec_now_requires() {
    let readme = document("README.md");
    let serving_exec = block(
        &readme,
        |line| line.trim() == "### Serving exec",
        |line| {
            line.starts_with("## ")
                || (line.starts_with("### ") && line.trim() != "### Serving exec")
        },
    );
    assert!(
        serving_exec.contains("socat"),
        "README § Serving exec was located but does not read like the deployment list: \
         {serving_exec}"
    );

    assert!(
        states_the_clause(&serving_exec),
        "the list says only \"provide the configured bubblewrap binary\", which is no longer \
         sufficient: `probe_bubblewrap` puts `--disable-userns` and `--assert-userns-disabled` in \
         its argv (probe.rs:336, :381) and a backend that refuses either leaves every `exec` fact \
         absent. An operator whose distribution ships a bubblewrap without those options loses \
         exec entirely and this list gives no reason. Section read:\n{serving_exec}"
    );
}
