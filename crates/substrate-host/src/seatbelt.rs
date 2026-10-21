//! Seatbelt confinement of one command on macOS (ADR 0037).
//!
//! A [`SeatbeltPolicy`] names one writable workspace, read-only roots and the executables a
//! command may run. [`SeatbeltPolicy::profile`] renders it as a Sandbox Profile Language (SBPL)
//! profile that starts with `(deny default)`, so everything the profile does not allow by name is
//! refused by the kernel. [`SeatbeltPolicy::command`] builds a [`Command`] that runs a listed
//! executable under that profile through [`SANDBOX_EXEC`], with a cleared environment and its own
//! process group.
//!
//! Rendering is plain string work and compiles everywhere; only macOS can apply the profile.
//! A path SBPL cannot carry as a plain string literal is refused rather than escaped, and a
//! missing launcher is the named refusal `exec.sandbox-unavailable`, so no path through this module
//! runs a command unconfined.

use std::ffi::OsStr;
use std::fmt::Write as _;
use std::os::unix::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::DriverError;

/// The launcher that applies a profile and then executes the command inside it.
pub const SANDBOX_EXEC: &str = "/usr/bin/sandbox-exec";

/// The named refusal for a missing or non-enforcing sandbox, shared with the Linux driver.
pub const SANDBOX_UNAVAILABLE: &str = "exec.sandbox-unavailable";

/// The refusal for a path SBPL cannot carry as a plain absolute string literal.
pub const PATH_UNREPRESENTABLE: &str = "exec.sandbox-path-unrepresentable";

/// The refusal for a program the policy does not list as executable.
pub const PROGRAM_UNLISTED: &str = "exec.sandbox-program-unlisted";

/// System locations every macOS process reads while the dynamic loader and libSystem start.
const SYSTEM_READ_SUBPATHS: [&str; 6] = [
    "/usr/lib",
    "/System/Library",
    "/System/Cryptexes",
    "/System/Volumes/Preboot/Cryptexes",
    "/private/var/db/dyld",
    "/Library/Apple/usr/lib",
];

/// Device nodes a confined command may read.
const DEVICE_READ_LITERALS: [&str; 4] = ["/dev/null", "/dev/zero", "/dev/random", "/dev/urandom"];

/// Services libSystem looks up during process start.
const MACH_SERVICES: [&str; 3] = [
    "com.apple.system.logger",
    "com.apple.system.notification_center",
    "com.apple.system.opendirectoryd.libinfo",
];

/// What one confined command may touch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeatbeltPolicy {
    workspace: PathBuf,
    read_only: Vec<PathBuf>,
    executables: Vec<PathBuf>,
}

impl SeatbeltPolicy {
    /// A policy whose only writable location is `workspace`.
    pub fn new(workspace: impl Into<PathBuf>) -> Self {
        Self {
            workspace: workspace.into(),
            read_only: Vec::new(),
            executables: Vec::new(),
        }
    }

    /// Adds a root the command may read and never write.
    #[must_use]
    pub fn read_only(mut self, root: impl Into<PathBuf>) -> Self {
        self.read_only.push(root.into());
        self
    }

    /// Adds an executable the command may run.
    #[must_use]
    pub fn executable(mut self, path: impl Into<PathBuf>) -> Self {
        self.executables.push(path.into());
        self
    }

    /// The same policy with every path resolved through symbolic links.
    ///
    /// Seatbelt matches the resolved path of a file, so `/var/folders/…` must be written as
    /// `/private/var/folders/…` for a rule to apply to it.
    ///
    /// # Errors
    ///
    /// `exec.sandbox-path-unrepresentable` when a path cannot be resolved.
    pub fn resolved(&self) -> Result<Self, DriverError> {
        let resolve = |path: &Path| {
            std::fs::canonicalize(path).map_err(|error| {
                DriverError::refused(
                    PATH_UNREPRESENTABLE,
                    format!("cannot resolve {}: {error}", path.display()),
                    "seatbelt.path",
                )
            })
        };
        Ok(Self {
            workspace: resolve(&self.workspace)?,
            read_only: self
                .read_only
                .iter()
                .map(|path| resolve(path))
                .collect::<Result<_, _>>()?,
            executables: self
                .executables
                .iter()
                .map(|path| resolve(path))
                .collect::<Result<_, _>>()?,
        })
    }

    /// Renders the policy as an SBPL profile.
    ///
    /// # Errors
    ///
    /// `exec.sandbox-path-unrepresentable` for a path that is not absolute and normal, is not
    /// UTF-8, or holds a quote, a backslash or a control character.
    pub fn profile(&self) -> Result<String, DriverError> {
        let workspace = literal(&self.workspace)?;
        let read_only = self
            .read_only
            .iter()
            .map(|path| literal(path))
            .collect::<Result<Vec<_>, _>>()?;
        let executables = self
            .executables
            .iter()
            .map(|path| literal(path))
            .collect::<Result<Vec<_>, _>>()?;

        let mut profile = String::from("(version 1)\n(deny default)\n");
        profile.push_str("(allow process-fork)\n");
        profile.push_str("(allow signal (target same-sandbox))\n");
        profile.push_str("(allow sysctl-read)\n");
        profile.push_str("(allow file-read-metadata)\n");
        for service in MACH_SERVICES {
            let _ = writeln!(profile, "(allow mach-lookup (global-name \"{service}\"))");
        }
        for subpath in SYSTEM_READ_SUBPATHS {
            let _ = writeln!(profile, "(allow file-read* (subpath \"{subpath}\"))");
        }
        for device in DEVICE_READ_LITERALS {
            let _ = writeln!(profile, "(allow file-read* (literal \"{device}\"))");
        }
        profile.push_str("(allow file-write-data (literal \"/dev/null\"))\n");
        let _ = writeln!(profile, "(allow file-read* (subpath \"{workspace}\"))");
        let _ = writeln!(profile, "(allow file-write* (subpath \"{workspace}\"))");
        for root in &read_only {
            let _ = writeln!(profile, "(allow file-read* (subpath \"{root}\"))");
        }
        for executable in &executables {
            let _ = writeln!(profile, "(allow file-read* (literal \"{executable}\"))");
            let _ = writeln!(profile, "(allow process-exec (literal \"{executable}\"))");
        }
        Ok(profile)
    }

    /// A command that runs `program` with `args` under this policy.
    ///
    /// The environment is cleared, the working directory is the workspace, standard input is
    /// closed, and the command leads its own process group. The caller spawns it.
    ///
    /// # Errors
    ///
    /// `exec.sandbox-unavailable` when [`SANDBOX_EXEC`] is missing,
    /// `exec.sandbox-program-unlisted` when `program` is not one of the policy's executables, and
    /// the refusals of [`Self::resolved`] and [`Self::profile`].
    pub fn command<I, S>(&self, program: &Path, args: I) -> Result<Command, DriverError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.command_with_launcher(Path::new(SANDBOX_EXEC), program, args)
    }

    fn command_with_launcher<I, S>(
        &self,
        launcher: &Path,
        program: &Path,
        args: I,
    ) -> Result<Command, DriverError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        if !launcher.is_file() {
            return Err(DriverError::unserved(
                SANDBOX_UNAVAILABLE,
                format!("the Seatbelt launcher {} is missing", launcher.display()),
                "seatbelt.launcher",
            ));
        }
        let resolved = self.resolved()?;
        let program = std::fs::canonicalize(program).map_err(|error| {
            DriverError::refused(
                PATH_UNREPRESENTABLE,
                format!("cannot resolve {}: {error}", program.display()),
                "seatbelt.program",
            )
        })?;
        if !resolved.executables.contains(&program) {
            return Err(DriverError::refused(
                PROGRAM_UNLISTED,
                format!("{} is not an executable of this policy", program.display()),
                "seatbelt.program",
            ));
        }
        let profile = resolved.profile()?;
        let mut command = Command::new(launcher);
        command
            .arg("-p")
            .arg(profile)
            .arg(&program)
            .args(args)
            .env_clear()
            .current_dir(&resolved.workspace)
            .stdin(Stdio::null())
            .process_group(0);
        Ok(command)
    }
}

/// Checks that the launcher exists and enforces a profile: a command the profile allows runs, and
/// the same command under a profile that allows no exec does not.
///
/// # Errors
///
/// `exec.sandbox-unavailable` when either half fails.
pub fn probe() -> Result<(), DriverError> {
    probe_with_launcher(Path::new(SANDBOX_EXEC))
}

fn probe_with_launcher(launcher: &Path) -> Result<(), DriverError> {
    let unavailable =
        |message: String| DriverError::unserved(SANDBOX_UNAVAILABLE, message, "seatbelt.probe");
    let scratch =
        tempfile::tempdir().map_err(|error| unavailable(format!("probe directory: {error}")))?;
    let program = Path::new("/usr/bin/true");
    let allowed = SeatbeltPolicy::new(scratch.path()).executable(program);
    let status = allowed
        .command_with_launcher(launcher, program, std::iter::empty::<&str>())?
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|error| unavailable(format!("probe run: {error}")))?;
    if !status.success() {
        return Err(unavailable(format!(
            "an allowed command failed under Seatbelt: {status}"
        )));
    }
    let denied = SeatbeltPolicy::new(scratch.path()).profile()?;
    let status = Command::new(launcher)
        .arg("-p")
        .arg(denied)
        .arg(program)
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|error| unavailable(format!("probe run: {error}")))?;
    if status.success() {
        return Err(unavailable(
            "a command the profile does not allow ran: the sandbox is not enforcing".to_owned(),
        ));
    }
    Ok(())
}

fn literal(path: &Path) -> Result<&str, DriverError> {
    let refuse = |why: &str| {
        DriverError::refused(
            PATH_UNREPRESENTABLE,
            format!("{} {why}", path.display()),
            "seatbelt.path",
        )
    };
    let text = path.to_str().ok_or_else(|| refuse("is not UTF-8"))?;
    if !path.is_absolute() {
        return Err(refuse("is not absolute"));
    }
    // `Path::components` drops interior `.` and repeated separators, so check the text itself.
    if text != "/"
        && text[1..]
            .split('/')
            .any(|segment| matches!(segment, "" | "." | ".."))
    {
        return Err(refuse("is not normal"));
    }
    if text
        .chars()
        .any(|character| character == '"' || character == '\\' || character.is_control())
    {
        return Err(refuse(
            "holds a character an SBPL string literal cannot carry",
        ));
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DriverErrorClass;

    fn policy() -> SeatbeltPolicy {
        SeatbeltPolicy::new("/w/space")
            .read_only("/r/one")
            .executable("/bin/bash")
    }

    #[test]
    fn profile_denies_by_default_and_names_every_allowance() {
        let profile = policy().profile().expect("profile");
        let lines: Vec<&str> = profile.lines().collect();
        assert_eq!(lines[..2], ["(version 1)", "(deny default)"]);
        for line in [
            "(allow file-read* (subpath \"/w/space\"))",
            "(allow file-write* (subpath \"/w/space\"))",
            "(allow file-read* (subpath \"/r/one\"))",
            "(allow process-exec (literal \"/bin/bash\"))",
            "(allow signal (target same-sandbox))",
        ] {
            assert!(lines.contains(&line), "missing {line}");
        }
    }

    #[test]
    fn profile_grants_no_network_and_writes_only_the_workspace() {
        let profile = policy().profile().expect("profile");
        assert!(!profile.contains("network"), "{profile}");
        let writes: Vec<&str> = profile
            .lines()
            .filter(|line| line.contains("file-write"))
            .collect();
        assert_eq!(
            writes,
            [
                "(allow file-write-data (literal \"/dev/null\"))",
                "(allow file-write* (subpath \"/w/space\"))",
            ]
        );
        assert!(!profile.contains("(allow process-exec)"), "{profile}");
        assert!(!profile.contains("(allow signal)"), "{profile}");
    }

    #[test]
    fn unrepresentable_paths_are_refused_by_name() {
        for path in [
            "relative/dir",
            "/w/../escape",
            "/w/./dot",
            "/w/quo\"te",
            "/w/back\\slash",
            "/w/new\nline",
            "/w//double",
            "/w/trailing/",
        ] {
            let error = SeatbeltPolicy::new(path).profile().expect_err(path);
            assert_eq!(error.code, PATH_UNREPRESENTABLE, "{path}");
            assert_eq!(error.class, DriverErrorClass::Refused, "{path}");
            let error = SeatbeltPolicy::new("/w")
                .read_only(path)
                .profile()
                .expect_err(path);
            assert_eq!(error.code, PATH_UNREPRESENTABLE, "{path}");
            let error = SeatbeltPolicy::new("/w")
                .executable(path)
                .profile()
                .expect_err(path);
            assert_eq!(error.code, PATH_UNREPRESENTABLE, "{path}");
        }
    }

    #[test]
    fn a_missing_launcher_refuses_before_resolving_anything() {
        let scratch = tempfile::tempdir().expect("scratch");
        let launcher = scratch.path().join("no-sandbox-exec");
        let error = SeatbeltPolicy::new(scratch.path().join("absent"))
            .command_with_launcher(&launcher, Path::new("/bin/sh"), ["-c", "true"])
            .expect_err("a missing launcher built a command");
        assert_eq!(error.code, SANDBOX_UNAVAILABLE);
        assert_eq!(error.class, DriverErrorClass::Unserved);
        let error = probe_with_launcher(&launcher).expect_err("the probe passed");
        assert_eq!(error.code, SANDBOX_UNAVAILABLE);
    }

    #[test]
    fn an_unlisted_program_is_refused() {
        let scratch = tempfile::tempdir().expect("scratch");
        let launcher = scratch.path().join("launcher");
        std::fs::write(&launcher, b"").expect("launcher stand-in");
        let error = SeatbeltPolicy::new(scratch.path())
            .executable("/bin/sh")
            .command_with_launcher(&launcher, Path::new("/bin/cat"), ["/etc/passwd"])
            .expect_err("an unlisted program built a command");
        assert_eq!(error.code, PROGRAM_UNLISTED);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn the_platform_sandbox_enforces_a_profile() {
        probe().expect("sandbox-exec enforces a deny-default profile on this host");
    }
}
