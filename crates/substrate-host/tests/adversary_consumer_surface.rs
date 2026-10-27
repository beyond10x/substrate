//! Adversary cases for `story:confinement-consumer-surface`, written against what the unit's own
//! page `website/docs/guides/confinement-profiles.md` tells a consumer.
//!
//! The page's "Choose an adapter by its facts" gives `needed` (the families of
//! `required_families()`, plus `audit` for a sink and `network_egress` for an isolated network on
//! an adapter that proves `host_network`) and `covers` (every family of `needed` served and every
//! fact of `required_facts()` proved), and says that when an adapter does not cover a profile the
//! refusal past the first five rows is the first family of `needed` it does not serve, read from
//! the facts before the call. `consumer_surface.rs` predicts the refusal the same way
//! (`first_unserved`).

use substrate_host::confinement::{
    self, AdapterFacts, AdapterRequest, ConfinementError, ConfinementProfile, ContractFamily,
    FAMILY_ORDER, Fact, Network, ProcessBounds, local::LocalRequest,
};

/// The page's `needed`, verbatim.
fn needed(profile: &ConfinementProfile, facts: &AdapterFacts) -> Vec<ContractFamily> {
    let implied = profile.required_families();
    FAMILY_ORDER
        .into_iter()
        .filter(|family| {
            implied.contains(family)
                || (*family == ContractFamily::Audit && profile.audit.is_some())
                || (*family == ContractFamily::NetworkEgress
                    && profile.network == Some(Network::Isolated)
                    && facts.proves(&Fact::HostNetwork))
        })
        .collect()
}

/// A consumer that wants an isolated network and bounds, offered `local` alone. `covers` is false
/// (`limits` is not served), so by the page the refusal is the first family `needed` names that
/// `local` does not serve.
///
/// `story:confinement-consumer-surface` adversary F3: the case first predicted the refusal from
/// `required_families()` alone and expected `limits`, which is what the page then said. Admission
/// also requires `network_egress` for an isolated network on an adapter that proves
/// `host_network`, so `local` answers `network_egress`. The page was corrected to state that rule
/// (`needed`), and the case now asserts the corrected page: the prediction comes from `needed`,
/// and it is `network_egress`, ahead of `limits` in the contract's order.
#[test]
fn adversary_the_refusal_is_the_first_missing_family_the_public_helpers_name() {
    let adapter = confinement::open(AdapterRequest::Local(LocalRequest::default()))
        .expect("local always opens");
    let facts = adapter.facts();
    let profile = ConfinementProfile {
        local_reason: Some("adversary case runs a host process".into()),
        workspace: Some(std::env::temp_dir()),
        network: Some(Network::Isolated),
        process_bounds: Some(ProcessBounds {
            timeout_ms: 1_000,
            processes: 8,
            memory_bytes: 1 << 26,
            cpu_millis: 1_000,
        }),
        ..ConfinementProfile::default()
    };
    let covers = needed(&profile, facts)
        .iter()
        .all(|family| facts.serves(family))
        && profile
            .required_facts()
            .iter()
            .all(|fact| facts.proves(fact));
    assert!(!covers, "the case needs a profile local does not cover");
    let predicted = needed(&profile, facts)
        .into_iter()
        .find(|family| !facts.serves(family))
        .expect("covers is false, so a family is missing");
    let refused = match adapter.admit(&profile) {
        Err(ConfinementError::FamilyUnserved { family, .. }) => family,
        other => panic!("expected family-unserved, got {other:?}"),
    };
    assert_eq!(
        refused, predicted,
        "the page says the refusal is predicted from the facts before the call"
    );
    assert_eq!(predicted, ContractFamily::NetworkEgress);
    assert!(
        !profile
            .required_families()
            .contains(&ContractFamily::NetworkEgress),
        "the page says required_families() does not name network_egress for an isolated network"
    );
}
