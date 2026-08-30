//! Adversarial cases against `story:pty-sessions`' acceptance statement, driven through the
//! *public* host API a daemon actually calls — `start_pipe_session`, `write_pipe_session`,
//! `read_pipe_session`, `resize_pty_session` — rather than through the startup probe's private
//! helpers.
//!
//! The unit's acceptance reads: "The delegated lane runs an interactive shell through a `pty`
//! session, echoes bytes, applies a resize the child observes, and exits on hangup". The cases the
//! change shipped observe a resize only inside `pty::observe_sandboxed_window`, which is the probe,
//! not a session: it never calls `start_pipe`, never calls `resize_pty`, and never writes a byte
//! through `write_pipe`. These do.
//!
//! Delegated lane only. Without `SUBSTRATE_VECTORS_CGROUP_ROOT` naming a cgroup v2 subtree this
//! process is inside, each case is **absent, never reported as passed** (invariant 3).

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use substrate_host::{DispatchOutcome, Driver as _, HostConfig, HostDriver};
use substrate_wire::{
    ConfinementRequest, ExecEnvironment, ExecLimits, ExecStartInput, NetworkMode,
    PipeSessionStartInput, PtyWindow, SandboxProfile, SessionMode,
};

/// The delegated cgroup v2 subtree this process is inside, or nothing.
///
/// The *root itself*, exactly as `scripts/delegated-lane.sh` hands it to the daemon: `probe_cgroup`
/// requires the configured root to be an ancestor of this process's own cgroup, so a private child
/// would leave `exec` false and the pty fact absent for a reason that is not this change's.
fn delegated_cgroup_root() -> Option<PathBuf> {
    let root = PathBuf::from(std::env::var_os("SUBSTRATE_VECTORS_CGROUP_ROOT")?);
    std::fs::write(root.join("cgroup.subtree_control"), "+cpu +memory +pids").ok()?;
    Some(root)
}

struct Lane {
    driver: Arc<HostDriver>,
    snapshot: String,
    _directory: tempfile::TempDir,
}

/// A host driver whose own startup probe published `sessions.pty`, or `None` when this machine
/// cannot make the claim at all.
fn lane() -> Option<Lane> {
    let delegated = delegated_cgroup_root()?;
    if !PathBuf::from("/usr/bin/bwrap").is_file() {
        return None;
    }
    let directory = tempfile::tempdir().expect("temporary host root");
    let mut config = HostConfig::minimum(directory.path().join("workspaces"));
    config.cgroup_root = Some(delegated);
    let driver = HostDriver::open(config).expect("host driver");
    let machine = driver.machine();
    assert_eq!(
        machine.facts.sessions_pty,
        Some(true),
        "the delegated lane's own probe must publish sessions.pty, or no session case below can \
         make its claim"
    );
    std::fs::create_dir_all(driver.root().join("ws_test")).expect("workspace directory");
    Some(Lane {
        driver,
        snapshot: machine.snapshot,
        _directory: directory,
    })
}

fn pty_start(snapshot: &str, argv: &[&str], window: PtyWindow) -> PipeSessionStartInput {
    PipeSessionStartInput {
        exec: ExecStartInput {
            workspace: "ws_test".to_owned(),
            argv: argv.iter().map(|part| (*part).to_owned()).collect(),
            env: ExecEnvironment {
                allow: vec![],
                set: BTreeMap::new(),
            },
            sandbox: ConfinementRequest {
                capability_snapshot: snapshot.to_owned(),
                network: NetworkMode::None,
                aperture: None,
                profile: SandboxProfile::Workspace,
                required: true,
            },
            limits: ExecLimits {
                timeout_ms: 60_000,
                output_bytes: 1_048_576,
                processes: 16,
                memory_bytes: 67_108_864,
                cpu_millis: 5_000,
            },
            wait: false,
            read_only_roots: Vec::new(),
            secret_slots: Vec::new(),
            capsule: None,
            lease_ttl_ms: Some(60_000),
        },
        input_limit_bytes: 65_536,
        frame_limit_bytes: 4_096,
        queued_frames: 8,
        mode: SessionMode::Pty,
        window: Some(window),
    }
}

/// Reads frames until `needle` shows up in the accumulated transcript, or the deadline passes.
async fn read_until(driver: &HostDriver, id: &str, needle: &str, transcript: &mut Vec<u8>) -> bool {
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while std::time::Instant::now() < deadline {
        if String::from_utf8_lossy(transcript).contains(needle) {
            return true;
        }
        match driver.read_pipe_session(id, Duration::from_secs(5)).await {
            Ok(Some(frame)) => transcript.extend_from_slice(&frame.bytes),
            Ok(None) => break,
            Err(_deadline) => {}
        }
    }
    String::from_utf8_lossy(transcript).contains(needle)
}

/// The acceptance statement itself, through the session API: an interactive shell on a terminal,
/// bytes echoed back by the line discipline, and a resize the **child** observes with `TIOCGWINSZ`
/// after `resize_pty_session` — not after a probe helper.
#[tokio::test(flavor = "multi_thread")]
async fn pty_session_echoes_bytes_and_the_child_observes_a_resize() {
    let Some(lane) = lane() else {
        return;
    };
    let start = pty_start(
        &lane.snapshot,
        &["/bin/sh"],
        PtyWindow {
            columns: 97,
            rows: 37,
        },
    );
    match lane
        .driver
        .start_pipe_session("ex_ptyecho", "ws_test", &start)
        .await
    {
        DispatchOutcome::Observed(_) => {}
        DispatchOutcome::NotDispatched(error)
        | DispatchOutcome::ContainedAbsent(error)
        | DispatchOutcome::OutcomeUnknown(error) => {
            panic!(
                "the delegated lane must dispatch a pty session: {} {}",
                error.code, error.message
            )
        }
    }

    let mut transcript = Vec::new();
    lane.driver
        .write_pipe_session("ex_ptyecho", b"/usr/bin/stty size\n")
        .await
        .expect("a pty session takes input at the master");
    assert!(
        read_until(
            &lane.driver,
            "ex_ptyecho",
            "/usr/bin/stty size",
            &mut transcript
        )
        .await,
        "the line discipline must echo the client's own bytes back: {}",
        String::from_utf8_lossy(&transcript)
    );
    assert!(
        read_until(&lane.driver, "ex_ptyecho", "37 97", &mut transcript).await,
        "the child must read back the window the start declared: {}",
        String::from_utf8_lossy(&transcript)
    );

    lane.driver
        .resize_pty_session(
            "ex_ptyecho",
            PtyWindow {
                columns: 132,
                rows: 43,
            },
        )
        .expect("a live pty session takes a resize");
    lane.driver
        .write_pipe_session("ex_ptyecho", b"/usr/bin/stty size\n")
        .await
        .expect("a pty session takes input at the master");
    assert!(
        read_until(&lane.driver, "ex_ptyecho", "43 132", &mut transcript).await,
        "the child must observe the resize the session applied: {}",
        String::from_utf8_lossy(&transcript)
    );
}

/// `resize_pty`'s own contract, verbatim: "Returns a typed refusal when the exec is not a **live**
/// pty session, and a failure when the kernel refuses the resize"
/// (`crates/substrate-host/src/process.rs:756-760`), and the driver port repeats it as "reports a
/// kernel refusal without pretending the child saw the new size"
/// (`crates/substrate-host/src/lib.rs:574-578`).
///
/// A session whose child has already exited is not a live pty session. The master descriptor
/// outlives the child, so `TIOCSWINSZ` still succeeds on it and the call reports success for a
/// window no process will ever read.
#[tokio::test(flavor = "multi_thread")]
async fn a_resize_after_the_child_exited_is_refused_rather_than_reported_applied() {
    let Some(lane) = lane() else {
        return;
    };
    let start = pty_start(
        &lane.snapshot,
        &["/bin/sh", "-c", "printf 'done\\n'"],
        PtyWindow {
            columns: 97,
            rows: 37,
        },
    );
    match lane
        .driver
        .start_pipe_session("ex_ptygone", "ws_test", &start)
        .await
    {
        DispatchOutcome::Observed(_) => {}
        DispatchOutcome::NotDispatched(error)
        | DispatchOutcome::ContainedAbsent(error)
        | DispatchOutcome::OutcomeUnknown(error) => {
            panic!(
                "the delegated lane must dispatch a pty session: {} {}",
                error.code, error.message
            )
        }
    }
    // Drain to end of file: the child has exited and the terminal has hung up.
    let mut transcript = Vec::new();
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    loop {
        match lane
            .driver
            .read_pipe_session("ex_ptygone", Duration::from_secs(5))
            .await
        {
            Ok(Some(frame)) => transcript.extend_from_slice(&frame.bytes),
            Ok(None) => break,
            Err(_deadline) => {}
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the terminal never reached end of file: {}",
            String::from_utf8_lossy(&transcript)
        );
    }
    let observed = lane
        .driver
        .observe_exec("ex_ptygone")
        .await
        .expect("the session is still tracked");
    assert_ne!(
        observed.resource.state,
        substrate_wire::ExecState::Running,
        "the child must have finished before this case makes its claim"
    );

    let outcome = lane.driver.resize_pty_session(
        "ex_ptygone",
        PtyWindow {
            columns: 132,
            rows: 43,
        },
    );
    let error = outcome.err().unwrap_or_else(|| {
        panic!(
            "a resize on a session in state {:?} reported success; the client is told a child saw \
             a window no process will ever read",
            observed.resource.state
        )
    });
    // `session.not-pty` next door means the caller named the wrong kind of thing. This exec *is* a
    // pty session; what is wrong is that nothing is left to observe the window, so it has its own
    // code (`substrate_wire::SESSION_PTY_ENDED`).
    assert_eq!(error.code, substrate_wire::SESSION_PTY_ENDED);
}

/// Design 13's central claim, at the layer that ships it: **the confined child of a `pty` session
/// has a controlling terminal**, taken inside the sandbox after bubblewrap's own `setsid`.
///
/// Nothing in the suite asserts this about a session. `pty.rs`'s cases assert it about
/// `observe_sandboxed_terminal`, which is the startup probe's private sandbox — a different argv,
/// a different bind set, no cgroup and no workspace. Deleting
/// `command.args(crate::pty::CONTROLLING_TERMINAL_ARGV)` from `ProcessRuntime::command`
/// (`crates/substrate-host/src/process.rs:1347-1349`) leaves every shipped case green: `TIOCGWINSZ`
/// and `TIOCSWINSZ` both work on a terminal that is nobody's controlling terminal, so the echo and
/// the resize above cannot see the difference. Only the hangup can, and no shipped case exercises a
/// hangup through a session.
///
/// Field 7 of `/proc/self/stat` is `tty_nr`; zero means "no controlling terminal".
#[tokio::test(flavor = "multi_thread")]
async fn a_pty_session_child_has_a_controlling_terminal() {
    let Some(lane) = lane() else {
        return;
    };
    let start = pty_start(
        &lane.snapshot,
        &[
            "/bin/sh",
            "-c",
            "printf 'T:%s\\n' \"$(cut -d' ' -f7 /proc/self/stat)\"",
        ],
        PtyWindow {
            columns: 97,
            rows: 37,
        },
    );
    match lane
        .driver
        .start_pipe_session("ex_ptyctty", "ws_test", &start)
        .await
    {
        DispatchOutcome::Observed(_) => {}
        DispatchOutcome::NotDispatched(error)
        | DispatchOutcome::ContainedAbsent(error)
        | DispatchOutcome::OutcomeUnknown(error) => {
            panic!(
                "the delegated lane must dispatch a pty session: {} {}",
                error.code, error.message
            )
        }
    }
    let mut transcript = Vec::new();
    assert!(
        read_until(&lane.driver, "ex_ptyctty", "T:", &mut transcript).await,
        "the child never reported its controlling terminal: {}",
        String::from_utf8_lossy(&transcript)
    );
    let text = String::from_utf8_lossy(&transcript).into_owned();
    let reported = text
        .lines()
        .find_map(|line| line.trim().strip_prefix("T:"))
        .expect("a T: line")
        .trim()
        .to_owned();
    assert_ne!(
        reported, "0",
        "a pty session's child must have a controlling terminal; with tty_nr 0 there is no \
         foreground process group, so no SIGWINCH on a resize and no hangup when the master \
         closes (design 13). transcript: {text}"
    );
}

/// The acceptance says "a resize the child **observes**". `TIOCGWINSZ` answers with the pty's
/// current size whether or not the child was ever told the size changed, so reading it back proves
/// only that the ioctl landed. What a terminal application actually depends on is the signal, and
/// what makes the signal possible is the controlling terminal design 13 goes to the trouble of
/// acquiring. This asserts the signal.
#[tokio::test(flavor = "multi_thread")]
async fn a_pty_session_resize_delivers_sigwinch_to_the_child() {
    let Some(lane) = lane() else {
        return;
    };
    let start = pty_start(
        &lane.snapshot,
        &[
            "/bin/sh",
            "-c",
            "trap 'printf \"WINCH\\n\"' WINCH; printf 'READY\\n'; \
             i=0; while [ $i -lt 200 ]; do /usr/bin/sleep 0.1; i=$((i+1)); done",
        ],
        PtyWindow {
            columns: 97,
            rows: 37,
        },
    );
    match lane
        .driver
        .start_pipe_session("ex_ptywinch", "ws_test", &start)
        .await
    {
        DispatchOutcome::Observed(_) => {}
        DispatchOutcome::NotDispatched(error)
        | DispatchOutcome::ContainedAbsent(error)
        | DispatchOutcome::OutcomeUnknown(error) => {
            panic!(
                "the delegated lane must dispatch a pty session: {} {}",
                error.code, error.message
            )
        }
    }
    let mut transcript = Vec::new();
    assert!(
        read_until(&lane.driver, "ex_ptywinch", "READY", &mut transcript).await,
        "the shell never announced itself: {}",
        String::from_utf8_lossy(&transcript)
    );
    lane.driver
        .resize_pty_session(
            "ex_ptywinch",
            PtyWindow {
                columns: 132,
                rows: 43,
            },
        )
        .expect("a live pty session takes a resize");
    let observed = read_until(&lane.driver, "ex_ptywinch", "WINCH", &mut transcript).await;
    let _terminated = lane
        .driver
        .signal(
            "ex_ptywinch",
            &substrate_wire::ExecSignalInput {
                signal: substrate_wire::Signal::Kill,
                grace_ms: 0,
            },
        )
        .await;
    assert!(
        observed,
        "the kernel must signal the terminal's foreground process group on a resize, which is the \
         whole reason design 13 takes a controlling terminal inside the sandbox. transcript: {}",
        String::from_utf8_lossy(&transcript)
    );
}

/// Design 13 and ADR 0014: reaching the declared output bound **ends** a terminal session and says
/// so by name. Design 05 gave the pty no `truncated` frame, and a terminal stream has no offset to
/// resume from, so a transcript that silently stopped would be unrecoverable — which is the whole
/// reason the refusal field exists. Bundle `0.9.0` states it as
/// `contracts/substrate-wire/0.9.0/vectors/driver/pty-session-output-bound-ends-the-session.json`:
/// `state: cancelled`, `code: session.output-limit`, `truncated_frames_delivered: 0`.
///
/// The flag `drain_capped` raises is only ever read from the 1 ms supervision tick
/// (`crates/substrate-host/src/process.rs:1693`). A child that crosses the bound on its **last**
/// write and then exits loses that race: `child.wait()` wins the `select!`, `output_exhausted`
/// stays false, and `record_terminal_output_bound` (`:1642`) writes nothing — the session is
/// reported `exited`, code 0, no refusal, with a transcript that silently stopped at exactly the
/// declared bound. The aperture ceiling has `ceiling_reached` (`:1611`) asked once more after the
/// wait for precisely this reason, and its doc comment (`:1592-1606`) names the resulting shape as
/// the silent degradation invariant 3 forbids. The output bound has no such re-read, and the
/// `Arc<AtomicBool>` it would read is still in scope at `:1554`.
///
/// The race is won by the wrong side most of the time but not every time, so the requirement is
/// asserted over several runs: crossing the bound must name the bound on **every** run, not on
/// most of them.
#[tokio::test(flavor = "multi_thread")]
async fn a_pty_session_that_crosses_its_output_bound_on_the_last_write_still_names_the_bound() {
    let Some(lane) = lane() else {
        return;
    };
    for attempt in 0..6 {
        let id = format!("ex_ptyflood{attempt}");
        let mut start = pty_start(
            &lane.snapshot,
            &[
                "/bin/sh",
                "-c",
                // Two bursts: the first fits inside the 4096-byte bound, the second crosses it and
                // is the last thing this child does. Nothing separates crossing from exiting.
                "/usr/bin/head -c 4096 /dev/zero | /usr/bin/tr '\\0' 'a'; \
                 /usr/bin/head -c 4096 /dev/zero | /usr/bin/tr '\\0' 'b'",
            ],
            PtyWindow {
                columns: 97,
                rows: 37,
            },
        );
        start.exec.limits.output_bytes = 4_096;
        match lane.driver.start_pipe_session(&id, "ws_test", &start).await {
            DispatchOutcome::Observed(_) => {}
            DispatchOutcome::NotDispatched(error)
            | DispatchOutcome::ContainedAbsent(error)
            | DispatchOutcome::OutcomeUnknown(error) => {
                panic!(
                    "the delegated lane must dispatch a pty session: {} {}",
                    error.code, error.message
                )
            }
        }
        // Drain to end of file so the observation is final.
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        let mut bytes = 0_usize;
        loop {
            match lane
                .driver
                .read_pipe_session(&id, Duration::from_secs(5))
                .await
            {
                Ok(Some(frame)) => bytes += frame.bytes.len(),
                Ok(None) => break,
                Err(_deadline) => {}
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the terminal never reached end of file after {bytes} bytes"
            );
        }
        let observed = lane
            .driver
            .observe_exec(&id)
            .await
            .expect("the session is still tracked");
        assert!(
            observed.stdout_truncated,
            "attempt {attempt} only makes its claim when the bound was actually crossed"
        );
        assert_eq!(
            observed
                .resource
                .refusal
                .as_ref()
                .map(|refusal| refusal.code.as_str()),
            Some("session.output-limit"),
            "attempt {attempt}: the bound that ended the session must name itself (ADR 0014); \
             observed state {:?}, exit {:?}, {bytes} bytes delivered, stdout_truncated true",
            observed.resource.state,
            observed.resource.exit
        );
        assert_eq!(
            observed.resource.state,
            substrate_wire::ExecState::Cancelled,
            "attempt {attempt}: reaching the output bound ends a terminal session; it does not let \
             it exit cleanly"
        );
    }
}

/// The cell bound is a bound on the terminal, and the terminal lives here.
///
/// `MAX_PTY_WINDOW_COLUMNS`'s own doc says why 1000 and not 65535: "a 65535x65535 window is not a
/// display but an amplification knob, because programs allocate per-cell buffers when the size
/// changes and that allocation is spent from the run's own memory bound"
/// (`crates/substrate-wire/src/lib.rs:82-88`). `ProcessRuntime::start_pipe` enforces it on the
/// initial window through `validate_session_window` (`process.rs:295`). `ProcessRuntime::resize_pty`
/// (`process.rs:761`) enforces nothing: it hands whatever `PtyWindow` it is given straight to
/// `TIOCSWINSZ`. The only check on a mid-session window is in the daemon's frame decoder
/// (`crates/substrate-daemon/src/app/sessions.rs:1009`), so the driver port — which `Driver` exposes
/// as `resize_pty_session` and which any second daemon path would call — has the bound only by
/// convention.
///
/// This asserts the bound at the layer that owns the ioctl, and shows the child reading the
/// out-of-bounds size back.
#[tokio::test(flavor = "multi_thread")]
async fn the_driver_port_refuses_a_resize_outside_the_declared_cell_bounds() {
    let Some(lane) = lane() else {
        return;
    };
    let start = pty_start(
        &lane.snapshot,
        &["/bin/sh"],
        PtyWindow {
            columns: 97,
            rows: 37,
        },
    );
    match lane
        .driver
        .start_pipe_session("ex_ptyhuge", "ws_test", &start)
        .await
    {
        DispatchOutcome::Observed(_) => {}
        DispatchOutcome::NotDispatched(error)
        | DispatchOutcome::ContainedAbsent(error)
        | DispatchOutcome::OutcomeUnknown(error) => {
            panic!(
                "the delegated lane must dispatch a pty session: {} {}",
                error.code, error.message
            )
        }
    }
    let huge = PtyWindow {
        columns: u16::MAX,
        rows: u16::MAX,
    };
    let outcome = lane.driver.resize_pty_session("ex_ptyhuge", huge);
    if outcome.is_err() {
        return;
    }
    // The refusal did not happen, so show what the child now believes its terminal is.
    let mut transcript = Vec::new();
    lane.driver
        .write_pipe_session("ex_ptyhuge", b"/usr/bin/stty size\n")
        .await
        .expect("a pty session takes input at the master");
    let observed = read_until(&lane.driver, "ex_ptyhuge", "65535 65535", &mut transcript).await;
    let _terminated = lane
        .driver
        .signal(
            "ex_ptyhuge",
            &substrate_wire::ExecSignalInput {
                signal: substrate_wire::Signal::Kill,
                grace_ms: 0,
            },
        )
        .await;
    panic!(
        "the driver port applied a {}x{} window, which is {} times the declared ceiling of \
         {}x{}; the confined child read it back: {observed}. transcript: {}",
        huge.columns,
        huge.rows,
        u32::from(huge.columns) / u32::from(substrate_wire::MAX_PTY_WINDOW_COLUMNS),
        substrate_wire::MAX_PTY_WINDOW_COLUMNS,
        substrate_wire::MAX_PTY_WINDOW_ROWS,
        String::from_utf8_lossy(&transcript)
    );
}
