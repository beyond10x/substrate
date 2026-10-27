//! The crate `seatbelt_adversary.rs` builds and tests under the macos adapter's build policy: a
//! vendored dependency, a build script, a private `TMPDIR` and no network.

include!(concat!(env!("OUT_DIR"), "/generated.rs"));

pub fn answer() -> u32 {
    seatbelt_fixture_dep::answer() + 1
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_vendored_dependency_answers() {
        assert_eq!(super::answer(), 42);
    }

    #[test]
    fn the_build_script_ran() {
        assert_eq!(super::FROM_BUILD_SCRIPT, "generated");
    }

    #[test]
    fn the_temporary_directory_is_writable() {
        let directory = std::env::var_os("TMPDIR").expect("the run sets TMPDIR");
        let file = std::path::Path::new(&directory).join("fixture-test");
        std::fs::write(&file, b"written").expect("TMPDIR is writable");
        assert_eq!(std::fs::read(&file).expect("read back"), b"written");
    }

    /// `FIXTURE_PORT` names a listener the caller holds; a connection to it must fail.
    #[test]
    fn the_network_is_unreachable() {
        let port: u16 = std::env::var("FIXTURE_PORT")
            .expect("the caller names a listening port")
            .parse()
            .expect("a port number");
        let connected = std::net::TcpStream::connect(("127.0.0.1", port));
        assert!(connected.is_err(), "connected to 127.0.0.1:{port}");
    }
}
