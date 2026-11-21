//! Design 35 § 8 and § 16, `story:executable-deny-list`: runs on the `linux` adapter held to an
//! executable allow-list and a deny-list beside it.
//!
//! The story's first two cases, probes K2 and K8 through the driver, stand at the end of the
//! parent file. Each case here opens the real adapter with `substrate-exec-gate`, starts runs
//! and reads what the runs themselves observed, as the parent's cases do; a case named
//! `limit_…` asserts a limit of § 8 as it is.

use super::*;

/// Admits `profile`, runs `argv` with no variable of its own and returns its exit code and its
/// stdout.
async fn run_argv(
    adapter: &Arc<dyn ConfinementAdapter>,
    profile: &ConfinementProfile,
    argv: &[&str],
) -> (Option<i32>, String) {
    let admitted = adapter.admit(profile).expect("admitted");
    let command = Command {
        argv: argv.iter().map(|part| (*part).to_owned()).collect(),
        env: BTreeMap::new(),
    };
    let run = adapter.start(&admitted, &command).await.expect("started");
    let ended = adapter.wait(&run).await.expect("waited");
    assert_eq!(ended.state, RunState::Ended, "{ended:?}");
    let output = adapter.output(&run).await.expect("output");
    (
        ended.exit.as_ref().and_then(|exit| exit.code),
        String::from_utf8_lossy(&output.stdout).into_owned(),
    )
}

/// The base directories and `more`, as the entries of a profile.
fn base_and(more: &[&str]) -> Vec<String> {
    let mut entries = base_directories();
    entries.extend(more.iter().map(|entry| (*entry).to_owned()));
    entries
}

/// [`listed_and_denied`] with the base directories beside `more`.
fn base_listed_and_denied(workspace: &Path, more: &[&str], denied: &[&str]) -> ConfinementProfile {
    let entries = base_and(more);
    let entries: Vec<&str> = entries.iter().map(String::as_str).collect();
    listed_and_denied(workspace, &entries, denied)
}

/// Whether `path` is a file of its own directory: a regular file and not a link to one.
fn own_file(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_file())
}

/// Two programs that are files of `/usr/bin` itself on this host and exit 0 given `--version`,
/// neither of them the shell the cases run; or why this host has none. Where a distribution
/// keeps a tool as a link out of `/usr/bin`, listing the directory does not admit the tool, so
/// such a name is passed over.
fn two_programs_of_usr_bin() -> Result<[String; 2], String> {
    let directory = Path::new("/usr/bin");
    if std::fs::canonicalize(directory).ok().as_deref() != Some(directory) {
        return Err("/usr/bin is not a directory under its own name".to_owned());
    }
    let shell = std::fs::canonicalize(tool("sh")).expect("the shell's file");
    let found: Vec<String> = ["sed", "grep", "gzip", "tar", "find", "diff"]
        .iter()
        .map(|name| directory.join(name))
        .filter(|path| own_file(path) && *path != shell)
        .map(|path| text(&path))
        .take(2)
        .collect();
    <[String; 2]>::try_from(found).map_err(|found| {
        format!("/usr/bin holds fewer than two of the probed programs as its own files: {found:?}")
    })
}

/// **The story's acceptance.** A profile allows `/usr/bin` and denies one program beneath it:
/// another program of `/usr/bin` runs, and an execution of the denied program fails inside the
/// run with status 126. The adapter proved `exec_denied_paths`, the admission record names
/// both facts, and the rendered argv carries the entry as one `--deny` option of the helper.
#[tokio::test(flavor = "multi_thread")]
async fn a_denied_program_beneath_a_listed_directory_fails_and_its_neighbour_runs() {
    const NAME: &str = "ESS confinement linux exec deny-list";
    let Some(lane) = lane(NAME) else {
        return;
    };
    let [denied, neighbour] = match two_programs_of_usr_bin() {
        Ok(programs) => programs,
        Err(reason) => {
            println!("{NAME}: ABSENT ({reason})");
            return;
        }
    };
    assert!(
        lane.adapter.facts().proves(&Fact::ExecDeniedPaths),
        "linux opened with the helper did not prove exec_denied_paths: {:?}",
        lane.adapter.facts()
    );
    let profile = listed_and_denied(&lane.workspace, &["/usr/bin"], &[&denied]);
    let (code, stdout) = run(
        &lane.adapter,
        &profile,
        &format!(
            "{neighbour} --version >/dev/null 2>&1; echo $?; \
             {denied} --version >/dev/null 2>&1; echo $?"
        ),
    )
    .await;
    assert_eq!(
        stdout, "0\n126\n",
        "{neighbour} ran and {denied}, denied beneath the listed /usr/bin, did not"
    );
    assert_eq!(code, Some(0));

    let (profile, records) = audited(profile);
    let admitted = lane.adapter.admit(&profile).expect("admitted");
    assert_eq!(
        records.lock()[0]["facts"],
        serde_json::json!(["exec_subpaths", "exec_denied_paths"]),
        "the admission names both facts"
    );
    let args = lane
        .adapter
        .render(&admitted, &shell("true"))
        .expect("rendered")
        .args;
    let deny: Vec<String> = args
        .windows(4)
        .filter(|window| window[0] == "--deny")
        .map(|window| format!("{}/{}", window[2], window[3]))
        .collect();
    assert_eq!(deny, [denied], "one --deny option, the entry: {args:?}");
    println!("{NAME}: executed");
}

/// A directory outside the workspace root whose `bin` holds `real` and `other`, copies of the
/// host's shell, and `sh`, a symbolic link to `real`: the shape of a host whose `/usr/bin/sh`
/// is a link to `bash`.
fn shells(host: &Path) -> PathBuf {
    let shells = host.join("shells");
    let bin = shells.join("bin");
    std::fs::create_dir_all(&bin).expect("bin");
    let shell = std::fs::canonicalize(tool("sh")).expect("the shell's file");
    for name in ["real", "other"] {
        std::fs::copy(&shell, bin.join(name)).expect("copy the shell");
    }
    std::os::unix::fs::symlink("real", bin.join("sh")).expect("the link");
    shells
}

/// **D23, a permanent case of the lane: a program that is a link to a denied program** (B1,
/// B2). A read-only root holds the program `real` and `sh`, a symbolic link to it; the profile
/// lists the root and denies `real`. A start whose program is `sh` is refused
/// `exec.sandbox-program-unlisted` before dispatch, with its audit record and nothing run. A
/// start with another program of the root runs, and its executions of `real` and of `sh` both
/// fail with status 126.
#[tokio::test(flavor = "multi_thread")]
async fn a_program_that_is_a_link_to_a_denied_program_is_refused_before_dispatch() {
    const NAME: &str = "ESS confinement linux exec deny link to denied";
    let Some(lane) = lane(NAME) else {
        return;
    };
    let shells = shells(lane.host.path());
    let profile = ConfinementProfile {
        read_only_roots: vec![root(&shells, "/opt/s")],
        ..listed_and_denied(&lane.workspace, &["/opt/s"], &["/opt/s/bin/real"])
    };
    let marker = lane.workspace.join("ran");
    for (program, what) in [
        (
            "/opt/s/bin/sh",
            "a program that is a link to a denied program",
        ),
        ("/opt/s/bin/real", "the denied program itself"),
    ] {
        let error = assert_start_refused(
            &lane.adapter,
            profile.clone(),
            program,
            ("exec.sandbox-program-unlisted", 422),
            &marker,
            what,
        )
        .await;
        assert!(
            error.to_string().contains("/opt/s/bin/real"),
            "{what}: the refusal names the deny entry: {error}"
        );
    }
    // The control for the marker, and the second half: the same start with another program of
    // the root is dispatched, writes the marker, and cannot execute the denied file by either
    // name.
    let (code, stdout) = run_argv(
        &lane.adapter,
        &profile,
        &[
            "/opt/s/bin/other",
            "-c",
            ": > /workspace/ran; /opt/s/bin/real -c : 2>/dev/null; echo $?; \
             /opt/s/bin/sh -c : 2>/dev/null; echo $?; /opt/s/bin/other -c :; echo $?",
        ],
    )
    .await;
    assert_eq!(
        stdout, "126\n126\n0\n",
        "the denied program, the link to it, and their neighbour"
    );
    assert_eq!(code, Some(0));
    assert!(marker.exists(), "the control start ran");
    println!("{NAME}: executed");
}

/// **D23 and D25, a permanent case of the lane, the base view's form** (K5): `/bin` is a link
/// to `/usr/bin` on this host, so one directory has two names in the view. With `/usr/bin`
/// listed and `/bin/id` denied, `id` fails under both names and another program of `/usr/bin`
/// runs; the reverse, `/bin` listed and `/usr/bin/id` denied, gives the same. On a host where
/// `/bin` is not that link, or `id` is not a file of `/usr/bin` itself, the case says it is
/// absent and why, and is never passed.
#[tokio::test(flavor = "multi_thread")]
async fn a_program_denied_under_one_name_of_the_base_view_is_denied_under_both() {
    const NAME: &str = "ESS confinement linux exec deny two names base view";
    let Some(lane) = lane(NAME) else {
        return;
    };
    let linked = std::fs::symlink_metadata("/bin").is_ok_and(|metadata| metadata.is_symlink())
        && std::fs::canonicalize("/bin").ok().as_deref() == Some(Path::new("/usr/bin"));
    let neighbour = match two_programs_of_usr_bin() {
        Ok(_) if !linked => Err("/bin is not a link to /usr/bin on this host".to_owned()),
        Ok(_) if !own_file(Path::new("/usr/bin/id")) => {
            Err("/usr/bin/id is not a file of /usr/bin itself on this host".to_owned())
        }
        Ok([first, _]) => Ok(first),
        Err(reason) => Err(reason),
    };
    let neighbour = match neighbour {
        Ok(neighbour) => neighbour,
        Err(reason) => {
            println!("{NAME}: ABSENT ({reason})");
            return;
        }
    };
    let script = format!(
        "/usr/bin/id >/dev/null 2>&1; echo $?; /bin/id >/dev/null 2>&1; echo $?; \
         {neighbour} --version >/dev/null 2>&1; echo $?"
    );
    for (entry, denied) in [("/usr/bin", "/bin/id"), ("/bin", "/usr/bin/id")] {
        let profile = listed_and_denied(&lane.workspace, &[entry], &[denied]);
        let (code, stdout) = run(&lane.adapter, &profile, &script).await;
        assert_eq!(
            stdout, "126\n126\n0\n",
            "{entry} listed and {denied} denied: id under both names, and {neighbour}"
        );
        assert_eq!(code, Some(0));
    }
    println!("{NAME}: executed");
}

/// **The same permanent case, built from two declared roots, on every host**: one directory is
/// the read-only root `/opt/a` and the read-only root `/opt/b`. With `/opt/a` listed and
/// `/opt/b/tool` denied, `tool` fails under both names and its neighbour runs under both; the
/// text of the entry is beneath no listed path.
#[tokio::test(flavor = "multi_thread")]
async fn a_program_denied_under_one_of_two_roots_of_one_directory_is_denied_under_both() {
    const NAME: &str = "ESS confinement linux exec deny two names declared roots";
    let Some(lane) = lane(NAME) else {
        return;
    };
    let bin = tools_with_a_neighbour(lane.host.path()).join("bin");
    let profile = ConfinementProfile {
        read_only_roots: vec![root(&bin, "/opt/a"), root(&bin, "/opt/b")],
        ..base_listed_and_denied(&lane.workspace, &["/opt/a"], &["/opt/b/tool"])
    };
    let (code, stdout) = run(
        &lane.adapter,
        &profile,
        "/opt/a/tool 2>/dev/null; echo $?; /opt/b/tool 2>/dev/null; echo $?; \
         /opt/a/other; echo $?; /opt/b/other; echo $?",
    )
    .await;
    assert_eq!(
        stdout, "126\n126\n0\n0\n",
        "the denied file under both mounts, and its neighbour under both"
    );
    assert_eq!(code, Some(0));
    println!("{NAME}: executed");
}

/// D13 and D10's bound on `linux`: a deny-list beside an empty `exec_subpaths` is refused
/// `confinement.exec-denied-paths-without-subpaths` (422) at admission, and 65 deny entries
/// beside an allow-list `confinement.exec-denied-paths-exceeded` (422). Each writes its
/// `profile_refused` record. No admitted profile exists after either, so nothing can be
/// started; 64 entries are admitted.
#[tokio::test(flavor = "multi_thread")]
async fn a_deny_list_without_an_allow_list_or_over_its_bound_is_refused_at_admission() {
    const NAME: &str = "ESS confinement linux exec deny-only";
    let Some(lane) = lane(NAME) else {
        return;
    };
    let entries = |count: usize| -> Vec<String> {
        (0..count)
            .map(|index| format!("bin/denied-{index}"))
            .collect()
    };
    let over = entries(confinement::MAX_EXEC_DENIED_PATHS + 1);
    let over: Vec<&str> = over.iter().map(String::as_str).collect();
    let cases = [
        (
            listed_and_denied(&lane.workspace, &[], &["bin/tool"]),
            "confinement.exec-denied-paths-without-subpaths",
        ),
        (
            listed_and_denied(&lane.workspace, &["bin"], &over),
            "confinement.exec-denied-paths-exceeded",
        ),
    ];
    for (profile, code) in cases {
        let (profile, records) = audited(profile);
        let error = lane.adapter.admit(&profile).expect_err(code);
        assert_eq!((error.code(), error.status()), (code, 422), "{error}");
        let records = records.lock();
        assert_eq!(records.len(), 1, "{records:?}");
        assert_eq!(records[0]["event"], "profile_refused", "{records:?}");
        assert_eq!(records[0]["code"], code, "{records:?}");
    }
    let at = entries(confinement::MAX_EXEC_DENIED_PATHS);
    let at: Vec<&str> = at.iter().map(String::as_str).collect();
    let (code, stdout) = run(
        &lane.adapter,
        &listed_and_denied(&lane.workspace, &["bin"], &at),
        "bin/tool; echo $?",
    )
    .await;
    assert_eq!(
        (stdout.as_str(), code),
        ("0\n", Some(0)),
        "64 deny entries, none of which exists, are admitted and deny nothing that exists"
    );
    println!("{NAME}: executed");
}

/// D11 through the driver (N1, N2, N5, N6): beside the listed workspace directory `bin`, the
/// profile denies `bin/tool`, the directory `bin/sub` and `bin/later`, which does not exist at
/// the start. The neighbour runs. The denied file, its hard link in the same directory, a
/// symbolic link to it, a program beneath the denied directory, a file created in the expanded
/// directory during the run, and the deny entry once the run creates it, each fail with status
/// 126. The denied file stays readable.
#[tokio::test(flavor = "multi_thread")]
async fn a_denied_file_is_denied_under_every_name_beside_it_and_a_denied_directory_whole() {
    const NAME: &str = "ESS confinement linux exec deny shapes";
    let Some(lane) = lane(NAME) else {
        return;
    };
    let bin = lane.workspace.join("bin");
    std::fs::create_dir(bin.join("sub")).expect("sub");
    for name in ["other", "sub/tool"] {
        std::fs::copy(tool("true"), bin.join(name)).expect("copy true");
    }
    std::fs::hard_link(bin.join("tool"), bin.join("again")).expect("a hard link");
    std::os::unix::fs::symlink("tool", bin.join("link")).expect("a link");
    let profile = base_listed_and_denied(
        &lane.workspace,
        &["bin"],
        &["bin/tool", "bin/sub", "bin/later"],
    );
    let (code, stdout) = run(
        &lane.adapter,
        &profile,
        "bin/other; echo $?; bin/tool 2>/dev/null; echo $?; bin/again 2>/dev/null; echo $?; \
         bin/link 2>/dev/null; echo $?; bin/sub/tool 2>/dev/null; echo $?; \
         [ -r bin/tool ]; echo $?; \
         cp bin/other bin/new && bin/new 2>/dev/null; echo $?; \
         cp bin/other bin/later && bin/later 2>/dev/null; echo $?",
    )
    .await;
    assert_eq!(
        stdout, "0\n126\n126\n126\n126\n0\n126\n126\n",
        "the neighbour; the denied file, its hard link, a link to it and a program beneath the \
         denied directory; that the file is readable; a new file of the expanded directory and \
         the deny entry created during the run"
    );
    assert_eq!(code, Some(0));
    println!("{NAME}: executed");
}

/// D23, the loader (B3): a deny entry that is the loader the host's base programs name gets no
/// rule, and the driver does not refuse the start for it. A dynamically linked program does not
/// start, the run ends with status 126; a statically linked program of the listed directory
/// runs.
#[tokio::test(flavor = "multi_thread")]
async fn a_denied_loader_starts_no_dynamic_program_and_a_static_one_runs() {
    const NAME: &str = "ESS confinement linux exec deny loader";
    let Some(lane) = lane(NAME) else {
        return;
    };
    let shell_file = std::fs::read(tool("sh")).expect("the shell");
    let Some(loader) = substrate_exec_gate::elf_interpreter(&shell_file) else {
        println!("{NAME}: ABSENT (the host's shell names no loader)");
        return;
    };
    let Some(program) = static_program(
        NAME,
        &lane.workspace.join("bin"),
        "static",
        "#include <unistd.h>\nint main(void) { return write(1, \"static\\n\", 7) == 7 ? 0 : 1; }\n",
        &[],
    ) else {
        return;
    };
    assert!(program.is_file());
    let profile = base_listed_and_denied(&lane.workspace, &["bin"], &[&loader]);
    let (code, stdout) = run(&lane.adapter, &profile, "echo ran").await;
    assert_eq!(
        (stdout.as_str(), code),
        ("", Some(126)),
        "the dynamically linked shell under a denied loader"
    );
    let (code, stdout) = run_argv(&lane.adapter, &profile, &["/workspace/bin/static"]).await;
    assert_eq!(
        (stdout.as_str(), code),
        ("static\n", Some(0)),
        "a statically linked program needs no loader"
    );
    println!("{NAME}: executed");
}

/// The limit of design 35 § 8.5 (N4): **a deny entry denies a path and every mount of it, not
/// every hard link to the file.** A hard link that existed before the start in another listed
/// directory, one the deny-list does not expand, runs.
#[tokio::test(flavor = "multi_thread")]
async fn limit_a_hard_link_that_existed_in_another_listed_directory_runs() {
    const NAME: &str = "ESS confinement linux exec deny limit hard link";
    let Some(lane) = lane(NAME) else {
        return;
    };
    std::fs::hard_link(
        lane.workspace.join("bin/tool"),
        lane.workspace.join("other/same"),
    )
    .expect("a hard link");
    let (code, stdout) = run(
        &lane.adapter,
        &listed_and_denied(&lane.workspace, &["bin", "other"], &["bin/tool"]),
        "bin/tool 2>/dev/null; echo $?; other/same; echo $?",
    )
    .await;
    assert_eq!(
        stdout, "126\n0\n",
        "the limit holds as stated: the denied file runs under its other name"
    );
    assert_eq!(code, Some(0));
    println!("{NAME}: executed, a hard link elsewhere beneath a listed path runs");
}

/// The limit of design 35 § 8.4 (W4, N3): **a run that can write and execute files in a listed
/// directory copies a denied program there and runs the copy.** A deny-list means something
/// only beside an allow-list that admits no directory the run can write.
#[tokio::test(flavor = "multi_thread")]
async fn limit_a_copy_in_a_listed_writable_directory_runs() {
    const NAME: &str = "ESS confinement linux exec deny limit copy";
    let Some(lane) = lane(NAME) else {
        return;
    };
    let (code, stdout) = run(
        &lane.adapter,
        &base_listed_and_denied(&lane.workspace, &["bin", "other"], &["bin/tool"]),
        "bin/tool 2>/dev/null; echo $?; cp bin/tool other/copy && other/copy; echo $?",
    )
    .await;
    assert_eq!(stdout, "126\n0\n", "the limit holds as stated");
    assert_eq!(code, Some(0));
    println!("{NAME}: executed, a copy of the denied program runs");
}

/// The limit of design 35 § 8 (B8, A8): **the loader given a denied program runs it.** A deny
/// entry fails `execve` of its path; the loader maps the file and executes nothing.
#[tokio::test(flavor = "multi_thread")]
async fn limit_the_loader_runs_a_denied_program() {
    const NAME: &str = "ESS confinement linux exec deny limit loader";
    let Some(lane) = lane(NAME) else {
        return;
    };
    let copied = std::fs::read(lane.workspace.join("bin/tool")).expect("the tool");
    let Some(loader) = substrate_exec_gate::elf_interpreter(&copied) else {
        // A statically linked `true` has no loader to be given to, so the limit has no subject.
        println!("{NAME}: executed, the host's true names no loader");
        return;
    };
    let (code, stdout) = run(
        &lane.adapter,
        &listed_and_denied(&lane.workspace, &["bin"], &["bin/tool"]),
        &format!("bin/tool 2>/dev/null; echo $?; {loader} bin/tool; echo $?"),
    )
    .await;
    assert_eq!(
        stdout, "126\n0\n",
        "denied by path, run by the loader: the limit holds as stated"
    );
    assert_eq!(code, Some(0));
    println!("{NAME}: executed, {loader} runs a denied program");
}

/// The limit of design 35 § 8 (B8, A7): **`sh` given a denied script runs it.** The kernel asks
/// the execute right of the script file only when the file itself is executed.
#[tokio::test(flavor = "multi_thread")]
async fn limit_sh_runs_a_denied_script() {
    const NAME: &str = "ESS confinement linux exec deny limit script";
    let Some(lane) = lane(NAME) else {
        return;
    };
    let script = lane.workspace.join("bin/script.sh");
    std::fs::write(
        &script,
        format!("#!{}\necho the script ran\n", tool("sh").display()),
    )
    .expect("script");
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).expect("mode");
    let (code, stdout) = run(
        &lane.adapter,
        &base_listed_and_denied(&lane.workspace, &["bin"], &["bin/script.sh"]),
        "bin/script.sh 2>/dev/null; echo $?; sh bin/script.sh; echo $?",
    )
    .await;
    assert_eq!(
        stdout, "126\nthe script ran\n0\n",
        "the limit holds as stated"
    );
    assert_eq!(code, Some(0));
    println!("{NAME}: executed, sh runs a denied script");
}

/// The limit of design 35 § 8.2 (B9): **a program rebuilt in an expanded directory loses the
/// execute right.** `bin` holds a deny entry, so it has no rule of its own and each file in it
/// has one. A file overwritten in place keeps its rule; one removed and written again is a new
/// file without one.
#[tokio::test(flavor = "multi_thread")]
async fn limit_a_program_replaced_in_an_expanded_directory_no_longer_runs() {
    const NAME: &str = "ESS confinement linux exec deny limit replaced";
    let Some(lane) = lane(NAME) else {
        return;
    };
    std::fs::copy(tool("true"), lane.workspace.join("bin/built")).expect("copy true");
    let (code, stdout) = run(
        &lane.adapter,
        &base_listed_and_denied(&lane.workspace, &["bin"], &["bin/tool"]),
        "cat other/tool > bin/built; bin/built; echo $?; \
         rm bin/built && cp other/tool bin/built && bin/built 2>/dev/null; echo $?",
    )
    .await;
    assert_eq!(
        stdout, "0\n126\n",
        "overwritten in place, then removed and written again: the limit holds as stated"
    );
    assert_eq!(code, Some(0));
    println!("{NAME}: executed, a replaced program is no longer executable");
}

/// D15 with D30 (§ 9): **`exec_denied_paths` is proved only where the helper held a deny
/// entry in a mount beneath a listed directory.** The deny probe binds a directory no entry
/// lists at `apart/held`, beneath its listed `apart`, and denies `apart/held/tool`. By place
/// that file lies in no listed directory, so a helper that compares places alone writes one
/// rule on `apart` and the file runs. The helper here restricts as the lane's does and has
/// that one defect in the form the probe can see: it holds every deny entry but that one.
/// The open proves `exec_subpaths` and not `exec_denied_paths`; a profile with both lists is
/// refused by the second fact's name, and one with an allow-list alone is admitted. The
/// lane's own helper proves both facts in every other case of this file.
#[tokio::test(flavor = "multi_thread")]
async fn a_helper_that_misses_a_mount_beneath_a_listed_directory_proves_the_allow_list_alone() {
    const NAME: &str = "ESS confinement linux exec deny probe held mount";
    let Some(cgroup_root) = delegated(NAME) else {
        return;
    };
    let host = tempfile::tempdir().expect("host root");
    let Some(gate) = defective(
        NAME,
        host.path(),
        "gate-drops-held-deny",
        &["-DDROP_HELD_DENY"],
    ) else {
        return;
    };
    let adapter = open_with(host.path(), &cgroup_root, Some(gate)).expect("linux opens");
    assert!(
        adapter.facts().proves(&Fact::ExecSubpaths),
        "the allow-list is proved by its own probe: {:?}",
        adapter.facts()
    );
    assert!(
        !adapter.facts().proves(&Fact::ExecDeniedPaths),
        "a helper that does not hold the deny entry in the mount beneath `apart` proved the \
         deny-list: {:?}",
        adapter.facts()
    );
    let workspace = workspace(host.path());
    let error = adapter
        .admit(&listed_and_denied(&workspace, &["bin"], &["bin/tool"]))
        .expect_err("both lists on a host that proved the allow-list alone");
    assert!(
        matches!(
            &error,
            ConfinementError::FactUnserved { fact, .. } if *fact == Fact::ExecDeniedPaths
        ),
        "{error}"
    );
    assert_eq!(error.status(), 501);
    adapter
        .admit(&listed(&workspace, &["bin"]))
        .expect("an allow-list alone on a host that proved it");
    println!("{NAME}: executed");
}
