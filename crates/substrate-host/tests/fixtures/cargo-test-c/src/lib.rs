//! The crate `seatbelt_developer_tools_adversary.rs` builds and tests under the macos adapter's
//! build policy: a build script that compiles C, as a crate depending on the `cc` crate does.

unsafe extern "C" {
    fn fixture_c_answer() -> u32;
}

pub fn answer() -> u32 {
    // SAFETY: the C function takes nothing and returns a constant.
    unsafe { fixture_c_answer() }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_c_library_answers() {
        assert_eq!(super::answer(), 42);
    }
}
