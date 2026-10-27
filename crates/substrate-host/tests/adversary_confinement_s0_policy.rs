//! Adversary case for `story:confinement-contract-on-linux-and-local`: the host's `local` policy
//! (`SUBSTRATE_CONFINEMENT_LOCAL`, ADR 0109 F3 A) read at open.
//!
//! The suite's host-policy scenario opens `local` with `LocalPolicy::Refuse`, so nothing reads the
//! variable: `host_permits_local` could return `true` for every value and the suite would stay
//! green. One case in its own test binary, because it changes the process environment.

use substrate_host::confinement::{
    self, AdapterName, AdapterRequest, ConfinementError, ConfinementProfile, Fact,
    local::{LOCAL_POLICY_ENV, LocalRequest},
};

#[test]
fn adv_the_host_policy_permits_local_only_unset_or_exactly_permit() {
    let admit = || {
        let adapter =
            confinement::open(AdapterRequest::Local(LocalRequest::default())).expect("opens");
        let permitted = adapter.facts().proves(&Fact::LocalPermitted);
        let result = adapter.admit(&ConfinementProfile {
            local_reason: Some("adversary".into()),
            ..ConfinementProfile::default()
        });
        (permitted, result)
    };
    // SAFETY: this test binary holds this one test, so no other thread reads the environment.
    unsafe { std::env::remove_var(LOCAL_POLICY_ENV) };
    let (permitted, result) = admit();
    assert!(permitted && result.is_ok(), "unset must permit local");
    // SAFETY: as above.
    unsafe { std::env::set_var(LOCAL_POLICY_ENV, "permit") };
    let (permitted, result) = admit();
    assert!(permitted && result.is_ok(), "`permit` must permit local");
    for value in [
        "Permit", " permit", "permit ", "PERMIT", "", "1", "true", "deny", "permit\n",
    ] {
        // SAFETY: as above.
        unsafe { std::env::set_var(LOCAL_POLICY_ENV, value) };
        let (permitted, result) = admit();
        assert!(!permitted, "{value:?} proved local_permitted");
        assert!(
            matches!(
                result,
                Err(ConfinementError::LocalRefused {
                    adapter: AdapterName::Local
                })
            ),
            "{value:?} admitted local: {result:?}"
        );
    }
    // SAFETY: as above.
    unsafe { std::env::remove_var(LOCAL_POLICY_ENV) };
}
