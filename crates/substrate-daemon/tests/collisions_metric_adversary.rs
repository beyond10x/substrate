//! Adversary, `story:kubernetes-collisions-metric`: the deployment guide is what an operator reads
//! to know the operational listener's metric set and its sample cap. The catalogue test in
//! `src/app/health.rs` says a change to the catalogue "changes this test, the deployment guide and
//! the changelog together". A Kubernetes daemon now serves one more family and 583 samples.

use std::path::Path;

fn guide() -> String {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../website/docs/guides/deployment.md");
    std::fs::read_to_string(&path).expect("the deployment guide")
}

/// The guide's metric list names the series a Kubernetes daemon serves.
#[test]
fn the_deployment_guide_names_the_kubernetes_collision_series() {
    let guide = guide();
    assert!(
        guide.contains("substrate_kubernetes_collisions_total"),
        "story:kubernetes-collisions-metric: the deployment guide's metric set omits the series a \
         `--driver kubernetes` daemon serves"
    );
}

/// The sample cap the guide states holds for a Kubernetes daemon's scrape (583 samples).
#[test]
fn the_deployment_guide_states_a_sample_cap_a_kubernetes_scrape_keeps() {
    let guide = guide();
    let caps = guide
        .split("at most ")
        .skip(1)
        .filter_map(|rest| {
            let (number, tail) = rest.split_once(' ')?;
            tail.starts_with("samples")
                .then(|| number.parse::<usize>().ok())
                .flatten()
        })
        .collect::<Vec<_>>();
    assert!(!caps.is_empty(), "the guide states a sample cap");
    assert!(
        caps.iter().any(|cap| *cap >= 583),
        "story:kubernetes-collisions-metric: the guide caps a scrape at {caps:?} samples, and a \
         Kubernetes daemon's scrape carries 583"
    );
}
