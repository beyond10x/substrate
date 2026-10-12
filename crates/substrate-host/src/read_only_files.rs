//! Declared read-only host files (ADR 0033).
//!
//! Admission is two halves. The lexical half is the wire crate's and needs no host. The
//! filesystem half opens every host path once, refuses a symlink in any component through the
//! kernel rather than a string comparison, checks that the object is a regular file, and keeps the
//! descriptor. Bubblewrap mounts that descriptor (`--ro-bind-fd`), so the file that is mounted is
//! the object admission inspected, whatever is renamed afterwards, and it closes the descriptor
//! before the program starts, so the program never holds a handle on the host file.

use std::ffi::CString;
use std::os::fd::{AsRawFd as _, FromRawFd as _, OwnedFd, RawFd};
use std::os::unix::process::CommandExt as _;
use std::path::Path;
use std::process::Command;

use substrate_wire::{
    MAX_READ_ONLY_FILES, READ_ONLY_FILE_ABSENT, READ_ONLY_FILE_NOT_A_FILE,
    READ_ONLY_FILE_PIN_FAILED, READ_ONLY_FILE_SYMLINK, READ_ONLY_FILES_UNSERVED, ReadOnlyFile,
    ReadOnlyFileFacts, ReadOnlyRoot,
};

use crate::DriverError;

const ADDRESS: &str = "read_only_files";
const RESOLVE_NO_MAGICLINKS: u64 = 0x02;
const RESOLVE_NO_SYMLINKS: u64 = 0x04;

#[repr(C)]
struct OpenHow {
    flags: u64,
    mode: u64,
    resolve: u64,
}

/// A declared file, opened and checked, pinned for the bind.
#[derive(Debug)]
pub(crate) struct PinnedFile {
    descriptor: OwnedFd,
    mount: String,
}

impl PinnedFile {
    pub(crate) fn raw_fd(&self) -> RawFd {
        self.descriptor.as_raw_fd()
    }

    /// The bubblewrap bind: the pinned object, never the declared path.
    ///
    /// A descriptor bind and never `--ro-bind /proc/self/fd/<n>`: bound by path, the descriptor
    /// stays open in the program, which reopens it through `/proc/self/fd/<n>` for writing on the
    /// host's writable mount and rewrites the host file. Bubblewrap mounts the descriptor, checks the
    /// mount is that object, and closes it before the program starts.
    pub(crate) fn bind_args(&self) -> [String; 3] {
        [
            "--ro-bind-fd".to_owned(),
            self.descriptor.as_raw_fd().to_string(),
            self.mount.clone(),
        ]
    }
}

/// The capability fact, published only when the probe proved the mechanism.
pub(crate) fn fact(proven: bool) -> Option<ReadOnlyFileFacts> {
    proven.then_some(ReadOnlyFileFacts {
        max_files: MAX_READ_ONLY_FILES,
    })
}

/// The lexical rules, then the capability: shape is the caller's mistake on any host, and a file
/// asked for where none can be bound is `unserved`, never a run without it.
pub(crate) fn admit(
    files: &[ReadOnlyFile],
    roots: &[ReadOnlyRoot],
    published: Option<&ReadOnlyFileFacts>,
) -> Result<(), DriverError> {
    substrate_wire::validate_read_only_files(files, roots).map_err(|error| {
        let code = substrate_wire::read_only_file_refusal(&error)
            .unwrap_or(substrate_wire::READ_ONLY_FILE_PATH_INVALID);
        DriverError::refused(
            code,
            format!("A declared read-only file is invalid: {error}."),
            ADDRESS,
        )
    })?;
    if !files.is_empty() && published.is_none() {
        return Err(DriverError::unserved(
            READ_ONLY_FILES_UNSERVED,
            "Declared read-only files are not served by this host.",
            ADDRESS,
        ));
    }
    Ok(())
}

/// Opens, checks and pins every declared file, in declaration order.
pub(crate) fn pin(files: &[ReadOnlyFile]) -> Result<Vec<PinnedFile>, DriverError> {
    files
        .iter()
        .map(|file| {
            Ok(PinnedFile {
                descriptor: pin_one(&file.host_path)?,
                mount: file.mount.clone(),
            })
        })
        .collect()
}

fn pin_one(host_path: &str) -> Result<OwnedFd, DriverError> {
    let path = CString::new(host_path).map_err(|_| absent())?;
    let how = OpenHow {
        flags: u64::try_from(libc::O_PATH | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .expect("open flags are non-negative"),
        mode: 0,
        resolve: RESOLVE_NO_SYMLINKS | RESOLVE_NO_MAGICLINKS,
    };
    // SAFETY: the arguments point to an initialized `OpenHow` and a NUL-terminated path.
    let fd = unsafe {
        libc::syscall(
            libc::SYS_openat2,
            libc::AT_FDCWD,
            path.as_ptr(),
            &raw const how,
            size_of::<OpenHow>(),
        )
    };
    if fd < 0 {
        let error = std::io::Error::last_os_error();
        return Err(match error.raw_os_error() {
            // Any symlink component, refused by the kernel during resolution.
            Some(libc::ELOOP) => symlink(),
            Some(libc::ENOSYS) => DriverError::unserved(
                READ_ONLY_FILES_UNSERVED,
                "This kernel cannot resolve a declared file without following symlinks.",
                ADDRESS,
            ),
            // A directory on the way the daemon may not search: the daemon cannot observe the
            // object, so for this host it is not present, said apart in the message (ADR 0033).
            Some(libc::EACCES) => DriverError::refused(
                READ_ONLY_FILE_ABSENT,
                "A declared read-only file is not reachable: a directory on its path is not \
                 searchable by this host.",
                ADDRESS,
            ),
            // ENOENT and ENOTDIR. ENAMETOOLONG is unreachable: the wire crate bounds the path and
            // every component below the kernel's limits.
            _ => absent(),
        });
    }
    // SAFETY: `openat2` returned a new descriptor owned by this scope.
    let opened = unsafe { OwnedFd::from_raw_fd(i32::try_from(fd).expect("descriptor fits i32")) };
    // SAFETY: an all-zero `stat` is a valid out-parameter for `fstat`.
    let mut status: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: a live descriptor and a writable `stat`.
    if unsafe { libc::fstat(opened.as_raw_fd(), &raw mut status) } != 0 {
        return Err(absent());
    }
    match status.st_mode & libc::S_IFMT {
        libc::S_IFREG => {}
        // `O_PATH | O_NOFOLLOW` opens a trailing symlink as itself rather than failing.
        libc::S_IFLNK => return Err(symlink()),
        _ => {
            return Err(DriverError::refused(
                READ_ONLY_FILE_NOT_A_FILE,
                "A declared read-only file is not a regular file.",
                ADDRESS,
            ));
        }
    }
    // Above every descriptor a secret slot may be placed at, so placing one can never replace a
    // pinned file.
    crate::process::above_secret_slots(&opened).ok_or_else(|| {
        DriverError::failed(
            READ_ONLY_FILE_PIN_FAILED,
            "A declared read-only file could not be pinned.",
        )
    })
}

fn absent() -> DriverError {
    DriverError::refused(
        READ_ONLY_FILE_ABSENT,
        "A declared read-only file is not present on this host.",
        ADDRESS,
    )
}

fn symlink() -> DriverError {
    DriverError::refused(
        READ_ONLY_FILE_SYMLINK,
        "A declared read-only file is reached through a symlink.",
        ADDRESS,
    )
}

const PROBE_DIRECTORY: &str = "/opt/substrate-read-only-file-probe";
const PROBE_MOUNT: &str = "/opt/substrate-read-only-file-probe/probe";
const PROBE_TOKEN: &str = "substrate-read-only-file-probe";

/// Proves the whole mechanism in a throwaway sandbox before the fact is published: a regular file
/// pinned and bound exactly as the driver binds it, executed by name from `PATH`, refused a write,
/// alone in its mount directory, and held through no descriptor in the program: none resolves to
/// the bound file, and a write through every descriptor the program holds leaves it unchanged.
pub(crate) fn mechanism_is_provable(bubblewrap: &Path) -> bool {
    let Ok(directory) = tempfile::tempdir() else {
        return false;
    };
    let Ok(host) = std::fs::canonicalize(directory.path()) else {
        return false;
    };
    let program = host.join("probe");
    let contents = format!("#!/bin/sh\necho {PROBE_TOKEN}\n");
    if std::fs::write(&program, &contents).is_err()
        || std::fs::write(host.join("sibling"), "sibling\n").is_err()
    {
        return false;
    }
    {
        use std::os::unix::fs::PermissionsExt as _;
        if std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).is_err() {
            return false;
        }
    }
    let Some(declared) = program.to_str() else {
        return false;
    };
    let Ok(pinned) = pin(&[ReadOnlyFile {
        host_path: declared.to_owned(),
        mount: PROBE_MOUNT.to_owned(),
    }]) else {
        return false;
    };
    let descriptors: Vec<RawFd> = pinned.iter().map(PinnedFile::raw_fd).collect();
    let mut command = Command::new(bubblewrap);
    command
        .env_clear()
        .args(crate::process::USER_NAMESPACE_ARGV)
        .args([
            "--unshare-ipc",
            "--unshare-pid",
            "--unshare-net",
            "--unshare-uts",
            "--new-session",
            "--die-with-parent",
            "--clearenv",
            "--ro-bind",
            "/usr",
            "/usr",
            "--ro-bind-try",
            "/bin",
            "/bin",
            "--ro-bind-try",
            "/lib",
            "/lib",
            "--ro-bind-try",
            "/lib64",
            "/lib64",
            "--proc",
            "/proc",
            "--dev",
            "/dev",
        ])
        .args(pinned.iter().flat_map(PinnedFile::bind_args))
        .args(["--setenv", "PATH"])
        .arg(format!("{PROBE_DIRECTORY}:/usr/bin:/bin"))
        .args(["--", "/bin/sh", "-c"])
        .arg(format!(
            "probe && test ! -e {PROBE_DIRECTORY}/sibling \
             && test \"$(ls -A {PROBE_DIRECTORY})\" = probe \
             && ! (echo written >{PROBE_MOUNT}) 2>/dev/null \
             && {}",
            crate::process::no_descriptor_resolves_to(&[PROBE_MOUNT], "")
        ));
    // SAFETY: `pre_exec` runs after the fork and calls only async-signal-safe `fcntl`; the vector is
    // built before the fork and only read after it.
    unsafe {
        command.pre_exec(move || crate::process::inherit(&descriptors));
    }
    let output = command.output();
    drop(pinned);
    output.is_ok_and(|output| {
        output.status.success() && String::from_utf8_lossy(&output.stdout).trim() == PROBE_TOKEN
    }) && std::fs::read_to_string(&program).is_ok_and(|after| after == contents)
}

#[cfg(test)]
mod tests {
    use std::os::fd::AsRawFd as _;

    use substrate_wire::{
        MAX_READ_ONLY_FILES, MAX_SECRET_SLOT_FD, READ_ONLY_FILE_ABSENT, READ_ONLY_FILE_NOT_A_FILE,
        READ_ONLY_FILE_SYMLINK, READ_ONLY_FILES_UNSERVED, ReadOnlyFile, ReadOnlyRoot,
    };

    use super::{admit, fact, pin};

    fn fixture() -> (tempfile::TempDir, std::path::PathBuf) {
        let directory = tempfile::tempdir().expect("fixture");
        let host = std::fs::canonicalize(directory.path()).expect("canonical fixture");
        std::fs::write(host.join("gate"), "#!/bin/sh\n").expect("program");
        (directory, host)
    }

    fn declared(path: &std::path::Path) -> ReadOnlyFile {
        ReadOnlyFile {
            host_path: path.to_str().expect("UTF-8").to_owned(),
            mount: "/opt/gate".to_owned(),
        }
    }

    fn code(files: &[ReadOnlyFile]) -> &'static str {
        pin(files).expect_err("refused").code
    }

    #[test]
    fn a_regular_file_is_pinned_above_every_slot_descriptor() {
        let (_directory, host) = fixture();
        let pinned = pin(&[declared(&host.join("gate"))]).expect("pinned");
        assert_eq!(pinned.len(), 1);
        assert!(
            pinned[0].raw_fd() > i32::try_from(MAX_SECRET_SLOT_FD).expect("fits"),
            "a slot placement could replace descriptor {}",
            pinned[0].raw_fd()
        );
        // A descriptor bind, never `/proc/self/fd/<n>` as a path: bound by path, the descriptor
        // stays open in the confined program.
        assert_eq!(
            pinned[0].bind_args(),
            [
                "--ro-bind-fd".to_owned(),
                pinned[0].descriptor.as_raw_fd().to_string(),
                "/opt/gate".to_owned(),
            ]
        );
    }

    #[test]
    fn a_directory_is_not_a_file() {
        let (_directory, host) = fixture();
        assert_eq!(code(&[declared(&host)]), READ_ONLY_FILE_NOT_A_FILE);
    }

    #[test]
    fn a_trailing_symlink_is_refused_as_itself() {
        let (_directory, host) = fixture();
        let link = host.join("link");
        std::os::unix::fs::symlink(host.join("gate"), &link).expect("symlink");
        assert_eq!(code(&[declared(&link)]), READ_ONLY_FILE_SYMLINK);
    }

    #[test]
    fn a_symlinked_parent_component_is_refused() {
        let (_directory, host) = fixture();
        let real = host.join("real");
        std::fs::create_dir(&real).expect("directory");
        std::fs::write(real.join("gate"), "#!/bin/sh\n").expect("program");
        let linked = host.join("linked");
        std::os::unix::fs::symlink(&real, &linked).expect("symlink");
        assert_eq!(
            code(&[declared(&linked.join("gate"))]),
            READ_ONLY_FILE_SYMLINK
        );
    }

    #[test]
    fn an_absent_path_is_refused() {
        let (_directory, host) = fixture();
        assert_eq!(
            code(&[declared(&host.join("absent"))]),
            READ_ONLY_FILE_ABSENT
        );
        assert_eq!(
            code(&[declared(&host.join("gate").join("beneath-a-file"))]),
            READ_ONLY_FILE_ABSENT
        );
    }

    #[test]
    fn a_device_is_not_a_file() {
        assert_eq!(
            code(&[declared(std::path::Path::new("/dev/null"))]),
            READ_ONLY_FILE_NOT_A_FILE
        );
    }

    #[test]
    fn shape_outranks_the_capability_and_the_capability_outranks_the_filesystem() {
        let file = ReadOnlyFile {
            host_path: "/absent/gate".to_owned(),
            mount: "/workspace/gate".to_owned(),
        };
        assert_eq!(
            admit(std::slice::from_ref(&file), &[], None)
                .expect_err("refused")
                .code,
            "exec.read-only-file-mount-reserved"
        );
        let file = ReadOnlyFile {
            mount: "/opt/gate".to_owned(),
            ..file
        };
        assert_eq!(
            admit(std::slice::from_ref(&file), &[], None)
                .expect_err("refused")
                .code,
            READ_ONLY_FILES_UNSERVED
        );
        let published = fact(true).expect("published");
        assert_eq!(published.max_files, MAX_READ_ONLY_FILES);
        assert!(admit(std::slice::from_ref(&file), &[], Some(&published)).is_ok());
        assert!(admit(&[], &[], None).is_ok(), "none declared needs no fact");
        assert!(fact(false).is_none());
        let root = ReadOnlyRoot {
            host_path: "/absent".to_owned(),
            mount: "/opt".to_owned(),
        };
        assert_eq!(
            admit(&[file], &[root], Some(&published))
                .expect_err("refused")
                .code,
            "exec.read-only-file-mount-overlap"
        );
    }

    /// Runs the probe's descriptor fragment in a minimal sandbox over `binds`, the pinned
    /// descriptors inherited, and reports whether it passed and whether the host file survived.
    fn fragment_passes(
        bubblewrap: &std::path::Path,
        pinned: &[super::PinnedFile],
        binds: Vec<String>,
        program: &std::path::Path,
    ) -> (bool, bool) {
        use std::os::unix::process::CommandExt as _;
        let descriptors: Vec<std::os::fd::RawFd> =
            pinned.iter().map(super::PinnedFile::raw_fd).collect();
        let mut command = std::process::Command::new(bubblewrap);
        command
            .env_clear()
            .args(crate::process::USER_NAMESPACE_ARGV)
            .args([
                "--unshare-pid",
                "--die-with-parent",
                "--ro-bind",
                "/usr",
                "/usr",
            ])
            .args([
                "--ro-bind-try",
                "/bin",
                "/bin",
                "--ro-bind-try",
                "/lib",
                "/lib",
            ])
            .args([
                "--ro-bind-try",
                "/lib64",
                "/lib64",
                "--proc",
                "/proc",
                "--dev",
                "/dev",
            ])
            .args(binds)
            .args(["--", "/bin/sh", "-c"])
            .arg(crate::process::no_descriptor_resolves_to(
                &["/opt/gate"],
                "",
            ));
        // SAFETY: `pre_exec` runs after the fork and calls only async-signal-safe `fcntl`.
        unsafe {
            command.pre_exec(move || crate::process::inherit(&descriptors));
        }
        let passed = command.status().is_ok_and(|status| status.success());
        let intact = std::fs::read_to_string(program).is_ok_and(|after| after == "#!/bin/sh\n");
        (passed, intact)
    }

    /// The probe's own check, shown to tell the two mechanisms apart: bound by path, the pinned
    /// descriptor stays open in the program, which the fragment refuses (and a write through it
    /// reaches the host file); bound as a descriptor, bubblewrap closes it first and the fragment
    /// passes with the host file untouched.
    #[test]
    fn the_probe_refuses_a_descriptor_left_open_in_the_program() {
        let bubblewrap = std::path::PathBuf::from("/usr/bin/bwrap");
        if !bubblewrap.is_file() {
            return;
        }
        let (_directory, host) = fixture();
        let program = host.join("gate");
        let pinned = pin(&[declared(&program)]).expect("pinned");
        let by_path = vec![
            "--ro-bind".to_owned(),
            format!("/proc/self/fd/{}", pinned[0].raw_fd()),
            "/opt/gate".to_owned(),
        ];
        let (passed, _) = fragment_passes(&bubblewrap, &pinned, by_path, &program);
        assert!(
            !passed,
            "the fragment passed a program that holds the pinned descriptor"
        );
        std::fs::write(&program, "#!/bin/sh\n").expect("restore the fixture");
        let pinned = pin(&[declared(&program)]).expect("pinned");
        let binds = pinned[0].bind_args().to_vec();
        assert_eq!(
            fragment_passes(&bubblewrap, &pinned, binds, &program),
            (true, true),
            "the descriptor bind left a descriptor in the program or the host file changed"
        );
    }
}
