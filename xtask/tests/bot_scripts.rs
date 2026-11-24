//! Offline assertions for `scripts/bot-gh.sh` and `scripts/as-bot.sh`.
//!
//! Both scripts mint a b10x-bot token with `scripts/bot-token.sh` and then run a program with
//! it. The rule held here: where the mint fails, or prints no token, the script ends non-zero
//! and the program is not run. `gh` without a `GH_TOKEN` acts under the login of the caller's
//! keyring, and `git` without the token's credential helper under the caller's own.
//!
//! Each case copies the script under test into a throwaway directory beside a stand-in
//! `bot-token.sh`, and puts stand-ins for `gh`, `git`, `curl` and `jq` first on `PATH`. Each
//! stand-in appends one line to a file, so a case reads which programs ran. Nothing here
//! reaches the network, reads a key or builds anything.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// What the stand-in `bot-token.sh` does.
#[derive(Clone, Copy)]
enum Mint {
    /// Prints nothing and exits 1.
    Fails,
    /// Prints the beginning of a token and exits 1.
    FailsAfterOutput,
    /// Prints nothing and exits 0.
    PrintsNothing,
    /// Prints `TOKEN` and exits 0.
    Succeeds,
}

const TOKEN: &str = "stand-in-token";

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask has a repository parent")
        .to_path_buf()
}

fn executable(path: &Path, text: &str) {
    fs::write(path, text).unwrap_or_else(|error| panic!("writing {}: {error}", path.display()));
    fs::set_permissions(path, fs::Permissions::from_mode(0o755))
        .unwrap_or_else(|error| panic!("mode of {}: {error}", path.display()));
}

/// A throwaway tree with the repository's `script` and a stand-in mint, and the result of
/// running the script there with `arguments`.
struct Run {
    output: Output,
    /// One line for each stand-in program that ran: its name, the token it saw, its arguments.
    calls: Vec<String>,
}

fn run(script: &str, mint: Mint, arguments: &[&str]) -> Run {
    let tree = tempfile::tempdir().expect("a temporary directory");
    let scripts = tree.path().join("scripts");
    let bin = tree.path().join("bin");
    fs::create_dir_all(&scripts).expect("the scripts directory");
    fs::create_dir_all(&bin).expect("the stand-in directory");

    let source = repository_root().join("scripts").join(script);
    let text = fs::read_to_string(&source)
        .unwrap_or_else(|error| panic!("the repository has no readable {script}: {error}"));
    executable(&scripts.join(script), &text);

    let mint_text = match mint {
        Mint::Fails => "#!/bin/sh\nexit 1\n".to_owned(),
        Mint::FailsAfterOutput => "#!/bin/sh\nprintf 'stand-in'\nexit 1\n".to_owned(),
        Mint::PrintsNothing => "#!/bin/sh\nexit 0\n".to_owned(),
        Mint::Succeeds => format!("#!/bin/sh\nprintf '%s\\n' '{TOKEN}'\n"),
    };
    executable(&scripts.join("bot-token.sh"), &mint_text);

    let calls = tree.path().join("calls");
    for program in ["gh", "git", "curl"] {
        executable(
            &bin.join(program),
            &format!(
                "#!/bin/sh\nprintf '%s token=%s%s args=%s\\n' {program} \"${{GH_TOKEN:-}}\" \"${{B10X_BOT_TOKEN:-}}\" \"$*\" >> '{}'\nprintf '{{\"id\": 1}}\\n'\n",
                calls.display()
            ),
        );
    }
    executable(
        &bin.join("jq"),
        &format!(
            "#!/bin/sh\nprintf 'jq\\n' >> '{}'\ncat > /dev/null\nprintf '1\\n'\n",
            calls.display()
        ),
    );

    let path = format!("{}:/usr/bin:/bin", bin.display());
    let output = Command::new("bash")
        .arg(scripts.join(script))
        .args(arguments)
        .env_clear()
        .env("PATH", path)
        .env("HOME", tree.path())
        .output()
        .unwrap_or_else(|error| panic!("running {script}: {error}"));
    let calls = fs::read_to_string(&calls)
        .map(|text| text.lines().map(str::to_owned).collect())
        .unwrap_or_default();
    Run { output, calls }
}

fn stderr(run: &Run) -> String {
    String::from_utf8_lossy(&run.output.stderr).into_owned()
}

fn assert_stopped(script: &str, mint: Mint, why: &str) {
    let run = run(script, mint, &["pr", "list"]);
    assert!(
        !run.output.status.success(),
        "{script} exits non-zero where the mint {why}; stderr: {}",
        stderr(&run)
    );
    assert_eq!(
        run.calls,
        Vec::<String>::new(),
        "{script} runs no program where the mint {why}"
    );
    assert!(
        stderr(&run).contains("is not run"),
        "{script} says that its program is not run where the mint {why}; stderr: {}",
        stderr(&run)
    );
}

#[test]
fn bot_gh_runs_no_gh_where_the_mint_fails() {
    assert_stopped("bot-gh.sh", Mint::Fails, "fails");
}

#[test]
fn bot_gh_runs_no_gh_where_the_mint_fails_after_it_printed_something() {
    assert_stopped("bot-gh.sh", Mint::FailsAfterOutput, "fails after output");
}

#[test]
fn bot_gh_runs_no_gh_where_the_mint_prints_no_token() {
    assert_stopped("bot-gh.sh", Mint::PrintsNothing, "prints no token");
}

#[test]
fn bot_gh_runs_gh_with_the_minted_token_and_the_callers_arguments() {
    let run = run("bot-gh.sh", Mint::Succeeds, &["pr", "list"]);
    assert!(
        run.output.status.success(),
        "bot-gh.sh succeeds with a minted token; stderr: {}",
        stderr(&run)
    );
    assert_eq!(
        run.calls,
        vec![format!("gh token={TOKEN} args=pr list")],
        "bot-gh.sh runs gh once, with the token and the arguments"
    );
}

#[test]
fn as_bot_runs_no_curl_and_no_git_where_the_mint_fails() {
    assert_stopped("as-bot.sh", Mint::Fails, "fails");
}

#[test]
fn as_bot_runs_no_curl_and_no_git_where_the_mint_prints_no_token() {
    assert_stopped("as-bot.sh", Mint::PrintsNothing, "prints no token");
}

#[test]
fn as_bot_runs_git_with_the_minted_token_exported() {
    let run = run("as-bot.sh", Mint::Succeeds, &["status"]);
    assert!(
        run.output.status.success(),
        "as-bot.sh succeeds with a minted token; stderr: {}",
        stderr(&run)
    );
    let git: Vec<&String> = run
        .calls
        .iter()
        .filter(|line| line.starts_with("git "))
        .collect();
    assert_eq!(git.len(), 1, "as-bot.sh runs git once: {:?}", run.calls);
    assert!(
        git[0].starts_with(&format!("git token={TOKEN} args=")) && git[0].ends_with(" status"),
        "git sees the exported token and the caller's arguments last: {}",
        git[0]
    );
}
