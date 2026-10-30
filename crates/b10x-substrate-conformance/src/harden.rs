//! Process hardening at start (design 31 § 5, point 1): no core file, and no same-uid `ptrace`
//! or `/proc/<pid>/mem` read of the credential the runner holds for one request.

use nix::sys::resource::{Resource, setrlimit};

/// Sets `PR_SET_DUMPABLE` to 0 and `RLIMIT_CORE` to 0.
pub fn harden() -> nix::Result<()> {
    nix::sys::prctl::set_dumpable(false)?;
    setrlimit(Resource::RLIMIT_CORE, 0, 0)
}

#[cfg(test)]
mod tests {
    #[test]
    fn hardening_leaves_the_process_undumpable_without_a_core_limit() {
        super::harden().expect("hardening");
        assert!(!nix::sys::prctl::get_dumpable().expect("dumpable flag"));
        let (soft, hard) = nix::sys::resource::getrlimit(nix::sys::resource::Resource::RLIMIT_CORE)
            .expect("core limit");
        assert_eq!((soft, hard), (0, 0));
    }
}
