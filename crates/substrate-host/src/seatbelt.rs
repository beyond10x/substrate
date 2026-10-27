//! Seatbelt confinement of one command on macOS (ADR 0037).
//!
//! A [`SeatbeltPolicy`] names one writable workspace, read-only roots and the executables a
//! command may run. [`SeatbeltPolicy::profile`] renders it as a Sandbox Profile Language (SBPL)
//! profile that starts with `(deny default)`, so everything the profile does not allow by name is
//! refused by the kernel. [`SeatbeltPolicy::command`] builds a [`Command`] that runs a listed
//! executable under that profile through [`SANDBOX_EXEC`], with a cleared environment and its own
//! process group.
//!
//! A build policy (ADR 0109 S2) adds three things a Rust build needs: further writable
//! directories (a private `TMPDIR`), exec by subpath (a toolchain, `target/`, the developer tools,
//! each also readable and mappable as code), and sealed paths inside the workspace that stay
//! unwritable. SBPL applies the last rule that matches, so the sealed denies are rendered after
//! every allow.
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

/// The refusal for an exec subpath outside the workspace, the declared read-only roots and the
/// system and developer directories: exec by subpath would make it readable without a declaration.
pub const EXEC_SUBPATH_UNDECLARED: &str = "exec.sandbox-exec-subpath-undeclared";

/// The refusal for a start whose exec subpaths hold the developer tools when the SDK or the
/// toolchain `clang` could not be resolved outside the sandbox: inside it, cargo would link through
/// the `/usr/bin/cc` shim, which runs `/bin/sh` and `xcodebuild` and fails.
pub const DEVELOPER_TOOLS_UNRESOLVED: &str = "exec.sandbox-developer-tools-unresolved";

/// The link `xcode-select` keeps to the selected developer directory. The `/usr/bin` tool shims
/// read it to find the compiler, so a policy with an exec subpath may read it.
pub const XCODE_SELECT_LINK: &str = "/private/var/db/xcode_select_link";

/// The Command Line Tools root, the developer directory when no Xcode is selected.
pub const COMMAND_LINE_TOOLS: &str = "/Library/Developer/CommandLineTools";

/// Fixed system directories a policy with an exec subpath may read: the system TLS library's
/// configuration and trust store. Cargo links the system `libcurl`, whose `LibreSSL` reads
/// `openssl.cnf` when it starts and exits if the read is refused. They hold no user data and no
/// credential.
pub const BUILD_READ_SUBPATHS: [&str; 1] = ["/private/etc/ssl"];

/// The system-wide Xcode preferences, where `xcodebuild -license` records the accepted licence.
/// `xcrun` and the `cc` shim refuse to run when they cannot see the acceptance, so a policy with an
/// exec subpath may read this one file. It holds no user data and no credential.
pub const XCODE_LICENSE_PREFERENCES: &str = "/Library/Preferences/com.apple.dt.Xcode.plist";

/// System directories an exec subpath may name without a declared root.
pub const SYSTEM_EXEC_SUBPATHS: [&str; 6] = [
    "/bin",
    "/sbin",
    "/usr/bin",
    "/usr/sbin",
    "/usr/libexec",
    COMMAND_LINE_TOOLS,
];

/// The selected developer tools' root, resolved: the `.app` bundle of a selected Xcode (whose
/// `Info.plist` and SDKs `xcrun` reads), else the selected directory itself, else the Command Line
/// Tools when they are installed. `None` on a host without developer tools.
pub fn developer_root() -> Option<PathBuf> {
    if let Ok(selected) = std::fs::canonicalize(XCODE_SELECT_LINK) {
        let bundle = Some(&selected)
            .filter(|selected| selected.ends_with("Contents/Developer"))
            .and_then(|selected| selected.ancestors().nth(2))
            .filter(|bundle| {
                bundle
                    .extension()
                    .is_some_and(|extension| extension == "app")
            })
            .map(Path::to_path_buf);
        return Some(bundle.unwrap_or(selected));
    }
    std::fs::canonicalize(COMMAND_LINE_TOOLS).ok()
}

/// What one confined command may touch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeatbeltPolicy {
    workspace: PathBuf,
    read_only: Vec<PathBuf>,
    executables: Vec<PathBuf>,
    writable: Vec<PathBuf>,
    exec_subpaths: Vec<PathBuf>,
    sealed: Vec<PathBuf>,
}

impl SeatbeltPolicy {
    /// A policy whose only writable location is `workspace`.
    pub fn new(workspace: impl Into<PathBuf>) -> Self {
        Self {
            workspace: workspace.into(),
            read_only: Vec::new(),
            executables: Vec::new(),
            writable: Vec::new(),
            exec_subpaths: Vec::new(),
            sealed: Vec::new(),
        }
    }

    /// Adds a directory beside the workspace the command may read and write.
    #[must_use]
    pub fn writable(mut self, directory: impl Into<PathBuf>) -> Self {
        self.writable.push(directory.into());
        self
    }

    /// Adds a directory beneath which the command may execute, read and map code.
    #[must_use]
    pub fn exec_subpath(mut self, directory: impl Into<PathBuf>) -> Self {
        self.exec_subpaths.push(directory.into());
        self
    }

    /// Adds a path that stays unwritable, with everything beneath it. Each directory between the
    /// workspace and the path is unwritable as itself, so it cannot be renamed or removed; what is
    /// beneath those directories beside the path stays writable.
    #[must_use]
    pub fn sealed(mut self, path: impl Into<PathBuf>) -> Self {
        self.sealed.push(path.into());
        self
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
    /// `exec.sandbox-path-unrepresentable` when a path cannot be resolved. An exec subpath or a
    /// sealed path need not exist yet (a build creates `target/`): its nearest existing ancestor
    /// is resolved and the rest appended.
    pub fn resolved(&self) -> Result<Self, DriverError> {
        let all = |paths: &[PathBuf], resolve: fn(&Path) -> Result<PathBuf, DriverError>| {
            paths
                .iter()
                .map(|path| resolve(path))
                .collect::<Result<Vec<_>, _>>()
        };
        Ok(Self {
            workspace: resolve(&self.workspace)?,
            read_only: all(&self.read_only, resolve)?,
            executables: all(&self.executables, resolve)?,
            writable: all(&self.writable, resolve)?,
            exec_subpaths: all(&self.exec_subpaths, resolve_ancestor)?,
            sealed: all(&self.sealed, resolve_ancestor)?,
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
        let writable = self
            .writable
            .iter()
            .map(|path| literal(path))
            .collect::<Result<Vec<_>, _>>()?;
        let exec_subpaths = self
            .exec_subpaths
            .iter()
            .map(|path| literal(path))
            .collect::<Result<Vec<_>, _>>()?;
        let sealed = self
            .sealed
            .iter()
            .map(|path| literal(path))
            .collect::<Result<Vec<_>, _>>()?;

        let mut profile = String::from("(version 1)\n(deny default)\n");
        profile.push_str("(allow process-fork)\n");
        profile.push_str("(allow signal (target same-sandbox))\n");
        profile.push_str("(allow sysctl-read)\n");
        profile.push_str("(allow file-read-metadata)\n");
        // The dynamic loader reads the root directory itself and aborts the process if it cannot;
        // this grants the listing of `/`, not anything beneath it.
        profile.push_str("(allow file-read-data (literal \"/\"))\n");
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
        for directory in &writable {
            let _ = writeln!(profile, "(allow file-read* (subpath \"{directory}\"))");
            let _ = writeln!(profile, "(allow file-write* (subpath \"{directory}\"))");
        }
        for directory in &exec_subpaths {
            let _ = writeln!(profile, "(allow file-read* (subpath \"{directory}\"))");
            let _ = writeln!(profile, "(allow process-exec (subpath \"{directory}\"))");
            let _ = writeln!(
                profile,
                "(allow file-map-executable (subpath \"{directory}\"))"
            );
        }
        if !exec_subpaths.is_empty() {
            let _ = writeln!(
                profile,
                "(allow file-read* (literal \"{XCODE_SELECT_LINK}\"))"
            );
            for subpath in BUILD_READ_SUBPATHS {
                let _ = writeln!(profile, "(allow file-read* (subpath \"{subpath}\"))");
            }
            let _ = writeln!(
                profile,
                "(allow file-read* (literal \"{XCODE_LICENSE_PREFERENCES}\"))"
            );
        }
        let mut ancestors: Vec<&Path> = Vec::new();
        for path in &self.sealed {
            for ancestor in path.ancestors().skip(1) {
                if ancestor == self.workspace || !ancestor.starts_with(&self.workspace) {
                    break;
                }
                if !ancestors.contains(&ancestor) {
                    ancestors.push(ancestor);
                }
            }
        }
        ancestors.sort();
        for ancestor in ancestors {
            // Every ancestor is a prefix of a sealed path that `literal` accepted.
            let _ = writeln!(
                profile,
                "(deny file-write* (literal \"{}\"))",
                literal(ancestor)?
            );
        }
        for path in &sealed {
            let _ = writeln!(profile, "(deny file-write* (subpath \"{path}\"))");
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

/// Which build-policy rules the platform sandbox was seen to enforce (ADR 0109 S2). A field is
/// `true` only when its probe ran under Seatbelt and both halves held: what the rule allows
/// happened, and what it refuses did not.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BuildProof {
    /// A write beneath a sealed path failed while a write beside it succeeded.
    pub sealed_paths: bool,
    /// A program beneath an exec subpath ran while one outside every subpath did not.
    pub exec_subpaths: bool,
    /// A directory beside the workspace was written through the variable `TMPDIR`.
    pub private_tmpdir: bool,
}

/// Probes the three build-policy rules under the platform sandbox. Any failure, a missing
/// launcher included, leaves that rule unproved; nothing here refuses.
pub fn probe_build() -> BuildProof {
    probe_build_with_launcher(Path::new(SANDBOX_EXEC))
}

fn probe_build_with_launcher(launcher: &Path) -> BuildProof {
    let Ok(scratch) = tempfile::tempdir() else {
        return BuildProof::default();
    };
    let Ok(root) = std::fs::canonicalize(scratch.path()) else {
        return BuildProof::default();
    };
    let workspace = root.join("workspace");
    let private = root.join("tmp");
    if [&workspace, &private, &workspace.join("sealed")]
        .iter()
        .any(|directory| std::fs::create_dir(directory).is_err())
    {
        return BuildProof::default();
    }
    let bash = Path::new("/bin/bash");
    let runs = |policy: &SeatbeltPolicy, script: &str, env: &[(&str, &Path)]| {
        policy
            .command_with_launcher(launcher, bash, ["-c", script, "bash"])
            .ok()
            .and_then(|mut command| {
                command
                    .envs(env.iter().copied())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                    .ok()
            })
            .is_some_and(|status| status.success())
    };
    let probe_policy = || SeatbeltPolicy::new(&workspace).executable(bash);
    let sealed_paths = runs(
        &probe_policy().sealed(workspace.join("sealed")),
        ": > beside || exit 1; if : > sealed/inside; then exit 2; fi 2>/dev/null; exit 0",
        &[],
    ) && workspace.join("beside").is_file()
        && !workspace.join("sealed/inside").exists();
    let exec_subpaths = runs(
        &probe_policy().exec_subpath("/usr/bin"),
        "/usr/bin/true || exit 1; if /bin/echo; then exit 2; fi >/dev/null 2>&1; exit 0",
        &[],
    );
    let private_tmpdir = runs(
        &probe_policy().writable(&private),
        ": > \"$TMPDIR/written\"",
        &[("TMPDIR", private.as_path())],
    ) && private.join("written").is_file();
    BuildProof {
        sealed_paths,
        exec_subpaths,
        private_tmpdir,
    }
}

fn resolve(path: &Path) -> Result<PathBuf, DriverError> {
    std::fs::canonicalize(path).map_err(|error| {
        DriverError::refused(
            PATH_UNREPRESENTABLE,
            format!("cannot resolve {}: {error}", path.display()),
            "seatbelt.path",
        )
    })
}

/// Resolves the nearest existing ancestor of `path` and appends the rest, which must be normal
/// names: `literal` refuses anything else when the profile is rendered.
fn resolve_ancestor(path: &Path) -> Result<PathBuf, DriverError> {
    let mut rest = Vec::new();
    let mut existing = path;
    loop {
        if std::fs::symlink_metadata(existing).is_ok() {
            let mut resolved = resolve(existing)?;
            resolved.extend(rest.iter().rev());
            return Ok(resolved);
        }
        match (existing.parent(), existing.file_name()) {
            (Some(parent), Some(name)) => {
                rest.push(name);
                existing = parent;
            }
            _ => return resolve(path),
        }
    }
}

/// Checks a workspace-relative path a profile names (a sealed path, a relative exec subpath): not
/// empty, not absolute, only normal names, and nothing an SBPL string literal cannot carry.
///
/// # Errors
///
/// `exec.sandbox-path-unrepresentable`.
pub fn relative_literal(text: &str) -> Result<(), DriverError> {
    let refuse = |why: &str| {
        DriverError::refused(
            PATH_UNREPRESENTABLE,
            format!("{text:?} {why}"),
            "seatbelt.path",
        )
    };
    if text.is_empty() || text.starts_with('/') {
        return Err(refuse("is not a relative path"));
    }
    if text
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
    Ok(())
}

/// Checks an absolute path a profile names the way the profile will carry it.
///
/// # Errors
///
/// `exec.sandbox-path-unrepresentable`.
pub fn absolute_literal(text: &str) -> Result<(), DriverError> {
    literal(Path::new(text)).map(|_| ())
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
            "(allow file-read-data (literal \"/\"))",
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

    fn build_policy() -> SeatbeltPolicy {
        policy()
            .writable("/t/run")
            .exec_subpath("/usr/bin")
            .exec_subpath("/w/space/target")
            .sealed("/w/space/deep/sealed")
            .sealed("/w/space/deep/other")
    }

    #[test]
    fn a_build_policy_execs_by_subpath_and_seals_paths_last() {
        let profile = build_policy().profile().expect("profile");
        let lines: Vec<&str> = profile.lines().collect();
        assert_eq!(lines[..2], ["(version 1)", "(deny default)"]);
        for line in [
            "(allow file-read* (subpath \"/t/run\"))",
            "(allow file-write* (subpath \"/t/run\"))",
            "(allow file-read* (subpath \"/usr/bin\"))",
            "(allow process-exec (subpath \"/usr/bin\"))",
            "(allow file-map-executable (subpath \"/usr/bin\"))",
            "(allow process-exec (subpath \"/w/space/target\"))",
            "(allow file-read* (literal \"/private/var/db/xcode_select_link\"))",
            "(allow file-read* (subpath \"/private/etc/ssl\"))",
            "(allow file-read* (literal \"/Library/Preferences/com.apple.dt.Xcode.plist\"))",
        ] {
            assert!(lines.contains(&line), "missing {line}\n{profile}");
        }
        // The last matching rule decides, so every sealed deny follows every allow, and an
        // ancestor between the workspace and a sealed path is denied as itself: renaming it away
        // would move the sealed path out from under its rule.
        assert_eq!(
            lines[lines.len() - 3..],
            [
                "(deny file-write* (literal \"/w/space/deep\"))",
                "(deny file-write* (subpath \"/w/space/deep/sealed\"))",
                "(deny file-write* (subpath \"/w/space/deep/other\"))",
            ],
            "{profile}"
        );
        assert!(!profile.contains("network"), "{profile}");
        assert!(!profile.contains("(allow process-exec)"), "{profile}");
        assert!(
            !profile.contains("(deny file-write* (literal \"/w/space\"))"),
            "the workspace itself is not an ancestor to seal: {profile}"
        );
    }

    #[test]
    fn without_an_exec_subpath_the_developer_tools_stay_unreadable() {
        let profile = policy().writable("/t/run").profile().expect("profile");
        assert!(!profile.contains(XCODE_SELECT_LINK), "{profile}");
        assert!(!profile.contains("file-map-executable"), "{profile}");
        assert!(!profile.contains("/private/etc/ssl"), "{profile}");
        assert!(!profile.contains("com.apple.dt.Xcode"), "{profile}");
    }

    #[test]
    fn unrepresentable_build_paths_are_refused_by_name() {
        for path in ["/w/quo\"te", "relative", "/w/../up", "/w/back\\slash"] {
            for policy in [
                policy().writable(path),
                policy().exec_subpath(path),
                policy().sealed(path),
            ] {
                let error = policy.profile().expect_err(path);
                assert_eq!(error.code, PATH_UNREPRESENTABLE, "{path}");
                assert_eq!(error.class, DriverErrorClass::Refused, "{path}");
            }
        }
    }

    #[test]
    fn a_relative_subpath_is_normal_and_carries_no_unrepresentable_character() {
        for accepted in ["target", ".git/hooks", "a/b/c"] {
            relative_literal(accepted).expect(accepted);
        }
        for refused in [
            "",
            "/abs",
            "a/../b",
            "..",
            "./a",
            "a//b",
            "a/",
            "quo\"te",
            "back\\slash",
            "new\nline",
        ] {
            let error = relative_literal(refused).expect_err(refused);
            assert_eq!(error.code, PATH_UNREPRESENTABLE, "{refused:?}");
        }
    }

    #[test]
    fn a_build_path_not_created_yet_resolves_through_its_existing_ancestor() {
        let scratch = tempfile::tempdir().expect("scratch");
        let real = scratch.path().join("real");
        std::fs::create_dir(&real).expect("real");
        let link = scratch.path().join("link");
        std::os::unix::fs::symlink(&real, &link).expect("link");
        let real = std::fs::canonicalize(&real).expect("canonical");
        let resolved = SeatbeltPolicy::new(&link)
            .sealed(link.join("absent/sealed"))
            .exec_subpath(link.join("target"))
            .writable(&link)
            .resolved()
            .expect("resolved");
        assert_eq!(resolved.workspace, real);
        assert_eq!(resolved.sealed, [real.join("absent/sealed")]);
        assert_eq!(resolved.exec_subpaths, [real.join("target")]);
        assert_eq!(resolved.writable, [real]);
    }

    #[test]
    fn without_the_launcher_no_build_fact_is_proved() {
        let scratch = tempfile::tempdir().expect("scratch");
        assert_eq!(
            probe_build_with_launcher(&scratch.path().join("no-sandbox-exec")),
            BuildProof::default()
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn the_platform_sandbox_proves_every_build_fact() {
        assert_eq!(
            probe_build(),
            BuildProof {
                sealed_paths: true,
                exec_subpaths: true,
                private_tmpdir: true,
            }
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn the_platform_sandbox_enforces_a_profile() {
        probe().expect("sandbox-exec enforces a deny-default profile on this host");
    }
}
