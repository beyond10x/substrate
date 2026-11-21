//! Adversary pass 1 on `story:linux-executable-allow-list` (design 35), commit `4b5208922`.
//!
//! Every case starts a run through `ConfinementAdapter` on the real `linux` adapter, opened with
//! the helper the lane built, and reads what the run itself printed.
//!
//! The cases need `SUBSTRATE_VECTORS_CGROUP_ROOT` naming a delegated cgroup v2 subtree this
//! process is inside, bubblewrap, and `SUBSTRATE_EXEC_GATE` naming `substrate-exec-gate`. Without
//! the first each prints `<name>: ABSENT (<reason>)` and is absent, never passed.

#![cfg(target_os = "linux")]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use substrate_host::HostConfig;
use substrate_host::confinement::{
    self, AdapterRequest, Command, ConfinementAdapter, ConfinementError, ConfinementProfile, Fact,
    ReadOnlyFile, ReadOnlyRoot, RunState, WritableHostRoot, linux::LinuxRequest,
};
use substrate_host::seatbelt::PATH_UNREPRESENTABLE;

/// The delegated cgroup v2 subtree this process is inside, or `None` after printing why not.
fn delegated(name: &str) -> Option<PathBuf> {
    let absent = |reason: &str| {
        println!("{name}: ABSENT ({reason})");
        None
    };
    let Some(root) = std::env::var_os("SUBSTRATE_VECTORS_CGROUP_ROOT").map(PathBuf::from) else {
        return absent("SUBSTRATE_VECTORS_CGROUP_ROOT is unset");
    };
    if std::fs::write(root.join("cgroup.subtree_control"), "+cpu +memory +pids").is_err() {
        return absent("the delegated cgroup root does not accept controllers");
    }
    if !Path::new("/usr/bin/bwrap").is_file() {
        return absent("no bubblewrap at /usr/bin/bwrap");
    }
    Some(root)
}

fn tool(name: &str) -> PathBuf {
    ["/usr/bin", "/bin"]
        .iter()
        .map(|directory| Path::new(directory).join(name))
        .find(|path| path.is_file())
        .unwrap_or_else(|| panic!("no {name} in /usr/bin or /bin"))
}

fn text(path: &Path) -> String {
    path.to_str().expect("UTF-8").to_owned()
}

struct Lane {
    host: tempfile::TempDir,
    adapter: Arc<dyn ConfinementAdapter>,
    workspace: PathBuf,
}

/// The adapter opened with the lane's helper, having proved `exec_subpaths`, and a workspace
/// with `bin/tool` and `other/tool`, both copies of the host's `true`.
fn lane(name: &str) -> Option<Lane> {
    lane_with(name, |_, _| {})
}

/// [`lane`], with `configure` given the host configuration and the host root before the open.
fn lane_with(name: &str, configure: impl FnOnce(&mut HostConfig, &Path)) -> Option<Lane> {
    let cgroup_root = delegated(name)?;
    let gate = PathBuf::from(std::env::var_os("SUBSTRATE_EXEC_GATE").expect(
        "SUBSTRATE_EXEC_GATE is unset: `bash scripts/delegated-lane.sh` builds the helper",
    ));
    let host = tempfile::tempdir().expect("host root");
    let mut config = HostConfig::minimum(host.path().join("workspaces"));
    config.exec_gate = Some(gate);
    configure(&mut config, host.path());
    let adapter = confinement::open(AdapterRequest::Linux(Box::new(
        LinuxRequest::new(config).with_cgroup_root(Some(cgroup_root)),
    )))
    .expect("linux opens");
    assert!(
        adapter.facts().proves(&Fact::ExecSubpaths),
        "{name}: linux opened with the helper did not prove exec_subpaths: {:?}",
        adapter.facts()
    );
    let workspace = host.path().join("workspaces").join("ws_adversary");
    for directory in ["bin", "other"] {
        std::fs::create_dir_all(workspace.join(directory)).expect(directory);
        std::fs::copy(tool("true"), workspace.join(directory).join("tool")).expect("copy true");
    }
    Some(Lane {
        host,
        adapter,
        workspace,
    })
}

/// **D28 covers variables; `/etc/ld.so.preload` is a file, not a variable.** The lane's helper is
/// dynamically linked (`interpreter /lib64/ld-linux-x86-64.so.2`), so its own loader reads
/// `/etc/ld.so.preload` before the helper's `main` and before `landlock_restrict_self`. Design 35
/// § 6 states this gap was not checked: "Whether a declared read-only file may be mounted at that
/// path was not checked" (`docs/design/35-executable-lists.md:1311`). `validate_read_only_files`
/// reserves only `RESERVED_MOUNTS` and the aperture/capsule/scratch mounts, not
/// `/etc/ld.so.preload`, so a caller may declare one there, pointing the loader at an object in the
/// workspace whose constructor runs with no list applied.
///
/// The object, on its first load, executes an unlisted base tool (`echo LEAKED`); a guard file
/// lets later loads of it (by that `echo`) proceed. If the list held, the run's own script would
/// print `0\n126\n126\n` (the base tool refused, 126). If the object ran before the ruleset,
/// `LEAKED` reaches stdout and the run never reaches its own script.
///
/// **Two assertions were tightened by the implementor**, on the coordinator's word and by the
/// design's seventh revision (D29, rule 2: "No list of loader files is kept. The driver does not
/// refuse a declared mount at `/etc/ld.so.preload`"). The review took a start the adapter refuses
/// as a pass, naming the refusal. The defence is a helper with no loader, so the profile must be
/// admitted, the start must be dispatched, and the list must hold: a refusal for the path is now
/// a failure.
#[tokio::test(flavor = "multi_thread")]
async fn adversary2_a_read_only_ld_so_preload_runs_before_the_ruleset() {
    const NAME: &str = "adversary2 exec list ld.so.preload";
    let Some(lane) = lane(NAME) else {
        return;
    };
    let echo = tool("echo");
    let source = lane.workspace.join("evil.c");
    std::fs::write(
        &source,
        format!(
            "#include <unistd.h>\n\
             #include <fcntl.h>\n\
             __attribute__((constructor)) static void before_the_ruleset(void) {{\n\
             \x20   int fd = open(\"/workspace/.preload-ran\", O_WRONLY | O_CREAT | O_EXCL, 0644);\n\
             \x20   if (fd < 0) return;\n\
             \x20   close(fd);\n\
             \x20   execl(\"{}\", \"echo\", \"LEAKED\", (char *)0);\n\
             }}\n",
            echo.display()
        ),
    )
    .expect("the source");
    let built = std::process::Command::new("cc")
        .args(["-shared", "-fPIC", "-o"])
        .arg(lane.workspace.join("evil.so"))
        .arg(&source)
        .output()
        .expect("this case needs a C compiler named cc on the lane's host");
    assert!(
        built.status.success(),
        "cc: {}",
        String::from_utf8_lossy(&built.stderr)
    );
    let conf = std::fs::canonicalize(&lane.workspace)
        .expect("canonical workspace")
        .join("preload.conf");
    std::fs::write(&conf, "/workspace/evil.so\n").expect("the preload file");

    let mut profile = listed(&lane.workspace, &["bin"]);
    profile.read_only_files = vec![ReadOnlyFile {
        host_path: text(&conf),
        mount: "/etc/ld.so.preload".to_owned(),
    }];
    let script = three_executions();

    let admitted = lane.adapter.admit(&profile).unwrap_or_else(|error| {
        panic!("D29 refuses no declared mount for its path, and admission did: {error}")
    });
    let command = Command {
        argv: vec![text(&tool("sh")), "-c".into(), script],
        env: BTreeMap::from([("PATH".to_owned(), "/usr/bin:/bin".to_owned())]),
    };
    let run = lane
        .adapter
        .start(&admitted, &command)
        .await
        .unwrap_or_else(|error| {
            panic!("D29 refuses no declared mount for its path, and the start did: {error}")
        });
    let ended = lane.adapter.wait(&run).await.expect("waited");
    let output = lane.adapter.output(&run).await.expect("output");
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    assert_eq!(
        (
            stdout.as_str(),
            ended.exit.as_ref().and_then(|exit| exit.code)
        ),
        ("0\n126\n126\n", Some(0)),
        "a run whose profile lists only `bin` loaded /workspace/evil.so through a read-only \
         /etc/ld.so.preload and executed an unlisted base tool before the ruleset"
    );
    println!("{NAME}: executed");
}

fn listed(workspace: &Path, entries: &[&str]) -> ConfinementProfile {
    ConfinementProfile {
        workspace: Some(workspace.to_path_buf()),
        exec_subpaths: entries.iter().map(|entry| (*entry).to_owned()).collect(),
        ..ConfinementProfile::default()
    }
}

/// Admits `profile`, runs `sh -c script` with `env` beside `PATH`, and returns the run's exit
/// code and its stdout.
async fn run(
    adapter: &Arc<dyn ConfinementAdapter>,
    profile: &ConfinementProfile,
    script: &str,
    env: &[(&str, &str)],
) -> (Option<i32>, String) {
    let mut environment = BTreeMap::from([("PATH".to_owned(), "/usr/bin:/bin".to_owned())]);
    for (name, value) in env {
        environment.insert((*name).to_owned(), (*value).to_owned());
    }
    let admitted = adapter.admit(profile).expect("admitted");
    let run = adapter
        .start(
            &admitted,
            &Command {
                argv: vec![text(&tool("sh")), "-c".into(), script.to_owned()],
                env: environment,
            },
        )
        .await
        .expect("started");
    let ended = adapter.wait(&run).await.expect("waited");
    assert_eq!(ended.state, RunState::Ended, "{ended:?}");
    let output = adapter.output(&run).await.expect("output");
    (
        ended.exit.as_ref().and_then(|exit| exit.code),
        String::from_utf8_lossy(&output.stdout).into_owned(),
    )
}

/// The acceptance's own script: a listed program, a workspace program outside the list, and a
/// base tool outside the list, each by path.
fn three_executions() -> String {
    format!(
        "bin/tool; echo $?; other/tool 2>/dev/null; echo $?; {} 2>/dev/null; echo $?",
        tool("true").display()
    )
}

/// **The story's claim 1 against the run's own environment.** "A profile with a non-empty
/// `exec_subpaths` … runs only its program, the host loader and what the entries name; `execve`
/// of any other path fails inside the run", and design 35 § 6: "Everything bubblewrap, `setsid`
/// and `env` execute happens before the ruleset exists."
///
/// `Command.env` is the run's environment, and bubblewrap sets it (`--setenv`) for
/// `/usr/bin/env`, which runs before the helper, and for the helper itself. Both are ordinary
/// dynamically linked programs, so the host's loader honours `LD_PRELOAD` in them. An object in
/// the workspace, which an earlier run can leave there, therefore runs its constructor before
/// `landlock_restrict_self`, and whatever it executes is held to no list.
///
/// The control is the same start without the variable. The object here does one thing: in the
/// first process that loads it, it executes `sh -c "$ADVERSARY_SCRIPT"`, the same script the
/// run was started with.
///
/// **The object's source was changed by the implementor**, and what it does was not: it takes
/// `LD_PRELOAD` out of `environ` itself where the review wrote `unsetenv("LD_PRELOAD")`. Since
/// D28 the first process that loads the object is the run's own shell, and bash defines an
/// `unsetenv` of its own that a preloaded object's call binds to and that leaves `environ` as it
/// is before bash's `main`; the object then executed a shell that loaded it again, without
/// end, and the case did not return.
#[tokio::test(flavor = "multi_thread")]
async fn adversary_a_preloaded_object_in_the_runs_environment_executes_outside_the_list() {
    const NAME: &str = "adversary exec list preload";
    let Some(lane) = lane(NAME) else {
        return;
    };
    let source = lane.workspace.join("pre.c");
    std::fs::write(
        &source,
        "#include <stdlib.h>\n\
         #include <string.h>\n\
         #include <unistd.h>\n\
         extern char **environ;\n\
         __attribute__((constructor)) static void before_the_ruleset(void) {\n\
         \x20   const char *script = getenv(\"ADVERSARY_SCRIPT\");\n\
         \x20   char **from = environ, **to = environ;\n\
         \x20   if (!getenv(\"LD_PRELOAD\") || !script) return;\n\
         \x20   for (; *from; from++)\n\
         \x20       if (strncmp(*from, \"LD_PRELOAD=\", 11) != 0) *to++ = *from;\n\
         \x20   *to = 0;\n\
         \x20   execl(\"/bin/sh\", \"sh\", \"-c\", script, (char *)0);\n\
         }\n",
    )
    .expect("the source");
    let built = std::process::Command::new("cc")
        .args(["-shared", "-fPIC", "-o"])
        .arg(lane.workspace.join("pre.so"))
        .arg(&source)
        .output()
        .expect("this case needs a C compiler named cc on the lane's host");
    assert!(
        built.status.success(),
        "cc: {}",
        String::from_utf8_lossy(&built.stderr)
    );

    let profile = listed(&lane.workspace, &["bin"]);
    let script = three_executions();

    let (code, stdout) = run(&lane.adapter, &profile, &script, &[]).await;
    assert_eq!(
        (stdout.as_str(), code),
        ("0\n126\n126\n", Some(0)),
        "the control: the list holds the run without the variable"
    );

    let (code, stdout) = run(
        &lane.adapter,
        &profile,
        &script,
        &[
            ("LD_PRELOAD", "/workspace/pre.so"),
            ("ADVERSARY_SCRIPT", &script),
        ],
    )
    .await;
    assert_eq!(
        (stdout.as_str(), code),
        ("0\n126\n126\n", Some(0)),
        "a run whose profile lists only `bin` executed other/tool and a base tool by path: \
         LD_PRELOAD from Command.env reached a program that runs before the ruleset"
    );
    println!("{NAME}: executed");
}

/// Other names of a program outside the list, tried by path inside a run whose profile lists
/// `bin`: none may execute. The link `bin/alias` is made on the host before the start; the entry
/// `bin` itself has no link in it.
#[tokio::test(flavor = "multi_thread")]
async fn adversary_other_names_of_an_unlisted_program_stay_refused() {
    const NAME: &str = "adversary exec list aliases";
    let Some(lane) = lane(NAME) else {
        return;
    };
    std::os::unix::fs::symlink("../other/tool", lane.workspace.join("bin/alias")).expect("link");
    let base = text(&tool("true"));
    let attempts = [
        format!("/proc/self/root{base}"),
        format!("/proc/1/root{base}"),
        format!("/usr/bin/../..{base}"),
        format!("/{base}"),
        "/proc/self/cwd/other/tool".to_owned(),
        "./other/./tool".to_owned(),
        "bin/../other/tool".to_owned(),
        "bin/alias".to_owned(),
        "/workspace/other/tool".to_owned(),
        "/.substrate-exec-gate".to_owned(),
    ];
    let script = attempts
        .iter()
        .map(|path| format!("{path} --version >/dev/null 2>&1; echo \"$? {path}\""))
        .collect::<Vec<_>>()
        .join("; ");
    for env in [&[][..], &[("PWD", "/workspace/other")][..]] {
        let (code, stdout) = run(
            &lane.adapter,
            &listed(&lane.workspace, &["bin"]),
            &format!("bin/tool || exit 90; {script}"),
            env,
        )
        .await;
        let ran: Vec<&str> = stdout
            .lines()
            .filter(|line| line.starts_with("0 "))
            .collect();
        assert!(
            ran.is_empty() && stdout.lines().count() == attempts.len(),
            "env {env:?}: executed: {ran:?}\n{stdout}"
        );
        assert_eq!(code, Some(0), "env {env:?}: the listed program ran first");
    }
    println!("{NAME}: executed");
}

/// Entries whose names read as options or hold a space: each is one operand of the helper, the
/// listed directory runs its program, and its neighbour does not.
#[tokio::test(flavor = "multi_thread")]
async fn adversary_entries_named_like_options_or_with_a_space_are_held() {
    const NAME: &str = "adversary exec list entry names";
    let Some(lane) = lane(NAME) else {
        return;
    };
    for entry in [
        "--",
        "--version",
        "-dash",
        "with space",
        "--allow",
        "--writable",
    ] {
        let directory = lane.workspace.join(entry);
        std::fs::create_dir_all(&directory).expect(entry);
        std::fs::copy(tool("true"), directory.join("tool")).expect("copy true");
        let (code, stdout) = run(
            &lane.adapter,
            &listed(&lane.workspace, &[entry]),
            &format!(
                "'./{entry}/tool'; echo $?; other/tool 2>/dev/null; echo $?; \
                 bin/tool 2>/dev/null; echo $?"
            ),
            &[],
        )
        .await;
        assert_eq!(
            (stdout.as_str(), code),
            ("0\n126\n126\n", Some(0)),
            "entry {entry:?}"
        );
    }
    println!("{NAME}: executed");
}

/// Starts a shell that would write `/workspace/ran` under `profile`, and returns the refusal the
/// start answered before dispatch. A start that is dispatched, or does not answer, fails.
async fn refused(lane: &Lane, profile: &ConfinementProfile, what: &str) -> ConfinementError {
    let admitted = lane
        .adapter
        .admit(profile)
        .unwrap_or_else(|error| panic!("{what}: admission reads no path: {error}"));
    let command = Command {
        argv: vec![text(&tool("sh")), "-c".into(), ": > /workspace/ran".into()],
        env: BTreeMap::new(),
    };
    let started = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        lane.adapter.start(&admitted, &command),
    )
    .await
    .unwrap_or_else(|_| panic!("{what}: the start did not answer in 30 s"));
    let Err(error) = started else {
        panic!("{what}: the start was dispatched");
    };
    assert!(
        !lane.workspace.join("ran").exists(),
        "{what}: the program ran"
    );
    error
}

fn read_only_root(host_path: &Path, mount: &str) -> ReadOnlyRoot {
    ReadOnlyRoot {
        host_path: text(host_path),
        mount: mount.to_owned(),
    }
}

/// `exec_gate.rs`, `MAX_LINKS`: an entry that names a cycle of symbolic links in a read-only
/// root is refused by name before dispatch, and the start answers.
#[tokio::test(flavor = "multi_thread")]
async fn adversary_an_entry_through_a_cycle_of_links_is_refused_before_dispatch() {
    const NAME: &str = "adversary exec list link cycle";
    let Some(lane) = lane(NAME) else {
        return;
    };
    let tools = lane.host.path().join("tools");
    std::fs::create_dir_all(&tools).expect("tools");
    std::os::unix::fs::symlink("b", tools.join("a")).expect("link");
    std::os::unix::fs::symlink("a", tools.join("b")).expect("link");
    std::os::unix::fs::symlink("self/x", tools.join("self")).expect("link");
    for entry in ["/opt/t/a", "/opt/t/self", "/opt/t/a/tool"] {
        let profile = ConfinementProfile {
            read_only_roots: vec![read_only_root(&tools, "/opt/t")],
            ..listed(&lane.workspace, &[entry])
        };
        let error = refused(&lane, &profile, entry).await;
        assert_eq!(error.code(), PATH_UNREPRESENTABLE, "{entry}: {error}");
        assert_eq!(error.status(), 422, "{entry}: {error}");
    }
    println!("{NAME}: executed");
}

/// `exec_gate.rs`, a mount that is a single file: a declared read-only file is an entry of its
/// own, admits that file only, and has nothing beneath it.
#[tokio::test(flavor = "multi_thread")]
async fn adversary_a_read_only_file_entry_admits_that_file_only() {
    const NAME: &str = "adversary exec list read-only file";
    let Some(lane) = lane(NAME) else {
        return;
    };
    let tools = lane.host.path().join("tools");
    std::fs::create_dir_all(&tools).expect("tools");
    std::fs::copy(tool("true"), tools.join("prog")).expect("copy true");
    let with = |entry: &str| ConfinementProfile {
        read_only_files: vec![ReadOnlyFile {
            host_path: text(&tools.join("prog")),
            mount: "/opt/f/prog".to_owned(),
        }],
        ..listed(&lane.workspace, &[entry])
    };
    let (code, stdout) = run(
        &lane.adapter,
        &with("/opt/f/prog"),
        "/opt/f/prog; echo $?; other/tool 2>/dev/null; echo $?; bin/tool 2>/dev/null; echo $?",
        &[],
    )
    .await;
    assert_eq!((stdout.as_str(), code), ("0\n126\n126\n", Some(0)));
    let error = refused(&lane, &with("/opt/f/prog/beneath"), "beneath a file mount").await;
    assert_eq!(error.code(), PATH_UNREPRESENTABLE, "{error}");
    assert_eq!(error.status(), 422, "{error}");
    println!("{NAME}: executed");
}

/// `exec_gate.rs`, a writable root: an entry beneath its mount is opened `beneath`, a link a run
/// left in it is refused, and so is a link in a read-only root that resolves into it.
#[tokio::test(flavor = "multi_thread")]
async fn adversary_an_entry_beneath_a_writable_root_is_held() {
    const NAME: &str = "adversary exec list writable root";
    let Some(lane) = lane_with(NAME, |config, host| {
        let out = host.join("out");
        for directory in ["bin", "other"] {
            std::fs::create_dir_all(out.join(directory)).expect(directory);
            std::fs::copy(tool("true"), out.join(directory).join("tool")).expect("copy true");
        }
        std::os::unix::fs::symlink("/usr/bin", out.join("planted")).expect("link");
        config.writable_roots = vec![confinement::DeclaredWritableRoot {
            name: "out".to_owned(),
            path: out,
        }];
    }) else {
        return;
    };
    let tools = lane.host.path().join("tools");
    std::fs::create_dir_all(&tools).expect("tools");
    std::os::unix::fs::symlink("/opt/w/bin", tools.join("into")).expect("link");
    let with = |entry: &str| ConfinementProfile {
        writable_roots: vec![WritableHostRoot {
            host: "out".to_owned(),
            mount: "/opt/w".to_owned(),
        }],
        read_only_roots: vec![read_only_root(&tools, "/opt/t")],
        ..listed(&lane.workspace, &[entry])
    };
    let (code, stdout) = run(
        &lane.adapter,
        &with("/opt/w/bin"),
        "/opt/w/bin/tool; echo $?; /opt/w/other/tool 2>/dev/null; echo $?; \
         other/tool 2>/dev/null; echo $?",
        &[],
    )
    .await;
    assert_eq!((stdout.as_str(), code), ("0\n126\n126\n", Some(0)));
    for entry in ["/opt/w/planted", "/opt/t/into"] {
        let error = refused(&lane, &with(entry), entry).await;
        assert_eq!(error.code(), PATH_UNREPRESENTABLE, "{entry}: {error}");
        assert_eq!(error.status(), 422, "{entry}: {error}");
    }
    println!("{NAME}: executed");
}

/// **The driver has checked every entry before dispatch** (design 35 § 6, *When the helper
/// fails*): "a helper failure means the kernel changed its answer after the probe or a path
/// changed after the check", and § 12: every entry the view cannot carry is a named refusal of
/// the start.
///
/// The boundary is the kernel's 4096-byte path. The driver opens a followed entry beneath the
/// descriptor of its mount; the helper opens the text of `<root>/<rel>`. A read-only root whose
/// mount path is longer than its host path therefore gives an entry the driver can open and the
/// helper cannot. Nothing changes between the check and the helper.
#[tokio::test(flavor = "multi_thread")]
async fn adversary_an_entry_at_the_path_limit_is_refused_before_dispatch_or_runs() {
    const NAME: &str = "adversary exec list path limit";
    let Some(lane) = lane(NAME) else {
        return;
    };
    let deep = lane.host.path().join("deep");
    std::fs::create_dir(&deep).expect("deep");
    // Fifteen names of 255 bytes and their separators are 3840 bytes with the last separator.
    let level = "d".repeat(255);
    let last = "e".repeat(
        4090_usize
            .checked_sub(deep.as_os_str().len() + 1 + 3840)
            .filter(|length| (1..=255).contains(length))
            .expect("a temporary directory short enough for this case"),
    );
    let built = std::process::Command::new(tool("sh"))
        .current_dir(&deep)
        .args([
            "-c",
            &format!(
                "i=0; while [ $i -lt 15 ]; do mkdir {level} && cd {level} || exit 1; \
                 i=$((i+1)); done; mkdir {last}"
            ),
        ])
        .status()
        .expect("sh");
    assert!(built.success(), "the deep directory was not built");
    let rel = format!("{}/{last}", vec![level.as_str(); 15].join("/"));
    let mount = format!("/opt/{}", "m".repeat(250));
    let entry = format!("{mount}/{rel}");
    assert!(
        deep.as_os_str().len() + 1 + rel.len() <= 4095,
        "the host path opens"
    );
    assert!(
        entry.len() > 4095,
        "the view path is over the kernel's limit"
    );

    let profile = ConfinementProfile {
        read_only_roots: vec![read_only_root(&deep, &mount)],
        ..listed(&lane.workspace, &[&entry])
    };
    let admitted = lane
        .adapter
        .admit(&profile)
        .expect("admission reads no path");
    let command = Command {
        argv: vec![text(&tool("sh")), "-c".into(), "echo ran".into()],
        env: BTreeMap::new(),
    };
    match lane.adapter.start(&admitted, &command).await {
        Err(error) => {
            // A named refusal before dispatch is one of the two answers the design allows.
            assert!(
                matches!(error.status(), 422 | 501),
                "refused, but not by a start refusal: {error}"
            );
            println!("{NAME}: executed, refused before dispatch: {error}");
        }
        Ok(run) => {
            let ended = lane.adapter.wait(&run).await.expect("waited");
            let output = lane.adapter.output(&run).await.expect("output");
            // The diagnostic names the whole entry; its beginning and its cause are enough.
            let diagnostic = String::from_utf8_lossy(&output.stderr);
            let diagnostic = diagnostic.trim_end();
            assert_eq!(
                (
                    String::from_utf8_lossy(&output.stdout).as_ref(),
                    ended.exit.as_ref().and_then(|exit| exit.code),
                ),
                ("ran\n", Some(0)),
                "the start was dispatched with an entry the driver checked, and the helper \
                 ended the run: {} … {}",
                diagnostic.chars().take(40).collect::<String>(),
                diagnostic.rsplit(": ").next().unwrap_or_default()
            );
            println!("{NAME}: executed, the entry is carried");
        }
    }
}

/// D24: "The writable locations are `HostConfig.workspace_root`, the path of every
/// `HostConfig.writable_roots` entry … whether or not this start mounts them: another run can
/// write them." A read-only root whose host directory lies in a declared writable root that this
/// start does not mount is therefore classed `beneath`, and a link in it is refused.
#[tokio::test(flavor = "multi_thread")]
async fn adversary_a_root_inside_a_declared_writable_root_no_start_mounts_is_classed_beneath() {
    const NAME: &str = "adversary exec list unmounted writable root";
    let Some(lane) = lane_with(NAME, |config, host| {
        let tools = host.join("out/tools");
        std::fs::create_dir_all(tools.join("real")).expect("tools");
        std::fs::copy(tool("true"), tools.join("real/tool")).expect("copy true");
        std::os::unix::fs::symlink("/usr/bin", tools.join("link")).expect("link");
        config.writable_roots = vec![confinement::DeclaredWritableRoot {
            name: "out".to_owned(),
            path: host.join("out"),
        }];
    }) else {
        return;
    };
    let tools = lane.host.path().join("out/tools");
    let with = |entry: &str| ConfinementProfile {
        read_only_roots: vec![read_only_root(&tools, "/opt/k")],
        ..listed(&lane.workspace, &[entry])
    };
    let error = refused(&lane, &with("/opt/k/link"), "a link another run can write").await;
    assert_eq!(error.code(), PATH_UNREPRESENTABLE, "{error}");
    assert!(error.to_string().contains("symbolic link"), "{error}");
    let (code, stdout) = run(
        &lane.adapter,
        &with("/opt/k/real"),
        &format!(
            "/opt/k/real/tool; echo $?; /opt/k/link/true 2>/dev/null; echo $?; {} 2>/dev/null; \
             echo $?",
            tool("true").display()
        ),
        &[],
    )
    .await;
    assert_eq!((stdout.as_str(), code), ("0\n126\n126\n", Some(0)));
    println!("{NAME}: executed");
}

/// **A declared mount at the helper's own path must not become the helper, and is refused
/// before dispatch.** D29 seals the real helper into a memfd, tests its bytes (`check_linkage`)
/// and binds it at [`GATE_MOUNT`] (`/.substrate-exec-gate`). `validate_read_only_files` does not
/// reserve that path (`collides_with_owned_mount` lists only `RESERVED_MOUNTS` and the
/// aperture/capsule/scratch mounts), so the wire crate's rules admit a read-only file there.
///
/// The declared file is a copy of the host's `true`: were it to become the executed helper, it
/// would ignore the helper's argv, exit 0 and run nothing.
///
/// **Changed by the implementor**, on the coordinator's word and by the design's eighth revision
/// (D29, rule 7; § 16). As first written the case declared one read-only file of the workspace
/// at the path and passed on any outcome but the shadow running: on a refusal, and on a run the
/// helper ended with 125. On a start with a list the driver now refuses such a mount before
/// dispatch, by the code its kind has for a mount the driver owns, and that refusal is what the
/// case requires: for a read-only file in the workspace and one outside every writable
/// location, at the path and beneath it, for a read-only root and for a writable root. The
/// control is a start without a list, which has no helper: the same file at the same path is
/// bound and runs. The bind order that kept the shadow from the helper before, the gate bind
/// after every declared bind in `ProcessRuntime::command`, is unchanged.
#[tokio::test(flavor = "multi_thread")]
async fn adversary3_a_read_only_file_cannot_shadow_the_helper_mount() {
    const NAME: &str = "adversary3 exec list helper shadow";
    const AT: &str = "/.substrate-exec-gate";
    const BENEATH: &str = "/.substrate-exec-gate/inner";
    let Some(lane) = lane_with(NAME, |config, host| {
        let out = host.join("out");
        std::fs::create_dir_all(&out).expect("out");
        config.writable_roots = vec![confinement::DeclaredWritableRoot {
            name: "out".to_owned(),
            path: out,
        }];
    }) else {
        return;
    };
    let copy_of_true = |path: PathBuf| {
        std::fs::copy(tool("true"), &path).expect("a copy of true");
        std::fs::canonicalize(&path).expect("canonical")
    };
    let inside = copy_of_true(lane.workspace.join("shadow"));
    let tools = lane.host.path().join("tools");
    std::fs::create_dir_all(&tools).expect("tools");
    let tools = std::fs::canonicalize(&tools).expect("canonical");
    let outside = copy_of_true(tools.join("shadow"));
    let file = |host_path: &Path, mount: &str| ConfinementProfile {
        read_only_files: vec![ReadOnlyFile {
            host_path: text(host_path),
            mount: mount.to_owned(),
        }],
        ..listed(&lane.workspace, &["bin"])
    };
    let cases = [
        (
            "a read-only file of the workspace at the path",
            file(&inside, AT),
            "exec.read-only-file-mount-reserved",
        ),
        (
            "a read-only file outside every writable location at the path",
            file(&outside, AT),
            "exec.read-only-file-mount-reserved",
        ),
        (
            "a read-only file beneath the path",
            file(&outside, BENEATH),
            "exec.read-only-file-mount-reserved",
        ),
        (
            "a read-only root at the path",
            ConfinementProfile {
                read_only_roots: vec![read_only_root(&tools, AT)],
                ..listed(&lane.workspace, &["bin"])
            },
            "exec.read-only-root-invalid",
        ),
        (
            "a writable root at the path",
            ConfinementProfile {
                writable_roots: vec![WritableHostRoot {
                    host: "out".to_owned(),
                    mount: AT.to_owned(),
                }],
                ..listed(&lane.workspace, &["bin"])
            },
            "exec.writable-root-mount-reserved",
        ),
    ];
    let mut seen = Vec::new();
    for (what, profile, code) in &cases {
        let refusal = refused(&lane, profile, what).await;
        assert_eq!(
            (refusal.code(), refusal.status()),
            (*code, 422),
            "{what}: {refusal}"
        );
        seen.push(format!("{what}: {}", refusal.code()));
    }
    // Without a list there is no helper and the driver owns no mount there: the same file at
    // the same path is bound, and the run executes it.
    let unlisted = ConfinementProfile {
        exec_subpaths: Vec::new(),
        ..file(&outside, AT)
    };
    let (code, stdout) = run(
        &lane.adapter,
        &unlisted,
        "/.substrate-exec-gate; echo $?",
        &[],
    )
    .await;
    assert_eq!((stdout.as_str(), code), ("0\n", Some(0)));
    println!("{NAME}: executed, refused before dispatch: {seen:?}");
}
