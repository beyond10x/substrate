//! Adversary cases for the static chart lane (ADR 0046). Each builds the manifest shape a template
//! or values change would render and asserts the lane refuses it, or that the YAML reader reads it
//! the way the Kubernetes decoder (`sigs.k8s.io/yaml`) does. No case needs `helm`.

use serde_json::{Value, json};

use super::{
    Expect, base_values, check_network_policy, check_services, check_statefulset, merged,
    pod_posture,
};
use crate::chart_yaml;

const ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../deploy/helm/substrate");

fn labels() -> Value {
    json!({"app.kubernetes.io/name": "b10x-substrate", "app.kubernetes.io/instance": "lane", "app.kubernetes.io/component": "daemon"})
}

fn policy(ingress: &Value, egress: &Value) -> Value {
    json!({"kind": "NetworkPolicy", "spec": {
        "podSelector": {"matchLabels": labels()},
        "policyTypes": ["Ingress", "Egress"],
        "ingress": ingress,
        "egress": egress
    }})
}

/// K5: `0.0.0.0/0` is refused unless `networkPolicy.allowAnyEgress`. A prefix of length zero is
/// every address however the address part is spelled; the template's `substrate.anyEgress` and the
/// lane's `check_network_policy` compare two exact strings, and `values.schema.json`'s `cidr`
/// admits every spelling below (`helm template` renders each, exit 0).
#[test]
fn adversary_any_address_egress_in_another_spelling_is_refused() {
    let mut admitted = Vec::new();
    for cidr in ["10.0.0.0/0", "0::/0", "::0/0", "0:0:0:0:0:0:0:0/0"] {
        let egress = json!([{"to": [{"ipBlock": {"cidr": cidr}}], "ports": [{"protocol": "TCP", "port": 443}]}]);
        if check_network_policy(&[policy(&json!([]), &egress)], &labels(), &Expect::lane())
            .is_empty()
        {
            admitted.push(cidr);
        }
    }
    assert!(
        admitted.is_empty(),
        "the lane admits any-address egress without allowAnyEgress: {admitted:?}"
    );
}

/// K5: ingress only from the selectors named in values, egress only to declared CIDR and port. An
/// empty selector is every pod (in every namespace, for `namespaceSelector`), and a port entry
/// without `port` is every port; the lane only checks the lists are non-empty.
#[test]
fn adversary_a_peer_or_port_wider_than_declared_is_refused() {
    let declared_to = json!([{"ipBlock": {"cidr": "192.0.2.10/32"}}]);
    let port = json!([{"protocol": "TCP", "port": 443}]);
    let cases = [
        (
            "egress to every pod in every namespace",
            json!([]),
            json!([{"to": [{"namespaceSelector": {}}], "ports": port}]),
        ),
        (
            "egress to a declared CIDR on every TCP port",
            json!([]),
            json!([{"to": declared_to, "ports": [{"protocol": "TCP"}]}]),
        ),
        (
            "ingress from every pod in every namespace",
            json!([{"from": [{"namespaceSelector": {}}], "ports": [{"protocol": "TCP", "port": 8443}]}]),
            json!([]),
        ),
        (
            "ingress from every pod in the namespace",
            json!([{"from": [{"podSelector": {}}], "ports": [{"protocol": "TCP", "port": 8443}]}]),
            json!([]),
        ),
    ];
    let admitted: Vec<&str> = cases
        .iter()
        .filter(|(_, ingress, egress)| {
            check_network_policy(&[policy(ingress, egress)], &labels(), &Expect::lane()).is_empty()
        })
        .map(|(name, _, _)| *name)
        .collect();
    assert!(admitted.is_empty(), "the lane admits {admitted:?}");
}

/// ADR 0046 § 3: a required `nodeSelector` and toleration *for the dedicated pool*. A selector on
/// a label every node carries and a toleration of every taint pass the lane's non-empty check.
#[test]
fn adversary_the_pod_is_bound_to_the_configured_pool_not_any_node() {
    let statefulset = json!({"kind": "StatefulSet", "metadata": {"name": "lane"}, "spec": {
        "replicas": 2,
        "selector": {"matchLabels": labels()},
        "template": {"metadata": {"labels": labels()}, "spec": {
            "nodeSelector": {"kubernetes.io/os": "linux"},
            "tolerations": [{"operator": "Exists"}],
            "containers": []
        }}
    }});
    let failures = check_statefulset(&statefulset, &[], &Expect::lane());
    assert!(
        failures.iter().any(|failure| failure.contains("pool")),
        "a pod that schedules on any linux node and tolerates every taint is not refused as unbound: {failures:?}"
    );
}

/// K4 and the acceptance ("refuse unsupported privilege ... postures"): `podAnnotations` is an
/// open map, so values can carry the `AppArmor` annotation the API server still copies into the pod
/// field when the field is absent (the execution-off daemon and both init containers set none).
/// `helm template` renders it; neither the schema nor the lane refuses it.
#[test]
fn adversary_values_cannot_unconfine_a_container_through_pod_annotations() {
    let schema: Value = serde_json::from_str(
        &std::fs::read_to_string(format!("{ROOT}/values.schema.json")).unwrap(),
    )
    .unwrap();
    let defaults =
        chart_yaml::documents(&std::fs::read_to_string(format!("{ROOT}/values.yaml")).unwrap())
            .unwrap()
            .remove(0);
    let annotations = json!({
        "container.apparmor.security.beta.kubernetes.io/daemon": "unconfined",
        "container.apparmor.security.beta.kubernetes.io/tls-material": "unconfined"
    });
    let values = merged(
        &merged(&defaults, &base_values()),
        &json!({"podAnnotations": annotations}),
    );
    let schema_admits = jsonschema::validator_for(&schema)
        .unwrap()
        .is_valid(&values);
    let statefulset = json!({"kind": "StatefulSet", "metadata": {"name": "lane"}, "spec": {
        "selector": {"matchLabels": labels()},
        "template": {"metadata": {"labels": labels(), "annotations": annotations}, "spec": {"containers": []}}
    }});
    let lane_flags = check_statefulset(&statefulset, &[], &Expect::lane())
        .iter()
        .any(|failure| failure.contains("apparmor") || failure.contains("AppArmor"));
    assert!(
        !schema_admits || lane_flags,
        "an Unconfined AppArmor annotation passes the values schema and the static lane"
    );
}

/// The reader's own contract: "Plain scalars are typed the way the Kubernetes API reads them".
/// `sigs.k8s.io/yaml` (Helm's `fromYaml` and the client decoder; probed with helm v3.22.1) reads
/// each of these as `true`; the reader reads a string, so `== Value::Bool(true)` checks miss it.
#[test]
fn adversary_yaml_1_1_booleans_read_as_the_api_reads_them_or_are_refused() {
    let mut misread = Vec::new();
    for text in [
        "privileged: True\n",
        "privileged: yes\n",
        "privileged: on\n",
        "privileged: Y\n",
    ] {
        if let Ok(documents) = chart_yaml::documents(text)
            && documents[0]["privileged"] != json!(true)
        {
            misread.push((text, documents[0].clone()));
        }
    }
    assert!(misread.is_empty(), "{misread:?}");
}

/// The reader's contract: anchors and tags are "refused with its line number, never guessed at".
/// On a key, and with a space before the colon, they are neither: the key is read with the extra
/// characters, so the field the lane looks up is absent. `sigs.k8s.io/yaml` reads `privileged`.
#[test]
fn adversary_a_key_spelled_with_space_anchor_or_tag_is_read_as_the_api_reads_it_or_refused() {
    let mut misread = Vec::new();
    for text in [
        "privileged : true\n",
        "&a privileged: true\n",
        "!!str privileged: true\n",
    ] {
        if let Ok(documents) = chart_yaml::documents(text)
            && documents[0].get("privileged") != Some(&json!(true))
        {
            misread.push((text, documents[0].clone()));
        }
    }
    assert!(misread.is_empty(), "{misread:?}");
}

/// The two reader gaps above, end to end: a rendered pod spec that the API server reads as a
/// privileged container on the host network, read by the lane and held to K4 by `pod_posture`.
#[test]
fn adversary_a_privileged_host_network_pod_is_refused_however_it_is_spelled() {
    let text = "\
hostNetwork : true
containers:
  - name: daemon
    securityContext:
      privileged: yes
      runAsNonRoot: true
      runAsUser: 65532
      allowPrivilegeEscalation: false
      capabilities:
        drop:
          - ALL
      seccompProfile:
        type: RuntimeDefault
";
    match chart_yaml::documents(text) {
        Err(_) => {}
        Ok(documents) => {
            let failures = pod_posture(&documents[0], false, "daemon pod");
            assert!(
                failures.iter().any(|f| f.contains("privileged"))
                    && failures.iter().any(|f| f.contains("hostNetwork")),
                "read as {} and passed K4: {failures:?}",
                documents[0]
            );
        }
    }
}

/// K1 A: "No other Service selects the pods." A Service on a label the `StatefulSet` controller adds
/// (`statefulset.kubernetes.io/pod-name`, `apps.kubernetes.io/pod-index`) is not in the template
/// labels, so `selects` never matches it and the lane counts only the headless Service.
#[test]
fn adversary_a_service_selecting_by_a_controller_label_is_refused() {
    let statefulset = json!({"metadata": {"name": "lane"}, "spec": {"serviceName": "lane"}});
    let headless = json!({"kind": "Service", "metadata": {"name": "lane"}, "spec": {"clusterIP": "None", "selector": labels()}});
    let ordinal = json!({"kind": "Service", "metadata": {"name": "lane-0-api"}, "spec": {
        "type": "ClusterIP",
        "selector": {"statefulset.kubernetes.io/pod-name": "lane-0"}
    }});
    let failures = check_services(&[headless, ordinal], &statefulset, &labels());
    assert!(
        failures
            .iter()
            .any(|failure| failure.contains("lane-0-api")),
        "a ClusterIP Service selecting a daemon pod is admitted: {failures:?}"
    );
}
