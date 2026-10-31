//! `substrate-executor`: the roles of design 34 § 7. Every role refuses an unknown subcommand, or
//! a known one with the wrong arguments, and exits 2.

#![allow(unsafe_code)]

use std::process::ExitCode;

#[cfg(target_os = "linux")]
fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let arguments: Vec<&str> = arguments.iter().map(String::as_str).collect();
    match arguments.as_slice() {
        ["version"] => {
            println!("{}", substrate_executor::Handshake::current().line());
            ExitCode::SUCCESS
        }
        ["keeper"] => {
            substrate_executor::keeper::keep();
            ExitCode::SUCCESS
        }
        ["fs"] => fs(),
        ["install", target] => install(target),
        ["run", parts @ ..] if !parts.is_empty() => run(parts),
        [substrate_executor::probe::PROBE_CHILD, ..] => probe_child(),
        _ => {
            eprintln!(
                "usage: substrate-executor version | keeper | fs | install <path> | run <spec part>... | probe-child <argument>..."
            );
            ExitCode::from(2)
        }
    }
}

#[cfg(not(target_os = "linux"))]
fn main() -> ExitCode {
    eprintln!("substrate-executor runs only on Linux");
    ExitCode::from(2)
}

/// The probe Pod's child (design 34 § 6.2): writes the argv it received, `argv[0]` included, as
/// one JSON line. An argument that is not UTF-8 is written lossily, so it can never read back as
/// the argv the driver sent. When its environment states readable probe targets, it then starts
/// the heartbeat grandchild, makes the egress attempts and writes what it observed as a second
/// line; unreadable targets observe nothing.
#[cfg(target_os = "linux")]
fn probe_child() -> ExitCode {
    use std::io::Write as _;
    use substrate_executor::probe::{ProbeTargets, TARGETS_ENV, observe, report};

    let argv: Vec<String> = std::env::args_os()
        .map(|part| part.to_string_lossy().into_owned())
        .collect();
    let mut text = report(&argv);
    let targets = std::env::var_os(TARGETS_ENV)
        .and_then(|value| value.into_string().ok())
        .and_then(|value| ProbeTargets::parse(&value).ok());
    if let Some(targets) = targets {
        text.push_str(&observe(&targets).line());
    }
    let mut out = std::io::stdout().lock();
    match out.write_all(text.as_bytes()).and_then(|()| out.flush()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(_) => ExitCode::FAILURE,
    }
}

/// One file operation from stdin, answered on stdout. Exit 0 means the request was answered,
/// whether with an observation or a refusal.
#[cfg(target_os = "linux")]
fn fs() -> ExitCode {
    let mut input = std::io::stdin().lock();
    let mut output = std::io::stdout().lock();
    let root = std::path::Path::new(substrate_executor::serve::WORKSPACE_ROOT);
    match substrate_executor::serve::serve(&mut input, &mut output, || {
        substrate_executor::serve::open_workspace(root)
    }) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("substrate-executor fs: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Copies this executor to `target`, a path that must not exist yet, as the exec Pod's init
/// container does into its shared volume.
#[cfg(target_os = "linux")]
fn install(target: &str) -> ExitCode {
    use std::io::Write as _;
    use std::os::unix::fs::OpenOptionsExt as _;

    let copy = || -> std::io::Result<()> {
        let bytes = std::fs::read("/proc/self/exe")?;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o555)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(target)?;
        file.write_all(&bytes)?;
        file.sync_all()
    };
    match copy() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("substrate-executor install {target}: {error}");
            ExitCode::FAILURE
        }
    }
}

/// PID 1 of an exec Pod.
#[cfg(target_os = "linux")]
fn run(parts: &[&str]) -> ExitCode {
    use substrate_executor::frame::{Control, Frame, Refusal};

    // A blocked TERM is queued even to a PID 1 without a handler; `forward_term` unblocks it with
    // the handler in place, and `run` forwards one that came before the child's group was known.
    substrate_executor::run::block_term();
    // Kubelet's `KUBERNETES_*` variables and the image's own `PATH` are in this environment; none
    // of it may reach the child, not even through the program lookup.
    for (name, _) in std::env::vars_os() {
        // SAFETY: no other thread exists yet in this process.
        unsafe { std::env::remove_var(name) };
    }
    let mut out = std::io::stdout().lock();
    let Ok(spec) = substrate_executor::spec::RunSpec::decode_parts(parts) else {
        let line = Frame::Control(Control::Refused(Refusal::SpecInvalid))
            .encode()
            .expect("a refusal encodes");
        let _ = std::io::Write::write_all(&mut out, format!("{line}\n").as_bytes());
        return ExitCode::from(2);
    };
    substrate_executor::run::forward_term();
    match substrate_executor::run::run_as_init(
        &spec,
        std::path::Path::new("/sys/fs/cgroup"),
        &mut out,
    ) {
        Ok(code) => ExitCode::from(u8::try_from(code).unwrap_or(1)),
        Err(_) => ExitCode::FAILURE,
    }
}
