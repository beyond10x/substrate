//! Offline assertions for `scripts/bot-gh.sh`, `scripts/as-bot.sh` and `scripts/bot-token.sh`.
//!
//! Both `bot-gh.sh` and `as-bot.sh` mint a b10x-bot token with `scripts/bot-token.sh` and then
//! run a program with it. The rule held here: where the mint fails, or prints no token, the
//! script ends non-zero and the program is not run. `gh` without a `GH_TOKEN` acts under the
//! login of the caller's keyring, and `git` without the token's credential helper under the
//! caller's own.
//!
//! Each case copies the script under test into a throwaway directory beside a stand-in
//! `bot-token.sh`, and puts stand-ins for `gh`, `git`, `curl` and `jq` first on `PATH`. Each
//! stand-in appends one line to a file, so a case reads which programs ran. Nothing here
//! reaches the network, reads a key or builds anything.
//!
//! `bot-token.sh` itself checks its two credential files before it reads either, and starts no
//! `cargo`. Its cases copy it into a throwaway directory with throwaway credential files of
//! arbitrary bytes, and run it with a `PATH` of two directories only: stand-ins for `jq`,
//! `openssl`, `curl` and `cargo`, and links to the system tools the script uses. No `cargo`
//! but the stand-in is on that `PATH`. `B10X_BOT_CONFIG`, `B10X_BOT_KEY`, `HOME` and
//! `XDG_CONFIG_HOME` all name the throwaway directory.

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

const UNAVAILABLE: &str = "b10x-bot credential input is unavailable";
const NOT_AN_OWNED_REGULAR_FILE: &str =
    "b10x-bot credential input must be a current-user-owned regular file";
const WRITABLE_BY_OTHERS: &str = "b10x-bot credential input must be not group/world-writable";
const NOT_OWNER_ONLY: &str = "b10x-bot credential input must be owner-only";
const NOT_READABLE: &str = "b10x-bot credential input is not readable";

/// The installation the stand-in `jq` reads out of the stand-in forge's list.
const INSTALLATION: &str = "2";

/// The system tools `bot-token.sh` and the stand-ins run, linked into a directory of their own
/// so that the case's `PATH` holds no `cargo` but the stand-in.
const TOOLS: [&str; 7] = ["cat", "date", "dirname", "find", "head", "id", "tr"];

/// The bytes of the throwaway credential files: no configuration a forge knows and no key.
const CONFIG_BYTES: &str = "stand-in configuration bytes\n";
const KEY_BYTES: &str = "stand-in key bytes\n";

fn system_program(name: &str) -> PathBuf {
    ["/usr/bin", "/bin"]
        .iter()
        .map(|directory| Path::new(directory).join(name))
        .find(|path| path.is_file())
        .unwrap_or_else(|| panic!("this host has no {name} in /usr/bin or /bin"))
}

fn effective_uid() -> u32 {
    let output = Command::new(system_program("id"))
        .arg("-u")
        .output()
        .expect("running id -u");
    String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .expect("id -u prints a number")
}

fn set_mode(path: &Path, mode: u32) {
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
        .unwrap_or_else(|error| panic!("mode of {}: {error}", path.display()));
}

/// A throwaway tree for one run of `bot-token.sh`: the script, a `credentials` directory the
/// two files are named in, a `home`, the stand-ins in `bin` and the system tools in `tools`.
struct TokenRun {
    tree: tempfile::TempDir,
}

impl TokenRun {
    /// A tree whose configuration is mode `0644` and whose key is mode `0600`.
    fn new() -> Self {
        let mint = Self {
            tree: tempfile::tempdir().expect("a temporary directory"),
        };
        for directory in ["scripts", "credentials", "home", "bin", "tools"] {
            fs::create_dir_all(mint.tree.path().join(directory))
                .unwrap_or_else(|error| panic!("the {directory} directory: {error}"));
        }
        let source = repository_root().join("scripts").join("bot-token.sh");
        let text = fs::read_to_string(&source)
            .unwrap_or_else(|error| panic!("the repository has no readable bot-token.sh: {error}"));
        executable(
            &mint.tree.path().join("scripts").join("bot-token.sh"),
            &text,
        );

        for tool in TOOLS {
            std::os::unix::fs::symlink(system_program(tool), mint.tools().join(tool))
                .unwrap_or_else(|error| panic!("linking {tool}: {error}"));
        }

        let calls = mint.calls_file();
        let record = format!(
            "printf '%s %s\\n' \"${{0##*/}}\" \"$*\" >> '{}'",
            calls.display()
        );
        executable(
            &mint.bin().join("cargo"),
            &format!("#!/bin/sh\n{record}\nexit 0\n"),
        );
        executable(
            &mint.bin().join("jq"),
            &format!(
                "#!/bin/sh\n{record}\ncase \"$*\" in\n*.app_id*) printf '1\\n' ;;\n*.token*) cat > /dev/null; printf '%s\\n' '{TOKEN}' ;;\n*) cat > /dev/null; printf '%s\\n' '{INSTALLATION}' ;;\nesac\n"
            ),
        );
        executable(
            &mint.bin().join("openssl"),
            &format!("#!/bin/sh\n{record}\ncat > /dev/null\nprintf 'c3RhbmQtaW4'\n"),
        );
        executable(
            &mint.bin().join("curl"),
            &format!("#!/bin/sh\n{record}\nprintf '{{}}\\n'\n"),
        );

        fs::write(mint.config(), CONFIG_BYTES).expect("the throwaway configuration");
        set_mode(&mint.config(), 0o644);
        fs::write(mint.key(), KEY_BYTES).expect("the throwaway key");
        set_mode(&mint.key(), 0o600);
        mint
    }

    fn bin(&self) -> PathBuf {
        self.tree.path().join("bin")
    }

    fn tools(&self) -> PathBuf {
        self.tree.path().join("tools")
    }

    fn calls_file(&self) -> PathBuf {
        self.tree.path().join("calls")
    }

    fn config(&self) -> PathBuf {
        self.tree.path().join("credentials").join("b10x-bot.json")
    }

    fn key(&self) -> PathBuf {
        self.tree
            .path()
            .join("credentials")
            .join("b10x-bot.private-key.pem")
    }

    /// Runs the script with `bin` and `tools` as its whole `PATH`.
    fn run(&self) -> Run {
        let home = self.tree.path().join("home");
        let output = Command::new(system_program("bash"))
            .arg(self.tree.path().join("scripts").join("bot-token.sh"))
            .current_dir(self.tree.path())
            .env_clear()
            .env(
                "PATH",
                format!("{}:{}", self.bin().display(), self.tools().display()),
            )
            .env("HOME", &home)
            .env("XDG_CONFIG_HOME", home.join(".config"))
            .env("B10X_BOT_CONFIG", self.config())
            .env("B10X_BOT_KEY", self.key())
            .output()
            .expect("running bot-token.sh");
        let calls = fs::read_to_string(self.calls_file())
            .map(|text| text.lines().map(str::to_owned).collect())
            .unwrap_or_default();
        Run { output, calls }
    }

    /// Asserts that the script ends non-zero, prints exactly `refusals` on standard error and
    /// nothing on standard output, and starts no program: no `jq`, `openssl`, `curl` or `cargo`.
    fn assert_refused(&self, refusals: &[&str]) {
        let run = self.run();
        assert!(
            !run.output.status.success(),
            "bot-token.sh ends non-zero for a refused credential file; stderr: {}",
            stderr(&run)
        );
        assert_eq!(
            stderr(&run),
            format!("{}\n", refusals.join("\n")),
            "bot-token.sh prints the refusals and nothing else on standard error"
        );
        assert!(
            run.output.stdout.is_empty(),
            "bot-token.sh prints nothing on standard output for a refused credential file"
        );
        assert_eq!(
            run.calls,
            Vec::<String>::new(),
            "bot-token.sh starts no program for a refused credential file"
        );
    }

    /// Asserts that the script prints the stand-in forge's token, asked the forge for the
    /// installation's token, and started no `cargo`.
    fn assert_minted(&self) {
        let run = self.run();
        assert!(
            run.output.status.success(),
            "bot-token.sh mints with admitted credential files; stderr: {}",
            stderr(&run)
        );
        assert_eq!(
            String::from_utf8_lossy(&run.output.stdout),
            format!("{TOKEN}\n"),
            "bot-token.sh prints the token the forge returned"
        );
        assert_eq!(
            stderr(&run),
            "",
            "bot-token.sh prints nothing on standard error"
        );
        assert!(
            !run.calls.iter().any(|line| line.starts_with("cargo ")),
            "bot-token.sh starts no cargo: {:?}",
            run.calls
        );
        let token_request =
            format!("https://api.github.com/app/installations/{INSTALLATION}/access_tokens");
        assert!(
            run.calls
                .iter()
                .any(|line| line.starts_with("curl ") && line.ends_with(&token_request)),
            "bot-token.sh asks the forge for the installation's token: {:?}",
            run.calls
        );
    }
}

#[test]
fn bot_token_mints_with_a_configuration_of_mode_0644_and_a_key_of_mode_0600() {
    TokenRun::new().assert_minted();
}

#[test]
fn bot_token_mints_with_a_configuration_of_mode_0600_and_a_key_of_mode_0400() {
    let mint = TokenRun::new();
    set_mode(&mint.config(), 0o600);
    set_mode(&mint.key(), 0o400);
    mint.assert_minted();
}

#[test]
fn bot_token_mints_with_no_cargo_on_path() {
    let mint = TokenRun::new();
    fs::remove_file(mint.bin().join("cargo")).expect("removing the stand-in cargo");
    mint.assert_minted();
}

#[test]
fn bot_token_refuses_a_missing_configuration() {
    let mint = TokenRun::new();
    fs::remove_file(mint.config()).expect("removing the configuration");
    mint.assert_refused(&[UNAVAILABLE]);
}

#[test]
fn bot_token_refuses_a_missing_key() {
    let mint = TokenRun::new();
    fs::remove_file(mint.key()).expect("removing the key");
    mint.assert_refused(&[UNAVAILABLE]);
}

#[test]
fn bot_token_refuses_a_configuration_that_is_a_directory() {
    let mint = TokenRun::new();
    fs::remove_file(mint.config()).expect("removing the configuration");
    fs::create_dir(mint.config()).expect("a directory in its place");
    mint.assert_refused(&[NOT_AN_OWNED_REGULAR_FILE]);
}

#[test]
fn bot_token_refuses_a_key_that_is_a_fifo() {
    let mint = TokenRun::new();
    fs::remove_file(mint.key()).expect("removing the key");
    let made = Command::new(system_program("mkfifo"))
        .arg(mint.key())
        .status()
        .expect("running mkfifo");
    assert!(made.success(), "mkfifo makes a FIFO in the throwaway tree");
    mint.assert_refused(&[NOT_AN_OWNED_REGULAR_FILE]);
}

#[test]
fn bot_token_refuses_a_configuration_that_is_a_symbolic_link_to_an_admitted_file() {
    let mint = TokenRun::new();
    let target = mint.tree.path().join("credentials").join("linked.json");
    fs::rename(mint.config(), &target).expect("moving the configuration");
    std::os::unix::fs::symlink(&target, mint.config()).expect("a link in its place");
    mint.assert_refused(&[NOT_AN_OWNED_REGULAR_FILE]);
}

#[test]
fn bot_token_refuses_a_key_that_is_a_symbolic_link_to_an_admitted_file() {
    let mint = TokenRun::new();
    let target = mint.tree.path().join("credentials").join("linked.pem");
    fs::rename(mint.key(), &target).expect("moving the key");
    std::os::unix::fs::symlink(&target, mint.key()).expect("a link in its place");
    mint.assert_refused(&[NOT_AN_OWNED_REGULAR_FILE]);
}

/// A file of another owner needs privilege to make. The script reads the effective user from
/// `id -u`; a stand-in `id` names another uid, so both throwaway files belong to another user
/// as far as the script can tell.
#[test]
fn bot_token_refuses_files_another_user_owns() {
    let mint = TokenRun::new();
    let other = effective_uid() + 1;
    executable(
        &mint.bin().join("id"),
        &format!("#!/bin/sh\n[ \"$1\" = -u ] || exit 1\nprintf '%s\\n' {other}\n"),
    );
    mint.assert_refused(&[NOT_AN_OWNED_REGULAR_FILE, NOT_AN_OWNED_REGULAR_FILE]);
}

#[test]
fn bot_token_refuses_a_configuration_writable_by_its_group() {
    let mint = TokenRun::new();
    set_mode(&mint.config(), 0o664);
    mint.assert_refused(&[WRITABLE_BY_OTHERS]);
}

#[test]
fn bot_token_refuses_a_configuration_writable_by_others() {
    let mint = TokenRun::new();
    set_mode(&mint.config(), 0o646);
    mint.assert_refused(&[WRITABLE_BY_OTHERS]);
}

#[test]
fn bot_token_refuses_a_key_its_group_may_read() {
    let mint = TokenRun::new();
    set_mode(&mint.key(), 0o640);
    mint.assert_refused(&[NOT_OWNER_ONLY]);
}

#[test]
fn bot_token_refuses_a_key_others_may_execute() {
    let mint = TokenRun::new();
    set_mode(&mint.key(), 0o601);
    mint.assert_refused(&[NOT_OWNER_ONLY]);
}

#[test]
fn bot_token_refuses_unreadable_files() {
    if effective_uid() == 0 {
        eprintln!("not run: the effective user is root, which reads a file of mode 000");
        return;
    }
    let mint = TokenRun::new();
    set_mode(&mint.config(), 0o000);
    set_mode(&mint.key(), 0o000);
    mint.assert_refused(&[NOT_READABLE, NOT_READABLE]);
}

#[test]
fn bot_token_prints_a_refusal_for_each_file() {
    let mint = TokenRun::new();
    fs::remove_file(mint.config()).expect("removing the configuration");
    set_mode(&mint.key(), 0o644);
    mint.assert_refused(&[UNAVAILABLE, NOT_OWNER_ONLY]);
}
