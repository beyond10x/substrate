//! Adversarial cases against ADR 0014's request-side ceiling guard.
//!
//! Added by the adversarial pass. Nothing here is a fix; each case states one promise the change
//! makes and fails where the change does not keep it.

use substrate_wire::{NetworkMode, WireValidationError, validate_aperture_request};

/// A request-supplied aperture name is arbitrary client bytes, and `validate_aperture_request` is
/// the first thing the daemon runs over them (`crates/substrate-daemon/src/app/operations.rs:411`,
/// `crates/substrate-host/src/process.rs:914`). The ceiling guard slices the first four *bytes* of
/// every `/`-separated term (`crates/substrate-wire/src/lib.rs:1941`), which panics whenever byte
/// index 4 lands inside a multi-byte character. Before ADR 0014 the same name was an ordinary
/// `InvalidApertureName` refusal.
#[test]
fn a_non_ascii_aperture_name_is_refused_and_not_a_panic() {
    for name in [
        "ab\u{20ac}cd",
        "abc\u{20ac}",
        "\u{1f600}xy",
        "ma\u{20ac}=1MiB",
    ] {
        let outcome = std::panic::catch_unwind(|| {
            validate_aperture_request(NetworkMode::Aperture, Some(name))
        });
        let result = outcome.unwrap_or_else(|_| {
            panic!("validate_aperture_request panicked on the client-supplied name {name:?}")
        });
        assert_eq!(
            result.expect_err("a name outside [a-z][a-z0-9_]{0,63} is refused"),
            WireValidationError::InvalidApertureName,
            "{name:?}"
        );
    }
}
