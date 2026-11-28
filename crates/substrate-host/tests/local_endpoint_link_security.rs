#![cfg(target_os = "linux")]
//! Security review of the local endpoint (ADR 0054 decision 4): what a relay's connect reaches
//! when the entry its key names is a symbolic link.
//!
//! The relay connects to `/proc/self/fd/<n>/<key>`, where `<n>` is the declared directory held by
//! descriptor (`relay_target` in `src/egress.rs`, whose unit case holds that exact string). The
//! kernel resolves the last component of a Unix socket path with the link followed; `connect` has
//! no flag that refuses a link. So the lookup starts at the held directory, and an entry that is a
//! link leads wherever the link points, resolved in the daemon's mount namespace.
//!
//! What follows from it: the directory is reachable to whoever can create an entry in it, and
//! through a link to every socket the daemon's uid may connect to. ADR 0054 decision 2 admits only
//! a directory the daemon's uid owns that its group and others cannot write, so only that uid
//! (and root) creates entries; this case states the reach those writers have.

use std::fs::OpenOptions;
use std::io::Read as _;
use std::os::fd::AsRawFd as _;
use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
use std::os::unix::net::{UnixListener, UnixStream};

use substrate_host::LocalEndpoint;

/// Decision recorded by this review: a link in the declared directory is followed by the relay's
/// connect, to a socket outside the directory.
#[test]
fn a_link_in_the_declared_directory_leads_the_relay_path_outside_it() {
    let scratch = tempfile::tempdir().expect("scratch");
    let directory = scratch.path().join("agent");
    std::fs::create_dir(&directory).expect("directory");
    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700)).expect("mode");
    // The directory is one a declaration admits.
    LocalEndpoint::open("agent", 7000, &directory).expect("declared");

    // A socket outside the directory, and a link to it inside, named as a key may be.
    let outside = scratch.path().join("elsewhere");
    std::fs::create_dir(&outside).expect("outside");
    let listener = UnixListener::bind(outside.join("socket")).expect("bind outside");
    std::os::unix::fs::symlink(outside.join("socket"), directory.join("worker-1")).expect("link");

    // The directory held as the declaration holds it, and the path the relay builds from it.
    let held = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&directory)
        .expect("held directory");
    let relay_path = format!("/proc/self/fd/{}/worker-1", held.as_raw_fd());

    let mut stream = UnixStream::connect(&relay_path).expect("the relay path follows the link");
    let (mut accepted, _) = listener.accept().expect("accepted outside the directory");
    std::io::Write::write_all(&mut accepted, b"outside").expect("reply");
    drop(accepted);
    let mut reply = Vec::new();
    stream.read_to_end(&mut reply).expect("read");
    assert_eq!(reply, b"outside");
}
