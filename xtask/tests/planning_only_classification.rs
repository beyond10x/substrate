//! Offline assertions for the planning-only classification of a pull request.
//!
//! `.github/workflows/classify-change.yml` holds one step that reads the changed paths of a pull
//! request against `.github/planning-only-paths` and answers `planning-only` or `code`. `gate.yml`
//! and `macos.yml` call it as their first job. These tests execute that step's own script against
//! throwaway Git repositories, and the `Full gate` step's own script against every combination of
//! results its jobs can report, so a pull request that changes anything outside the list still
//! needs both gate phases green. Nothing here reaches the network or builds anything.
//!
//! The last three cases hold the repository's second path list, the two patterns the
//! code-scanning analysis skips (`.github/codeql/codeql-config.yml`), to test code: the file is
//! those two patterns and nothing else, and no path they match is compiled into a shipped binary.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

const LIST: &str = ".github/planning-only-paths";
const CLASSIFIER: &str = ".github/workflows/classify-change.yml";
const CLASSIFY_STEP: &str = "Classify the changed paths";
const GATE_STEP: &str = "Require both gate phases";
const CALL: &str = "    uses: ./.github/workflows/classify-change.yml\n";
const NEEDS: &str = "    needs: [classify]\n";
const SKIP: &str =
    "    if: ${{ !cancelled() && needs.classify.outputs.change != 'planning-only' }}\n";
const ABSENT: &str = "0000000000000000000000000000000000000000";
/// Listed Markdown that Rust source names and that stays planning-only: `check-adrs` alone reads
/// the ADR index, every new ADR adds a row to it, and a wrong row is the prose-check hazard
/// `AGENTS.md` accepts on a pull request, caught by the push run on `main`.
const PROSE_CHECK_ONLY: [&str; 1] = ["adr/README.md"];

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask has a repository parent")
        .to_path_buf()
}

/// A file of this repository, read when the test runs.
fn repository_file(relative: &str) -> String {
    fs::read_to_string(repository_root().join(relative))
        .unwrap_or_else(|error| panic!("the repository has no readable {relative}: {error}"))
}

/// The text of one job of `workflow`, from the line after its `  <id>:` line to the next job.
fn job(workflow: &str, id: &str) -> String {
    let text = repository_file(workflow);
    let header = format!("\n  {id}:\n");
    let start = text
        .find(&header)
        .unwrap_or_else(|| panic!("{workflow} has no job {id}"))
        + header.len();
    let body = &text[start..];
    let end = body
        .match_indices('\n')
        .map(|(index, _)| index)
        .find(|&index| {
            let line = &body[index + 1..];
            line.starts_with("  ") && !line.starts_with("   ")
        })
        .map_or(body.len(), |index| index + 1);
    body[..end].to_owned()
}

/// The `run: |` body of the named step of `workflow`, dedented, as the runner would execute it.
fn step_script(workflow: &str, name: &str) -> String {
    let text = repository_file(workflow);
    let marker = format!("- name: {name}\n");
    let start = text
        .find(&marker)
        .unwrap_or_else(|| panic!("{workflow} has no step {name:?}"));
    let mut lines = text[start..].lines().skip(1);
    for line in lines.by_ref() {
        if line.trim() == "run: |" {
            break;
        }
        assert!(
            !line.trim_start().starts_with("- name:"),
            "step {name:?} of {workflow} has no `run: |` block"
        );
    }
    let mut body = Vec::new();
    let mut indent = None;
    for line in lines {
        if line.trim().is_empty() {
            body.push(String::new());
            continue;
        }
        let width = line.len() - line.trim_start().len();
        let indent = *indent.get_or_insert(width);
        if width < indent {
            break;
        }
        body.push(line[indent..].to_owned());
    }
    assert!(
        !body.is_empty(),
        "step {name:?} of {workflow} has an empty `run: |` block"
    );
    body.join("\n") + "\n"
}

fn git(directory: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(directory)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args([
            "-c",
            "user.name=classification-test",
            "-c",
            "user.email=classification-test@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "init.defaultBranch=main",
            "-c",
            "protocol.file.allow=always",
        ])
        .args(args)
        .output()
        .expect("git runs");
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .expect("utf-8 git output")
        .trim()
        .to_owned()
}

/// What the base commit of every throwaway repository tracks beside the list: one file per kind
/// of path the cases change, rename or delete.
const TRACKED: [(&str, &str); 13] = [
    (
        ".engineering/planning/story/first.md",
        "---\nstatus: open\n---\n\n# First\n",
    ),
    ("docs/design/01-example.md", "# Example design\n\nA body.\n"),
    ("adr/9999-example.md", "# Example decision\n"),
    ("ROADMAP.md", "# Roadmap\n"),
    ("STATUS.md", "# Status\n"),
    ("README.md", "# Readme\n"),
    ("CHANGELOG.md", "# Changelog\n"),
    ("website/docs/page.md", "# Page\n"),
    (
        "contracts/substrate-wire/0.1.0/README.md",
        "# Bundle 0.1.0\n",
    ),
    (
        "crates/example/src/lib.rs",
        "pub fn answer() -> u8 {\n    42\n}\n",
    ),
    ("crates/example/src/old.rs", "pub const OLD: u8 = 1;\n"),
    ("crates/example/README.md", "# Example crate\n"),
    (".github/workflows/gate.yml", "name: Gate\n"),
];

/// The step's answer: what it wrote to `GITHUB_OUTPUT` and everything it printed.
struct Answer {
    change: String,
    log: String,
}

impl Answer {
    fn expect(&self, change: &str, case: &str) {
        assert_eq!(self.change, change, "{case}:\n{}", self.log);
        assert!(
            self.log.contains(&format!("change: {change}")),
            "{case}: the log does not state the answer:\n{}",
            self.log
        );
    }

    /// The class the step printed for one changed path.
    fn class_of(&self, path: &str) -> &str {
        self.log
            .lines()
            .find_map(|line| {
                let (class, printed) = line.trim().split_once(char::is_whitespace)?;
                (printed.trim() == path).then_some(class)
            })
            .unwrap_or_else(|| panic!("the step did not print {path}:\n{}", self.log))
    }
}

/// A throwaway repository whose `main` holds [`TRACKED`] and a path list, isolated from every
/// user-level and system-level Git configuration and hook.
struct Checkout {
    directory: tempfile::TempDir,
}

impl Checkout {
    /// `main` holds this repository's own list.
    fn new() -> Self {
        Self::with_list(Some(&repository_file(LIST)))
    }

    fn with_list(list: Option<&str>) -> Self {
        let checkout = Self {
            directory: tempfile::tempdir().expect("temporary repository"),
        };
        git(checkout.root(), &["init", "--quiet"]);
        for (path, contents) in TRACKED {
            checkout.write(path, contents);
        }
        if let Some(list) = list {
            checkout.write(LIST, list);
        }
        checkout.commit("base");
        checkout
    }

    fn root(&self) -> &Path {
        self.directory.path()
    }

    fn write(&self, path: &str, contents: &str) {
        let file = self.root().join(path);
        fs::create_dir_all(file.parent().expect("a tracked file has a parent")).expect("tree");
        fs::write(file, contents).expect("file");
    }

    fn git(&self, args: &[&str]) -> String {
        git(self.root(), args)
    }

    fn commit(&self, message: &str) -> String {
        self.git(&["add", "--all"]);
        self.git(&["commit", "--quiet", "--allow-empty", "-m", message]);
        self.git(&["rev-parse", "HEAD"])
    }

    /// A branch from `main` that `change` edits; returns `main`'s commit and the branch's.
    fn pull_request(&self, change: impl FnOnce(&Self)) -> (String, String) {
        let base = self.git(&["rev-parse", "main"]);
        self.git(&["checkout", "--quiet", "--detach", "main"]);
        change(self);
        let head = self.commit("pull request");
        self.git(&["checkout", "--quiet", "main"]);
        (base, head)
    }

    /// The answer for a pull request whose only change is `change`.
    fn answer_for(&self, change: impl FnOnce(&Self)) -> Answer {
        let (base, head) = self.pull_request(change);
        self.classify("pull_request", &base, &head)
    }

    fn classify(&self, event: &str, base: &str, head: &str) -> Answer {
        self.classify_with(event, base, head, &[])
    }

    /// Runs the classifying step in this repository. The step always ends with an answer, so a
    /// failing step, or one that writes anything but one `change=` line, fails the case.
    fn classify_with(&self, event: &str, base: &str, head: &str, env: &[(&str, &str)]) -> Answer {
        classify_in(self.root(), event, base, head, env)
    }
}

fn classify_in(root: &Path, event: &str, base: &str, head: &str, env: &[(&str, &str)]) -> Answer {
    let scratch = tempfile::tempdir().expect("runner scratch");
    let output_file = scratch.path().join("github-output");
    fs::write(&output_file, "").expect("GITHUB_OUTPUT");
    let mut command = Command::new("bash");
    command
        .current_dir(root)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("RUNNER_TEMP", scratch.path())
        .env("GITHUB_OUTPUT", &output_file)
        .env("EVENT_NAME", event)
        .env("BASE_SHA", base)
        .env("HEAD_SHA", head)
        .args([
            "--noprofile",
            "--norc",
            "-e",
            "-o",
            "pipefail",
            "-c",
            &step_script(CLASSIFIER, CLASSIFY_STEP),
        ]);
    for (key, value) in env {
        command.env(key, value);
    }
    let output = command.output().expect("bash runs");
    let log = String::from_utf8_lossy(&output.stdout).into_owned()
        + &String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "the classifying step failed instead of answering:\n{log}"
    );
    let written = fs::read_to_string(&output_file).expect("GITHUB_OUTPUT readable");
    let change = match written.as_str() {
        "change=planning-only\n" => "planning-only",
        "change=code\n" => "code",
        other => panic!("the step wrote {other:?} to GITHUB_OUTPUT:\n{log}"),
    };
    Answer {
        change: change.to_owned(),
        log,
    }
}

/// The story's first acceptance case: planning files, a design document, an ADR and the roadmap,
/// changed and added, and nothing else.
#[test]
fn a_pull_request_that_changes_only_listed_paths_is_planning_only() {
    let checkout = Checkout::new();
    let answer = checkout.answer_for(|pr| {
        pr.write(
            ".engineering/planning/story/first.md",
            "---\nstatus: active\n---\n",
        );
        pr.write(
            ".engineering/planning/story/second.md",
            "---\nstatus: open\n---\n",
        );
        pr.write(".engineering/evidence/story/second/record.json", "{}\n");
        pr.write(
            "docs/design/01-example.md",
            "# Example design\n\nRevised.\n",
        );
        pr.write("docs/design/02 spaced name.md", "# Spaced\n");
        pr.write("docs/design/03-überblick.md", "# Overview\n");
        pr.write("adr/9999-example.md", "# Example decision, amended\n");
        pr.write("ROADMAP.md", "# Roadmap\n\nObserved.\n");
    });
    answer.expect("planning-only", "only listed paths");
    for path in [
        ".engineering/planning/story/first.md",
        ".engineering/planning/story/second.md",
        ".engineering/evidence/story/second/record.json",
        "docs/design/01-example.md",
        "adr/9999-example.md",
        "ROADMAP.md",
    ] {
        assert_eq!(answer.class_of(path), "planning-only", "{path}");
    }
}

/// `STATUS.md` left the list when `cargo xtask check-docs` began to hold its release lines to the
/// workspace version (`xtask/src/site_release.rs`). A pull request that changes only it can turn
/// that check red, so it runs the gate; beside a listed path it is still the one code path.
#[test]
fn a_pull_request_that_changes_only_the_status_file_is_code() {
    let checkout = Checkout::new();
    let alone = checkout.answer_for(|pr| pr.write("STATUS.md", "# Status\n\nObserved.\n"));
    alone.expect("code", "only STATUS.md");
    assert_eq!(alone.class_of("STATUS.md"), "code");
    let beside = checkout.answer_for(|pr| {
        pr.write("STATUS.md", "# Status\n\nObserved again.\n");
        pr.write("ROADMAP.md", "# Roadmap\n\nObserved.\n");
    });
    beside.expect("code", "STATUS.md beside a listed path");
    assert_eq!(beside.class_of("STATUS.md"), "code");
    assert_eq!(beside.class_of("ROADMAP.md"), "planning-only");
}

/// The story's second acceptance case: the same pull request with one `.rs` file added.
#[test]
fn one_rust_file_beside_listed_paths_makes_the_pull_request_code() {
    let checkout = Checkout::new();
    let answer = checkout.answer_for(|pr| {
        pr.write(
            ".engineering/planning/story/first.md",
            "---\nstatus: active\n---\n",
        );
        pr.write(
            "docs/design/01-example.md",
            "# Example design\n\nRevised.\n",
        );
        pr.write("crates/example/src/added.rs", "pub const ADDED: u8 = 2;\n");
    });
    answer.expect("code", "one .rs file added");
    assert_eq!(answer.class_of("crates/example/src/added.rs"), "code");
    assert_eq!(
        answer.class_of(".engineering/planning/story/first.md"),
        "planning-only"
    );
    assert_eq!(
        answer.class_of("docs/design/01-example.md"),
        "planning-only"
    );
}

/// The story's third acceptance case. The file keeps its bytes, so Git's rename detection would
/// report one renamed path; the step reads both sides.
#[test]
fn a_rename_out_of_a_listed_path_is_code() {
    let checkout = Checkout::new();
    let answer = checkout.answer_for(|pr| {
        pr.git(&[
            "mv",
            "docs/design/01-example.md",
            "crates/example/src/design.rs",
        ]);
    });
    answer.expect("code", "a listed path renamed to a code path");
    assert_eq!(
        answer.class_of("docs/design/01-example.md"),
        "planning-only"
    );
    assert_eq!(answer.class_of("crates/example/src/design.rs"), "code");
}

/// The other side: a code file that leaves the build under a listed name. A diff that names only
/// the destination of a rename would see one listed path here.
#[test]
fn a_rename_into_a_listed_path_is_code() {
    let checkout = Checkout::new();
    let answer = checkout.answer_for(|pr| {
        pr.git(&["mv", "crates/example/src/old.rs", "docs/design/old.md"]);
        pr.write(
            ".engineering/planning/story/first.md",
            "---\nstatus: active\n---\n",
        );
    });
    answer.expect("code", "a code path renamed into a listed path");
    assert_eq!(answer.class_of("crates/example/src/old.rs"), "code");
    assert_eq!(answer.class_of("docs/design/old.md"), "planning-only");
}

#[test]
fn a_copy_of_a_listed_file_to_a_code_path_is_code() {
    let checkout = Checkout::new();
    let answer = checkout.answer_for(|pr| {
        pr.write(
            "crates/example/src/copied.rs",
            "# Example design\n\nA body.\n",
        );
    });
    answer.expect("code", "a listed file copied to a code path");
}

#[test]
fn a_deleted_code_file_is_code() {
    let checkout = Checkout::new();
    let answer = checkout.answer_for(|pr| {
        pr.git(&["rm", "--quiet", "crates/example/src/old.rs"]);
        pr.write("ROADMAP.md", "# Roadmap\n\nObserved.\n");
    });
    answer.expect("code", "a deleted code file");
    assert_eq!(answer.class_of("crates/example/src/old.rs"), "code");
}

/// One changed path per pull request: a page of the built site, the changelog, a released bundle
/// file, a bundle source, a workflow, the list itself and the two root documents tests read.
#[test]
fn the_built_site_the_changelog_a_released_bundle_and_a_workflow_are_each_code() {
    let checkout = Checkout::new();
    for path in [
        "website/docs/page.md",
        "CHANGELOG.md",
        "contracts/substrate-wire/0.1.0/README.md",
        "xtask/bundle-source/9.9.9/README.md",
        ".github/workflows/gate.yml",
        LIST,
        "README.md",
        "AGENTS.md",
    ] {
        checkout
            .answer_for(|pr| pr.write(path, "changed by the pull request\n"))
            .expect("code", path);
    }
}

/// The list names prose trees, so a Markdown file anywhere else is code: the built site, the
/// changelog, released bundles and their sources, crate READMEs, vendored and specification
/// sources, every workflow file, and a name that only resembles a listed one. One pull request
/// changes them all, and the step prints each with its class.
#[test]
fn every_path_outside_the_list_is_code() {
    let checkout = Checkout::new();
    let paths = [
        "website/docs/page.md",
        "website/ux/findings/UXF-999.md",
        "CHANGELOG.md",
        "contracts/substrate-wire/0.1.0/README.md",
        "contracts/substrate-wire/9.9.9/notes.md",
        "xtask/bundle-source/9.9.9/README.md",
        ".github/workflows/gate.yml",
        ".github/workflows/notes.md",
        LIST,
        "README.md",
        "AGENTS.md",
        "STATUS.md",
        "crates/example/README.md",
        "crates/example/docs/notes.md",
        "vendor/example-1.0.0/src/docs/routing.md",
        "spec/generated/cli/README.md",
        "deploy/seccomp/README.md",
        "scripts/notes.md",
        "notes.md",
        "Cargo.lock",
        "docs/design/diagram.svg",
        "docs/design/01-example.md.rs",
        "adr/9998-example.mdx",
        "xdocs/notes.md",
        ".engineering-notes/notes.md",
        ".engineering.md",
    ];
    let answer = checkout.answer_for(|pr| {
        for path in paths {
            pr.write(path, "changed by the pull request\n");
        }
    });
    answer.expect("code", "every path outside the list");
    for path in paths {
        assert_eq!(answer.class_of(path), "code", "{path}");
    }
}

/// Every kind of path the list names, changed by one pull request.
#[test]
fn every_listed_path_is_planning_only() {
    let checkout = Checkout::new();
    let paths = [
        ".engineering/planning/story/first.md",
        ".engineering/planning/epic/new.md",
        ".engineering/evidence/story/first/record.json",
        ".engineering/project.yaml",
        "docs/design/01-example.md",
        "docs/plan/01-example.md",
        "docs/reviews/archived/review.md",
        "docs/VISION.md",
        "adr/9999-example.md",
        "adr/README.md",
        "architecture/overview.md",
        "ROADMAP.md",
        "SECURITY.md",
        "glossary.md",
    ];
    let answer = checkout.answer_for(|pr| {
        for path in paths {
            pr.write(path, "changed by the pull request\n");
        }
    });
    answer.expect("planning-only", "every kind of listed path");
    for path in paths {
        assert_eq!(answer.class_of(path), "planning-only", "{path}");
    }
}

#[test]
fn a_pull_request_without_a_changed_path_is_code() {
    let checkout = Checkout::new();
    let main = checkout.git(&["rev-parse", "main"]);
    checkout
        .classify("pull_request", &main, &main)
        .expect("code", "head is base");
    checkout
        .answer_for(|_| {})
        .expect("code", "one commit that changes no path");
    let (base, head) = checkout.pull_request(|pr| {
        pr.write("crates/example/src/lib.rs", "pub fn answer() {}\n");
        pr.commit("change");
        pr.write(
            "crates/example/src/lib.rs",
            "pub fn answer() -> u8 {\n    42\n}\n",
        );
    });
    checkout
        .classify("pull_request", &base, &head)
        .expect("code", "a change and its exact reversal");
}

/// `release.yml` reads `gate.yml`'s `success` for the tagged commit from a `push` run on `main` or
/// a dispatch, so those runs are always the full gate, whatever the commit changed.
#[test]
fn a_push_and_a_dispatch_never_skip() {
    let checkout = Checkout::new();
    let (base, head) = checkout.pull_request(|pr| {
        pr.write(
            ".engineering/planning/story/first.md",
            "---\nstatus: active\n---\n",
        );
    });
    checkout
        .classify("pull_request", &base, &head)
        .expect("planning-only", "the same commits as a pull request");
    for event in [
        "push",
        "workflow_dispatch",
        "pull_request_target",
        "merge_group",
        "schedule",
        "Pull_Request",
        "pull_request ",
        "",
    ] {
        checkout
            .classify(event, &base, &head)
            .expect("code", &format!("event {event:?}"));
    }
    for event in ["push", "workflow_dispatch"] {
        checkout
            .classify(event, "", "")
            .expect("code", &format!("event {event:?} without commits"));
    }
}

#[test]
fn a_base_or_head_the_checkout_does_not_hold_is_code() {
    let checkout = Checkout::new();
    let (base, head) = checkout.pull_request(|pr| pr.write("ROADMAP.md", "# Roadmap\n\nMore.\n"));
    checkout
        .classify("pull_request", &base, &head)
        .expect("planning-only", "both commits present");
    let tree = checkout.git(&["rev-parse", "main^{tree}"]);
    for (base, head, what) in [
        (ABSENT, head.as_str(), "an absent base"),
        (base.as_str(), ABSENT, "an absent head"),
        ("", head.as_str(), "no base"),
        (base.as_str(), "", "no head"),
        ("main", head.as_str(), "a base that is a branch name"),
        (base.as_str(), "HEAD", "a head that is a ref name"),
        (&base[..12], head.as_str(), "an abbreviated base"),
        (tree.as_str(), head.as_str(), "a base that is a tree"),
        (base.as_str(), tree.as_str(), "a head that is a tree"),
        ("--all", head.as_str(), "a base that is an option"),
    ] {
        checkout
            .classify("pull_request", base, head)
            .expect("code", what);
    }
}

#[test]
fn commits_without_a_merge_base_are_code() {
    let checkout = Checkout::new();
    let base = checkout.git(&["rev-parse", "main"]);
    checkout.git(&["checkout", "--quiet", "--orphan", "unrelated"]);
    checkout.git(&["rm", "--quiet", "-r", "--cached", "."]);
    checkout.write("ROADMAP.md", "# Roadmap of another history\n");
    checkout.git(&["add", "ROADMAP.md"]);
    checkout.git(&["commit", "--quiet", "-m", "unrelated"]);
    let head = checkout.git(&["rev-parse", "HEAD"]);
    checkout
        .classify("pull_request", &base, &head)
        .expect("code", "unrelated histories");
}

/// A checkout that is not the whole history cannot be trusted to hold the merge base, so it is
/// code even when the two commits it was given happen to be present.
#[test]
fn a_shallow_checkout_is_code() {
    let checkout = Checkout::new();
    checkout.commit("a second commit, so the history is longer than the clone");
    let (base, head) = checkout.pull_request(|pr| pr.write("ROADMAP.md", "# Roadmap\n\nMore.\n"));
    checkout.git(&["branch", "pull", &head]);
    checkout
        .classify("pull_request", &base, &head)
        .expect("planning-only", "the full history");
    let shallow = tempfile::tempdir().expect("shallow clone");
    let source = format!("file://{}", checkout.root().display());
    git(
        shallow.path(),
        &[
            "clone", "--quiet", "--depth", "2", "--branch", "pull", &source, "clone",
        ],
    );
    let clone = shallow.path().join("clone");
    assert_eq!(
        git(&clone, &["rev-parse", "--is-shallow-repository"]),
        "true"
    );
    git(&clone, &["cat-file", "-e", &format!("{base}^{{commit}}")]);
    classify_in(&clone, "pull_request", &base, &head, &[])
        .expect("code", "a shallow clone that holds both commits");
}

#[test]
fn a_missing_or_malformed_list_is_code() {
    for (list, case) in [
        (None, "no list on the base"),
        (Some(""), "an empty list"),
        (Some("# only a comment\n"), "a list without a pattern"),
        (
            Some("exclude website/*\n"),
            "a list that only excludes paths",
        ),
        (
            Some("include ROADMAP.md\nallow docs/*\n"),
            "an unknown directive",
        ),
        (
            Some("include ROADMAP.md\nROADMAP.md\n"),
            "a pattern without a directive",
        ),
        (
            Some("include ROADMAP.md\ninclude docs/?.md\n"),
            "a `?` wildcard",
        ),
        (
            Some("include ROADMAP.md\ninclude docs/[a-z]*.md\n"),
            "a bracket expression",
        ),
        (
            Some("include ROADMAP.md\ninclude @(docs|adr)/*\n"),
            "an extended pattern",
        ),
        (
            Some("include ROADMAP.md\ninclude docs/ *.md\n"),
            "a pattern with a space",
        ),
        (Some("include ROADMAP.md\ninclude \n"), "an empty pattern"),
        (
            Some("include ROADMAP.md\r\ninclude docs/*.md\r\n"),
            "carriage returns",
        ),
    ] {
        let checkout = Checkout::with_list(list);
        checkout
            .answer_for(|pr| pr.write("ROADMAP.md", "# Roadmap\n\nMore.\n"))
            .expect("code", case);
    }
    let checkout = Checkout::with_list(Some("# a list\n\ninclude ROADMAP.md\nexclude docs/*\n"));
    checkout
        .answer_for(|pr| pr.write("ROADMAP.md", "# Roadmap\n\nMore.\n"))
        .expect("planning-only", "a well-formed list that names the path");
}

/// The list that classifies a pull request is the one its base branch holds, so a pull request
/// cannot add its own paths to the list, and the list itself is never a listed path.
#[test]
fn a_pull_request_cannot_list_its_own_paths() {
    let checkout = Checkout::new();
    let widened = repository_file(LIST) + "include crates/*\ninclude .github/*\n";
    checkout
        .answer_for(|pr| {
            pr.write(LIST, &widened);
            pr.write("crates/example/src/added.rs", "pub const ADDED: u8 = 2;\n");
        })
        .expect("code", "a pull request that lists its own code path");
    checkout
        .answer_for(|pr| pr.write(LIST, &widened))
        .expect("code", "a pull request that changes only the list");
}

/// The changed paths are the pull request's own, read against its merge base: what `main` gained
/// since the branch point is not the pull request's change, in either direction.
#[test]
fn commits_main_gained_since_the_branch_point_are_not_the_pull_requests_change() {
    let checkout = Checkout::new();
    let (_, planning) = checkout.pull_request(|pr| pr.write("ROADMAP.md", "# Roadmap\n\nMore.\n"));
    let (_, code) = checkout.pull_request(|pr| {
        pr.write("crates/example/src/added.rs", "pub const ADDED: u8 = 2;\n");
    });
    checkout.write("crates/example/src/lib.rs", "pub fn answer() {}\n");
    checkout.write("docs/design/01-example.md", "# Example design, on main\n");
    let main = checkout.commit("main moves on");
    let answer = checkout.classify("pull_request", &main, &planning);
    answer.expect("planning-only", "main gained a code change");
    assert!(
        !answer.log.contains("crates/example/src/lib.rs"),
        "the step read main's own change as the pull request's:\n{}",
        answer.log
    );
    checkout
        .classify("pull_request", &main, &code)
        .expect("code", "a code pull request behind main");
    checkout
        .classify("pull_request", &planning, &main)
        .expect("code", "base and head exchanged");
}

/// The step passes every option its diff needs: a runner whose Git configuration detects copies,
/// quotes paths or hides submodule changes still gets both sides of a rename and every changed
/// path.
#[test]
fn the_answer_does_not_depend_on_the_runners_git_configuration() {
    let checkout = Checkout::new();
    let configuration = checkout.root().join(".git/hostile-config");
    fs::write(
        &configuration,
        "[diff]\n\trenames = copies\n\tnoprefix = true\n\trelative = true\n\tignoreSubmodules = all\n[core]\n\tquotePath = true\n\tfileMode = false\n",
    )
    .expect("hostile configuration");
    let configuration = configuration.display().to_string();
    let env = [("GIT_CONFIG_GLOBAL", configuration.as_str())];
    let (base, head) = checkout.pull_request(|pr| {
        pr.git(&["mv", "crates/example/src/old.rs", "docs/design/old.md"]);
    });
    checkout
        .classify_with("pull_request", &base, &head, &env)
        .expect("code", "a rename into a listed path");
    let (base, head) = checkout.pull_request(|pr| {
        pr.write("docs/design/03-überblick.md", "# Overview\n");
        pr.write("docs/design/04 \"quoted\".md", "# Quoted\n");
    });
    checkout
        .classify_with("pull_request", &base, &head, &env)
        .expect("planning-only", "listed paths Git would quote");
    // A submodule pointer and a mode change, each beside a listed path, with no file content
    // changed outside the list.
    checkout.git(&["checkout", "--quiet", "--detach", "main"]);
    checkout.write("ROADMAP.md", "# Roadmap\n\nMore.\n");
    checkout.git(&["add", "ROADMAP.md"]);
    checkout.git(&[
        "update-index",
        "--add",
        "--cacheinfo",
        &format!("160000,{base},crates/example/submodule"),
    ]);
    checkout.git(&["commit", "--quiet", "-m", "a submodule pointer"]);
    let pointer = checkout.git(&["rev-parse", "HEAD"]);
    checkout.git(&["reset", "--quiet", "--hard", "main"]);
    checkout.write("ROADMAP.md", "# Roadmap\n\nMore.\n");
    checkout.git(&["add", "ROADMAP.md"]);
    checkout.git(&["update-index", "--chmod=+x", "crates/example/src/lib.rs"]);
    checkout.git(&["commit", "--quiet", "-m", "a mode change"]);
    let mode = checkout.git(&["rev-parse", "HEAD"]);
    checkout.git(&["reset", "--quiet", "--hard", "main"]);
    checkout.git(&["checkout", "--quiet", "main"]);
    for (head, what) in [(&pointer, "a submodule pointer"), (&mode, "a mode change")] {
        let answer = checkout.classify_with("pull_request", &base, head, &env);
        answer.expect("code", what);
    }
}

/// A diff the step cannot compute is code: `git` here fails every `diff` and nothing else.
#[test]
fn a_diff_that_cannot_be_computed_is_code() {
    let checkout = Checkout::new();
    let (base, head) = checkout.pull_request(|pr| pr.write("ROADMAP.md", "# Roadmap\n\nMore.\n"));
    checkout
        .classify("pull_request", &base, &head)
        .expect("planning-only", "the real git");
    let tools = tempfile::tempdir().expect("stub directory");
    let stub = tools.path().join("git");
    let real = String::from_utf8(
        Command::new("bash")
            .args(["-c", "command -v git"])
            .output()
            .expect("bash runs")
            .stdout,
    )
    .expect("utf-8");
    fs::write(
        &stub,
        format!(
            "#!/bin/bash\nfor argument in \"$@\"; do\n  if [ \"$argument\" = diff ]; then\n    echo 'fatal: stub diff' >&2\n    exit 128\n  fi\ndone\nexec {} \"$@\"\n",
            real.trim()
        ),
    )
    .expect("stub");
    let mut permissions = fs::metadata(&stub).expect("stub metadata").permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o755);
    fs::set_permissions(&stub, permissions).expect("stub mode");
    let path = format!(
        "{}:{}",
        tools.path().display(),
        std::env::var("PATH").expect("PATH")
    );
    checkout
        .classify_with("pull_request", &base, &head, &[("PATH", path.as_str())])
        .expect("code", "a failing diff");
}

/// A path cannot write the step's output or a workflow command: every path is printed quoted, on
/// one line that begins with its class.
#[test]
fn a_changed_path_cannot_forge_the_answer() {
    let checkout = Checkout::new();
    let answer = checkout.answer_for(|pr| {
        pr.write(
            "docs/design/a\nchange: planning-only\n::notice::forged\nchange=planning-only\n.md",
            "# Forged\n",
        );
        pr.write("crates/example/src/added.rs", "pub const ADDED: u8 = 2;\n");
    });
    answer.expect("code", "a path that spells an answer");
    assert!(
        !answer
            .log
            .lines()
            .any(|line| line.trim_start().starts_with("::")
                || line.trim() == "change: planning-only"),
        "a path wrote a line of its own:\n{}",
        answer.log
    );
}

fn uses(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|line| line.trim().strip_prefix("uses: "))
        .map(|action| {
            action
                .split_whitespace()
                .next()
                .expect("an action reference")
                .to_owned()
        })
        .collect()
}

/// The classifying workflow checks out the whole history and runs one script: no toolchain, no
/// cache, no build. The event and both commits reach the script as environment, never as text.
#[test]
fn the_classifying_workflow_reads_history_and_builds_nothing() {
    let workflow = repository_file(CLASSIFIER);
    assert!(workflow.contains("\non:\n  workflow_call:\n    outputs:\n      change:\n"));
    assert!(workflow.contains("        value: ${{ jobs.classify.outputs.change }}\n"));
    let classify = job(CLASSIFIER, "classify");
    assert!(
        classify.contains("    outputs:\n      change: ${{ steps.classify.outputs.change }}\n")
    );
    assert!(classify.contains("        id: classify\n"));
    assert!(classify.contains("    permissions:\n      contents: read\n"));
    assert!(!classify.contains("continue-on-error"));
    assert!(classify.contains("          fetch-depth: 0\n"));
    assert!(classify.contains("          persist-credentials: false\n"));
    assert!(classify.contains("        if: github.event_name == 'pull_request'\n"));
    assert!(classify.contains("          EVENT_NAME: ${{ github.event_name }}\n"));
    assert!(classify.contains("          BASE_SHA: ${{ github.event.pull_request.base.sha }}\n"));
    assert!(classify.contains("          HEAD_SHA: ${{ github.event.pull_request.head.sha }}\n"));
    assert!(classify.contains("        shell: bash\n"));
    let script = step_script(CLASSIFIER, CLASSIFY_STEP);
    assert!(
        !script.contains("${{"),
        "the script interpolates an expression"
    );
    assert!(
        script.contains(&format!("list={LIST}\n")),
        "the script reads another list"
    );
    for tool in ["cargo", "rustup", "rust-cache", "rustc", "npm", "docker"] {
        assert!(!workflow.contains(tool), "the classifier names {tool}");
    }
    let actions = uses(&workflow);
    assert_eq!(actions.len(), 1, "{actions:?}");
    let gate_checkout = uses(&repository_file(".github/workflows/gate.yml"))
        .into_iter()
        .find(|action| action.starts_with("actions/checkout@"))
        .expect("gate.yml checks out");
    assert_eq!(actions[0], gate_checkout);
    let (_, revision) = actions[0].rsplit_once('@').expect("a pinned action");
    assert_eq!(revision.len(), 40);
    assert!(revision.bytes().all(|byte| byte.is_ascii_hexdigit()));
}

/// The three build and test jobs of `gate.yml` wait for the classification and skip only for the
/// exact answer `planning-only`; a failed or absent classification runs them.
#[test]
fn gate_yml_classifies_first_and_skips_its_build_jobs_only_for_planning_only() {
    let gate = repository_file(".github/workflows/gate.yml");
    assert!(
        gate.contains("\njobs:\n  classify:\n"),
        "classify is not gate.yml's first job"
    );
    let classify = job(".github/workflows/gate.yml", "classify");
    assert!(classify.contains(CALL));
    assert!(
        !classify.contains("    if:"),
        "the classification is conditional"
    );
    for id in ["gate-tests", "gate-static", "pod-lane"] {
        let body = job(".github/workflows/gate.yml", id);
        assert!(
            body.starts_with(&format!("{NEEDS}{SKIP}")),
            "{id} does not wait for the classification"
        );
        assert_eq!(body.matches("\n    if:").count(), 1, "{id} has a second if");
        assert!(!body.contains("continue-on-error"), "{id}");
    }
    assert!(!gate.contains("continue-on-error"));
    assert!(!gate.contains("paths:") && !gate.contains("paths-ignore:"));
    assert_eq!(gate.matches("needs.classify.outputs.change").count(), 4);
    let jobs: Vec<&str> = gate
        .lines()
        .skip_while(|line| *line != "jobs:")
        .skip(1)
        .filter(|line| line.starts_with("  ") && !line.starts_with("   ") && line.ends_with(':'))
        .map(str::trim)
        .collect();
    assert_eq!(
        jobs,
        [
            "classify:",
            "gate-tests:",
            "gate-static:",
            "gate:",
            "pod-lane:"
        ]
    );
}

/// `Full gate` reads what the classification and both phases concluded.
#[test]
fn the_full_gate_reads_the_classification_and_both_phases() {
    let full = job(".github/workflows/gate.yml", "gate");
    assert!(full.contains("    name: Full gate\n"));
    assert!(full.contains("    needs: [classify, gate-tests, gate-static]\n"));
    assert!(full.contains("    if: always()\n"));
    assert!(!full.contains("continue-on-error"));
    for binding in [
        "EVENT: ${{ github.event_name }}",
        "CLASSIFY: ${{ needs.classify.result }}",
        "CHANGE: ${{ needs.classify.outputs.change }}",
        "TESTS: ${{ needs.gate-tests.result }}",
        "STATIC: ${{ needs.gate-static.result }}",
    ] {
        assert!(
            full.contains(&format!("          {binding}\n")),
            "Full gate does not bind {binding}"
        );
    }
    assert_eq!(full.matches("- name: ").count(), 1);
    assert!(!step_script(".github/workflows/gate.yml", GATE_STEP).contains("${{"));
}

/// The `Full gate` step's own script, executed for every combination its inputs can take. It is
/// green for two green phases, and for a pull request classified `planning-only` whose phases were
/// both skipped; nothing else is green. A skipped or failed phase on a `code` change, on a push or
/// on a dispatch fails it, and so does a phase skipped for any reason but the classification.
#[test]
fn the_full_gate_is_green_only_for_two_green_phases_or_a_planning_only_pull_request() {
    let script = step_script(".github/workflows/gate.yml", GATE_STEP);
    let results = ["success", "failure", "cancelled", "skipped"];
    // One thread per event; each returns its combinations, its green ones and its skipped ones.
    let counts = std::thread::scope(|scope| {
        let handles: Vec<_> = ["pull_request", "push", "workflow_dispatch"]
            .into_iter()
            .map(|event| {
                let script = &script;
                scope.spawn(move || {
                    let mut counts = [0_usize; 3];
                    for classify in results {
                        for change in ["planning-only", "code", ""] {
                            for tests in results {
                                for checks in results {
                                    let green =
                                        full_gate(script, event, classify, change, tests, checks);
                                    counts[0] += 1;
                                    counts[1] += usize::from(green);
                                    counts[2] += usize::from(green && tests == "skipped");
                                }
                            }
                        }
                    }
                    counts
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().expect("a combination failed"))
            .fold([0_usize; 3], |sum, counts| {
                [sum[0] + counts[0], sum[1] + counts[1], sum[2] + counts[2]]
            })
    });
    assert_eq!(counts[0], 3 * 4 * 3 * 4 * 4, "combinations");
    assert_eq!(counts[1], 3 * 4 * 3 + 1, "green combinations");
    assert_eq!(counts[2], 1, "green combinations with skipped phases");
}

/// Runs the `Full gate` step's script for one combination, holds its result to the rule, and
/// returns whether it was green.
fn full_gate(
    script: &str,
    event: &str,
    classify: &str,
    change: &str,
    tests: &str,
    checks: &str,
) -> bool {
    let output = Command::new("bash")
        .env("EVENT", event)
        .env("CLASSIFY", classify)
        .env("CHANGE", change)
        .env("TESTS", tests)
        .env("STATIC", checks)
        .args(["--noprofile", "--norc", "-e", "-c", script])
        .output()
        .expect("bash runs");
    let log = String::from_utf8_lossy(&output.stdout).into_owned();
    let case = format!(
        "event {event}, classification {classify}, change {change:?}, tests {tests}, static {checks}"
    );
    let phases = tests == "success" && checks == "success";
    let skipped = event == "pull_request"
        && classify == "success"
        && change == "planning-only"
        && tests == "skipped"
        && checks == "skipped";
    assert_eq!(output.status.success(), phases || skipped, "{case}:\n{log}");
    let says_skipped = log.contains("planning-only pull request: no build and no test ran");
    assert_eq!(says_skipped, skipped, "{case}:\n{log}");
    output.status.success()
}

/// `macos.yml` is not a required context. It skips its one job by the same classification, from
/// the same workflow and so the same list, and holds no path filter of its own.
#[test]
fn macos_yml_skips_its_job_by_the_same_classification() {
    let macos = repository_file(".github/workflows/macos.yml");
    assert!(macos.contains("\njobs:\n  classify:\n"));
    assert_eq!(
        job(".github/workflows/macos.yml", "classify"),
        job(".github/workflows/gate.yml", "classify")
    );
    let host = job(".github/workflows/macos.yml", "host");
    assert!(host.starts_with(&format!("{NEEDS}{SKIP}")));
    assert_eq!(host.matches("\n    if:").count(), 1, "host has a second if");
    assert!(!macos.contains("paths:") && !macos.contains("paths-ignore:"));
    assert_eq!(macos.matches("needs.classify.outputs.change").count(), 1);
}

/// One list: it names trees, never a pattern for every tree; a workflow that skips for
/// `planning-only` calls the one classifier and reads no changed path itself; and `AGENTS.md`
/// names the list and the rule that `main` always runs the full gate.
#[test]
fn the_list_is_one_file_and_agents_md_states_the_rule() {
    let list = repository_file(LIST);
    let patterns: Vec<&str> = list
        .lines()
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .collect();
    assert!(patterns.contains(&"include .engineering/*"));
    for line in &patterns {
        let (directive, pattern) = line.split_once(' ').expect("directive and pattern");
        assert!(["include", "exclude"].contains(&directive), "{line}");
        assert!(
            pattern
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"._/*-".contains(&byte)),
            "{line}"
        );
        assert!(
            !pattern.starts_with('*'),
            "{line} lists paths in every tree"
        );
    }
    let workflows = repository_root().join(".github/workflows");
    for entry in fs::read_dir(&workflows).expect("workflows") {
        let path = entry.expect("workflow entry").path();
        let name = path
            .file_name()
            .expect("name")
            .to_string_lossy()
            .into_owned();
        let text = fs::read_to_string(&path).expect("workflow text");
        if name != "classify-change.yml" && text.contains("planning-only") {
            assert!(text.contains(CALL), "{name} does not call the classifier");
            assert!(
                !text.contains("git diff") && !text.contains("merge-base"),
                "{name} reads changed paths itself"
            );
        }
    }
    let agents = repository_file("AGENTS.md");
    for statement in [
        "`.github/planning-only-paths`",
        "`.github/workflows/classify-change.yml`",
        "A push to `main` and a `workflow_dispatch` always run the full gate",
    ] {
        assert!(agents.contains(statement), "AGENTS.md lacks {statement:?}");
    }
}

fn rust_sources(directory: &Path, found: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(directory).expect("source directory") {
        let path = entry.expect("source entry").path();
        if path.is_dir() {
            if path.file_name().is_some_and(|name| name != "target") {
                rust_sources(&path, found);
            }
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            found.push(path);
        }
    }
}

fn normalized(path: &Path) -> PathBuf {
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                result.pop();
            }
            Component::CurDir => {}
            other => result.push(other),
        }
    }
    result
}

/// Every Markdown path a line of Rust source spells outside a comment, as written.
fn markdown_names(source: &str) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for line in source.lines() {
        if line.trim_start().starts_with("//") {
            continue;
        }
        for (end, _) in line.match_indices(".md") {
            let after = line[end + 3..].chars().next();
            if after.is_some_and(|next| next.is_ascii_alphanumeric() || next == '_') {
                continue;
            }
            let start = line[..end]
                .rfind(|character: char| {
                    !(character.is_ascii_alphanumeric() || "._/-".contains(character))
                })
                .map_or(0, |index| {
                    index + line[index..].chars().next().map_or(1, char::len_utf8)
                });
            if start < end {
                names.insert(line[start..end + 3].to_owned());
            }
        }
    }
    names
}

/// A test, a build or an `include_str!` that names a Markdown file can fail when that file
/// changes, so such a file is code: a pull request that changes it runs the gate. The names are
/// read from the Rust sources of `crates/` and `xtask/`, resolved against the repository, the
/// source file's directory and its package, and every one that is a file of this repository is
/// changed by one throwaway pull request. A path built at run time is not found this way.
///
/// [`PROSE_CHECK_ONLY`] is the one allowance: a listed file that only a prose check reads.
#[test]
fn a_markdown_file_that_rust_source_names_is_never_planning_only() {
    let root = repository_root();
    let mut sources = Vec::new();
    rust_sources(&root.join("crates"), &mut sources);
    rust_sources(&root.join("xtask"), &mut sources);
    assert!(sources.len() > 50, "found {} Rust sources", sources.len());
    let own = root.join(file!());
    assert!(own.is_file(), "this test does not find its own source");
    let mut named = BTreeSet::new();
    for source in sources.iter().filter(|source| **source != own) {
        let directory = source.parent().expect("a source file has a directory");
        let package = directory
            .ancestors()
            .find(|ancestor| ancestor.join("Cargo.toml").is_file())
            .expect("a source file has a package");
        let text = fs::read_to_string(source).expect("Rust source");
        for name in markdown_names(&text) {
            for base in [root.as_path(), directory, package] {
                let candidate = normalized(&base.join(&name));
                if let Ok(relative) = candidate.strip_prefix(&root)
                    && candidate.is_file()
                {
                    named.insert(relative.to_string_lossy().into_owned());
                }
            }
        }
    }
    for expected in [
        "README.md",
        "AGENTS.md",
        "CHANGELOG.md",
        "crates/b10x-substrate-sdk/README.md",
    ] {
        assert!(
            named.contains(expected),
            "the scan no longer finds {expected}: {named:?}"
        );
    }
    for index in PROSE_CHECK_ONLY {
        assert!(
            named.remove(index),
            "Rust source no longer names {index}; drop its allowance"
        );
    }
    let checkout = Checkout::new();
    let answer = checkout.answer_for(|pr| {
        for path in &named {
            pr.write(path, "changed by the pull request\n");
        }
    });
    let listed: Vec<&String> = named
        .iter()
        .filter(|path| answer.class_of(path) != "code")
        .collect();
    assert!(
        listed.is_empty(),
        "Rust source names these Markdown files and {LIST} still lists them; exclude each: {listed:#?}"
    );
    answer.expect("code", "every Markdown file Rust source names");
}

/// The configuration the code-scanning analysis reads (`AGENTS.md` § *The gate*).
const SCANNER_CONFIG: &str = ".github/codeql/codeql-config.yml";
/// The whole file: the analysis skips two patterns of test code and changes nothing else.
const SCANNER_CONFIG_TEXT: &str = "paths-ignore:\n  - \"crates/*/tests/**\"\n  - \"**/tests.rs\"\n";
/// The top-level directories that hold Rust source.
const RUST_ROOTS: [&str; 5] = ["crates", "spec", "test-support", "vendor", "xtask"];

/// Which of the two patterns of [`SCANNER_CONFIG_TEXT`] skips `path`, a repository-relative path.
fn scanner_skips(path: &str) -> Option<&'static str> {
    let components: Vec<&str> = path.split('/').collect();
    if components.len() > 3 && components[0] == "crates" && components[2] == "tests" {
        Some("crates/*/tests/**")
    } else if components.last() == Some(&"tests.rs") {
        Some("**/tests.rs")
    } else {
        None
    }
}

/// Whether `parent` declares a module `tests`, and every such declaration is compiled for tests
/// only: one of the attribute lines directly above it is `#[cfg(test)]`.
fn declares_tests_for_tests_only(parent: &str) -> bool {
    let lines: Vec<&str> = parent.lines().map(str::trim).collect();
    let mut declared = false;
    for (index, line) in lines.iter().enumerate() {
        if *line != "mod tests;" && !line.ends_with(" mod tests;") {
            continue;
        }
        declared = true;
        let gated = lines[..index]
            .iter()
            .rev()
            .take_while(|above| above.starts_with("#["))
            .any(|above| *above == "#[cfg(test)]");
        if !gated {
            return false;
        }
    }
    declared
}

/// Where `line` of shipped source or of a manifest takes code from a `tests/` directory: a
/// `#[path]` attribute or an `include!`. `include_str!` and `include_bytes!` take data.
fn compiles_from_a_tests_directory(line: &str) -> bool {
    let line = line.trim_start();
    !line.starts_with("//")
        && line.contains("tests/")
        && (line.contains("#[path") || line.contains("include!("))
}

/// The scanner's configuration is the two patterns and nothing else: no query is removed, no
/// threat model is changed and no third path is skipped without this test changing with it.
#[test]
fn the_scanner_configuration_skips_two_test_path_patterns_and_nothing_else() {
    assert_eq!(repository_file(SCANNER_CONFIG), SCANNER_CONFIG_TEXT);
    assert_eq!(
        scanner_skips("crates/substrate-daemon/tests/pipe_session.rs"),
        Some("crates/*/tests/**")
    );
    assert_eq!(
        scanner_skips("crates/substrate-daemon/tests/processes/events.rs"),
        Some("crates/*/tests/**")
    );
    assert_eq!(
        scanner_skips("crates/substrate-store/src/backend/tests.rs"),
        Some("**/tests.rs")
    );
    for scanned in [
        "crates/substrate-daemon/src/app.rs",
        "crates/substrate-daemon/src/app/tests/exec_lists.rs",
        "crates/substrate-kubernetes/src/namespace/probe_tests.rs",
        "crates/tests/lib.rs",
        "xtask/tests/release_workflow.rs",
        "xtask/src/main.rs",
    ] {
        assert_eq!(scanner_skips(scanned), None, "{scanned} is scanned");
    }
}

/// Both sides of the rule the next case applies to every skipped `tests.rs`.
#[test]
fn a_tests_module_counts_as_test_code_only_under_cfg_test() {
    for gated in [
        "#[cfg(test)]\nmod tests;\n",
        "pub fn shipped() {}\n\n#[cfg(target_os = \"linux\")]\n#[cfg(test)]\nmod tests;\n",
        "#[cfg(test)]\n#[allow(clippy::unwrap_used)]\n    mod tests;\n",
    ] {
        assert!(declares_tests_for_tests_only(gated), "{gated:?}");
    }
    for shipped in [
        "mod tests;\n",
        "pub mod tests;\n",
        "#[cfg(test)]\nmod other;\nmod tests;\n",
        "#[cfg(test)]\n\nmod tests;\n",
        "#[cfg(feature = \"tests\")]\npub(crate) mod tests;\n",
        "#[cfg(test)]\nmod tests;\n#[cfg(unix)]\nmod tests;\n",
        "mod tests {}\n",
        "",
    ] {
        assert!(!declares_tests_for_tests_only(shipped), "{shipped:?}");
    }
    assert!(compiles_from_a_tests_directory(
        "#[path = \"../tests/support.rs\"]"
    ));
    assert!(compiles_from_a_tests_directory(
        "    include!(\"../tests/generated.rs\");"
    ));
    for data in [
        "    const GOLDEN: &str = include_str!(\"../tests/golden/exec-pod.json\");",
        "// #[path = \"../tests/support.rs\"]",
        "#[path = \"support.rs\"]",
    ] {
        assert!(!compiles_from_a_tests_directory(data), "{data:?}");
    }
}

/// No path the scanner skips is compiled into a shipped binary: a skipped `tests.rs` is a module
/// its parent declares under `#[cfg(test)]`, no manifest builds a library, a binary or an example
/// from a `tests/` directory, and no scanned source takes code from one.
#[test]
fn no_path_the_scanner_skips_is_compiled_into_a_shipped_binary() {
    let root = repository_root();
    let mut sources = Vec::new();
    for directory in RUST_ROOTS {
        rust_sources(&root.join(directory), &mut sources);
    }
    let mut skipped = [0_usize; 2];
    let mut wrong = Vec::new();
    for source in &sources {
        let relative = source
            .strip_prefix(&root)
            .expect("a source of this repository")
            .to_str()
            .expect("a UTF-8 path")
            .replace('\\', "/");
        match scanner_skips(&relative) {
            Some("crates/*/tests/**") => skipped[0] += 1,
            Some(_) => {
                skipped[1] += 1;
                let directory = source.parent().expect("a source has a directory");
                let mut parents = vec![directory.with_extension("rs"), directory.join("mod.rs")];
                if directory.file_name().is_some_and(|name| name == "src") {
                    parents = vec![directory.join("lib.rs"), directory.join("main.rs")];
                }
                let gated = parents
                    .iter()
                    .filter_map(|parent| fs::read_to_string(parent).ok())
                    .any(|parent| declares_tests_for_tests_only(&parent));
                if !gated {
                    wrong.push(format!(
                        "{relative}: no parent module declares it under #[cfg(test)]"
                    ));
                }
            }
            None => {
                let text = fs::read_to_string(source).expect("a readable source");
                for (number, line) in text.lines().enumerate() {
                    if relative.starts_with("crates/") && compiles_from_a_tests_directory(line) {
                        wrong.push(format!(
                            "{relative}:{}: takes code from a tests directory",
                            number + 1
                        ));
                    }
                }
            }
        }
    }
    for entry in fs::read_dir(root.join("crates")).expect("the crates directory") {
        let manifest = entry.expect("a crate entry").path().join("Cargo.toml");
        let Ok(text) = fs::read_to_string(&manifest) else {
            continue;
        };
        let mut table = String::new();
        for line in text.lines().map(str::trim) {
            if line.starts_with('[') {
                table = line.to_owned();
            } else if line.starts_with("path") && line.contains("\"tests/") && table != "[[test]]" {
                wrong.push(format!(
                    "{}: {table} builds from a tests directory: {line}",
                    manifest.display()
                ));
            }
        }
    }
    assert!(
        skipped[0] > 0 && skipped[1] > 0,
        "the walk found no skipped source for one of the two patterns: {skipped:?}"
    );
    assert!(
        wrong.is_empty(),
        "{SCANNER_CONFIG} skips these paths, and each is compiled into shipped code or reaches \
         it; change the code or the pattern:\n{}",
        wrong.join("\n")
    );
}
