//! The objects `cargo xtask namespace-lane` renders (design 34 § 10.2), as pure functions of
//! their inputs, so `namespace-lane check` can hold them to committed goldens and `observe` creates
//! exactly what the goldens show.
//!
//! Two sides. The **admin** side is the provisioning of § 10.2 step 1: the `restricted` namespace,
//! an unlabelled one, the driver's `ServiceAccount`, the Role of ADR 0047 decision 3 and its binding,
//! the default-deny `NetworkPolicy` and the `ResourceQuota`. The **driver** side is what the lane
//! creates as the driver would, with its credential: probe objects named and labelled as design 34
//! § 4 names and labels them (`sbx-pr-<nonce>-{pvc,keeper,pod}`, the four keys), each namespaced.

use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};

/// `app.kubernetes.io/managed-by`, and the value every driver-side object carries under it.
pub const MANAGED_BY: &str = "app.kubernetes.io/managed-by";
pub const MANAGER: &str = "b10x-substrate";
/// The label naming the deployment by digest.
pub const DEPLOYMENT_LABEL: &str = "b10x-substrate/deployment";
/// The resource label of a probe object: its nonce.
pub const PROBE_LABEL: &str = "b10x-substrate/probe";
/// The label naming the object's role: `pvc`, `keeper` or `pod`.
pub const ROLE_LABEL: &str = "b10x-substrate/role";
/// The annotation binding the object to its operation: for a probe, `sha256:<hex>` of the nonce.
pub const OPERATION_ANNOTATION: &str = "b10x-substrate/operation";
/// The four keys of design 34 § 4 (ADR 0047 decision 4) every driver-side object carries, plus
/// the role label § 4's table adds: `(member, key)`.
pub const OWNERSHIP_KEYS: [(&str, &str); 5] = [
    ("labels", MANAGED_BY),
    ("labels", DEPLOYMENT_LABEL),
    ("labels", PROBE_LABEL),
    ("labels", ROLE_LABEL),
    ("annotations", OPERATION_ANNOTATION),
];

/// The driver's `ServiceAccount`, `Role` and `RoleBinding`.
pub const DRIVER: &str = "substrate-driver";
/// The operator's default-deny policy (ADR 0047 N5).
pub const DEFAULT_DENY: &str = "default-deny";
/// The lane's `ResourceQuota`.
pub const QUOTA: &str = "substrate-lane";
/// The PVCs and Pods the quota admits (§ 10.2 step 1).
pub const QUOTA_CLAIMS: u32 = 4;
pub const QUOTA_PODS: u32 = 8;
/// The uid every restricted lane Pod runs as (design 34 § 3's default `run_as_user`).
pub const RUN_AS_USER: u64 = 65532;
/// kind's default storage class.
pub const STORAGE_CLASS: &str = "standard";
/// Each probe claim's request; kind's provisioner does not enforce it.
pub const CLAIM_SIZE: &str = "64Mi";
/// The container a keeper runs, and the one an exec-like Pod runs.
pub const KEEPER_CONTAINER: &str = "keeper";
pub const RUN_CONTAINER: &str = "run";
/// The memory and CPU every lane container requests and is limited to.
pub const MEMORY: &str = "32Mi";
pub const CPU: &str = "100m";
/// The node-affinity key an exec Pod is pinned to its keeper's node by (ADR 0047 decision 5).
pub const HOSTNAME_KEY: &str = "kubernetes.io/hostname";

/// The kinds the driver side may render: all namespaced.
pub const DRIVER_KINDS: [&str; 2] = ["Pod", "PersistentVolumeClaim"];
/// The only cluster-scoped kind the lane renders, and only on the admin side.
pub const CLUSTER_SCOPED_ADMIN_KINDS: [&str; 1] = ["Namespace"];

/// One rule of the driver's Role: an API group, a resource and its verbs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Grant {
    pub group: String,
    pub resource: String,
    pub verbs: Vec<String>,
}

/// The Role the lane installs, in ADR 0047 decision 3's order: `(group, resource, verbs)`.
/// `namespace-lane check` holds it to the table in the ADR itself.
pub const ROLE_RULES: [(&str, &str, &[&str]); 6] = [
    ("", "pods", &["create", "get", "list", "watch", "delete"]),
    ("", "pods/log", &["get"]),
    ("", "pods/exec", &["create"]),
    (
        "",
        "persistentvolumeclaims",
        &["create", "get", "list", "delete"],
    ),
    ("networking.k8s.io", "networkpolicies", &["get", "list"]),
    (
        "coordination.k8s.io",
        "leases",
        &["create", "get", "update"],
    ),
];

/// The groups of the bare resource names decision 3's table writes without one.
const BARE_GROUPS: [(&str, &str); 5] = [
    ("pods", ""),
    ("pods/log", ""),
    ("pods/exec", ""),
    ("persistentvolumeclaims", ""),
    ("networkpolicies", "networking.k8s.io"),
];

/// [`ROLE_RULES`] as grants.
pub fn role_grants() -> Vec<Grant> {
    ROLE_RULES
        .iter()
        .map(|(group, resource, verbs)| Grant {
            group: (*group).to_owned(),
            resource: (*resource).to_owned(),
            verbs: verbs.iter().map(|verb| (*verb).to_owned()).collect(),
        })
        .collect()
}

/// The table of ADR 0047 decision 3 (`| resource | verbs | for |`), read from the ADR's text:
/// one grant per row, the resource and each verb in backticks. A `name.group` resource names its
/// group; a bare one is looked up in the closed list of the names the table writes bare.
///
/// # Errors
///
/// No decision 3, no table under it, an empty table, or a row this reader cannot place.
pub fn adr_role_table(adr: &str) -> Result<Vec<Grant>, String> {
    let mut lines = adr
        .lines()
        .skip_while(|line| !line.contains("**Minimal RBAC"))
        .skip(1)
        .map(str::trim);
    if !lines.any(|line| line == "| resource | verbs | for |") {
        return Err("ADR 0047 holds no `| resource | verbs | for |` table under decision 3".into());
    }
    if lines.next().is_none_or(|line| !line.starts_with("|---")) {
        return Err("decision 3's table has no separator row".into());
    }
    let mut grants = Vec::new();
    for line in lines.take_while(|line| line.starts_with('|')) {
        let cells: Vec<&str> = line.trim_matches('|').split('|').map(str::trim).collect();
        let [resource, verbs, _] = cells.as_slice() else {
            return Err(format!("decision 3's row {line:?} is not three cells"));
        };
        let resource = resource
            .strip_prefix('`')
            .and_then(|cell| cell.strip_suffix('`'))
            .ok_or_else(|| format!("decision 3's resource {resource:?} is not in backticks"))?;
        let (resource, group) =
            if let Some((bare, group)) = BARE_GROUPS.iter().find(|(bare, _)| *bare == resource) {
                ((*bare).to_owned(), (*group).to_owned())
            } else {
                let (name, group) = resource.split_once('.').ok_or_else(|| {
                    format!("decision 3's resource {resource:?} names no group this lane knows")
                })?;
                (name.to_owned(), group.to_owned())
            };
        let verbs = verbs
            .split(',')
            .map(|verb| {
                verb.trim()
                    .strip_prefix('`')
                    .and_then(|verb| verb.strip_suffix('`'))
                    .map(str::to_owned)
                    .ok_or_else(|| format!("decision 3's verb {verb:?} is not in backticks"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        grants.push(Grant {
            group,
            resource,
            verbs,
        });
    }
    if grants.is_empty() {
        return Err("decision 3's table has no row".into());
    }
    Ok(grants)
}

/// Lowercase RFC 4648 base32 without padding, as design 34 § 4 writes names and labels.
pub fn base32(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";
    let mut text = String::with_capacity(bytes.len().div_ceil(5) * 8);
    let mut buffer: u32 = 0;
    let mut bits = 0;
    for byte in bytes {
        buffer = (buffer << 8) | u32::from(*byte);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            text.push(char::from(ALPHABET[((buffer >> bits) & 31) as usize]));
        }
    }
    if bits > 0 {
        text.push(char::from(ALPHABET[((buffer << (5 - bits)) & 31) as usize]));
    }
    text
}

/// The deployment label: the first 26 characters of the base32 SHA-256 of the deployment.
pub fn deployment_digest(deployment: &str) -> String {
    let mut digest = base32(&Sha256::digest(deployment.as_bytes()));
    digest.truncate(26);
    digest
}

/// A 16-character base32 probe nonce from the low 80 bits of `bits`.
pub fn nonce(bits: u128) -> String {
    base32(&bits.to_be_bytes()[6..])
}

/// The admin side of § 10.2 step 1, in creation order.
pub fn provisioning(namespace: &str, unlabelled: &str) -> Vec<Value> {
    let rules: Vec<Value> = role_grants()
        .iter()
        .map(|grant| {
            json!({
                "apiGroups": [grant.group],
                "resources": [grant.resource],
                "verbs": grant.verbs,
            })
        })
        .collect();
    vec![
        json!({
            "apiVersion": "v1",
            "kind": "Namespace",
            "metadata": {
                "name": namespace,
                "labels": {"pod-security.kubernetes.io/enforce": "restricted"},
            },
        }),
        json!({
            "apiVersion": "v1",
            "kind": "Namespace",
            "metadata": {"name": unlabelled},
        }),
        json!({
            "apiVersion": "v1",
            "kind": "ServiceAccount",
            "metadata": {"name": DRIVER, "namespace": namespace},
            "automountServiceAccountToken": false,
        }),
        json!({
            "apiVersion": "rbac.authorization.k8s.io/v1",
            "kind": "Role",
            "metadata": {"name": DRIVER, "namespace": namespace},
            "rules": rules,
        }),
        json!({
            "apiVersion": "rbac.authorization.k8s.io/v1",
            "kind": "RoleBinding",
            "metadata": {"name": DRIVER, "namespace": namespace},
            "roleRef": {"apiGroup": "rbac.authorization.k8s.io", "kind": "Role", "name": DRIVER},
            "subjects": [{"kind": "ServiceAccount", "name": DRIVER, "namespace": namespace}],
        }),
        json!({
            "apiVersion": "networking.k8s.io/v1",
            "kind": "NetworkPolicy",
            "metadata": {"name": DEFAULT_DENY, "namespace": namespace},
            "spec": {"podSelector": {}, "policyTypes": ["Ingress", "Egress"]},
        }),
        json!({
            "apiVersion": "v1",
            "kind": "ResourceQuota",
            "metadata": {"name": QUOTA, "namespace": namespace},
            "spec": {"hard": {
                "count/persistentvolumeclaims": QUOTA_CLAIMS.to_string(),
                "pods": QUOTA_PODS.to_string(),
            }},
        }),
    ]
}

/// The collection path of an admin-side object of `kind` in `namespace`.
pub fn collection(kind: &str, namespace: &str) -> Option<String> {
    Some(match kind {
        "Namespace" => "/api/v1/namespaces".to_owned(),
        "ServiceAccount" => format!("/api/v1/namespaces/{namespace}/serviceaccounts"),
        "ResourceQuota" => format!("/api/v1/namespaces/{namespace}/resourcequotas"),
        "PersistentVolumeClaim" => format!("/api/v1/namespaces/{namespace}/persistentvolumeclaims"),
        "Pod" => format!("/api/v1/namespaces/{namespace}/pods"),
        "Role" => format!("/apis/rbac.authorization.k8s.io/v1/namespaces/{namespace}/roles"),
        "RoleBinding" => {
            format!("/apis/rbac.authorization.k8s.io/v1/namespaces/{namespace}/rolebindings")
        }
        "NetworkPolicy" => {
            format!("/apis/networking.k8s.io/v1/namespaces/{namespace}/networkpolicies")
        }
        _ => return None,
    })
}

/// What one probe Pod mounts and where it runs.
#[derive(Debug, Clone, Copy)]
pub struct PodShape<'a> {
    /// `keeper` (container `keeper`) or `pod` (container `run`).
    pub role: &'a str,
    /// The `sh -c` script the container runs.
    pub script: &'a str,
    /// The claim mounted at `/workspace` and owner-referenced: `(name, uid)`.
    pub claim: Option<(&'a str, &'a str)>,
    /// The node the Pod is pinned to by required node affinity.
    pub node: Option<&'a str>,
    /// A `privileged` container: only ever dry-run (O2).
    pub privileged: bool,
}

/// One probe's objects: `sbx-pr-<nonce>-{pvc,keeper,pod}` in `namespace`, labelled for
/// `deployment`.
#[derive(Debug, Clone, Copy)]
pub struct Probe<'a> {
    pub namespace: &'a str,
    pub deployment: &'a str,
    pub nonce: &'a str,
    pub image: &'a str,
}

impl Probe<'_> {
    /// The object name of `role`.
    pub fn name(&self, role: &str) -> String {
        format!("sbx-pr-{}-{role}", self.nonce)
    }

    fn metadata(&self, role: &str) -> Value {
        json!({
            "name": self.name(role),
            "namespace": self.namespace,
            "labels": {
                MANAGED_BY: MANAGER,
                DEPLOYMENT_LABEL: deployment_digest(self.deployment),
                PROBE_LABEL: self.nonce,
                ROLE_LABEL: role,
            },
            "annotations": {
                OPERATION_ANNOTATION: format!("sha256:{}", hex::encode(Sha256::digest(self.nonce.as_bytes()))),
            },
        })
    }

    /// The probe's `ReadWriteOnce` claim of [`STORAGE_CLASS`].
    pub fn claim(&self) -> Value {
        json!({
            "apiVersion": "v1",
            "kind": "PersistentVolumeClaim",
            "metadata": self.metadata("pvc"),
            "spec": {
                "accessModes": ["ReadWriteOnce"],
                "storageClassName": STORAGE_CLASS,
                "resources": {"requests": {"storage": CLAIM_SIZE}},
            },
        })
    }

    /// A probe Pod of `shape`: no token, `restartPolicy: Never`, requests equal to limits, and,
    /// unless `privileged`, every field Pod Security `restricted` requires.
    pub fn pod(&self, shape: &PodShape<'_>) -> Value {
        let container = if shape.role == "keeper" {
            KEEPER_CONTAINER
        } else {
            RUN_CONTAINER
        };
        let security = if shape.privileged {
            json!({"privileged": true})
        } else {
            json!({
                "allowPrivilegeEscalation": false,
                "readOnlyRootFilesystem": true,
                "capabilities": {"drop": ["ALL"]},
            })
        };
        let mut metadata = self.metadata(shape.role);
        let mut spec = json!({
            "automountServiceAccountToken": false,
            "restartPolicy": "Never",
            "terminationGracePeriodSeconds": 1,
            "activeDeadlineSeconds": 600,
            "securityContext": {
                "runAsNonRoot": true,
                "runAsUser": RUN_AS_USER,
                "runAsGroup": RUN_AS_USER,
                "seccompProfile": {"type": "RuntimeDefault"},
            },
            "containers": [{
                "name": container,
                "image": self.image,
                "imagePullPolicy": "IfNotPresent",
                "command": ["sh", "-c", shape.script],
                "securityContext": security,
                "resources": {
                    "requests": {"memory": MEMORY, "cpu": CPU},
                    "limits": {"memory": MEMORY, "cpu": CPU},
                },
            }],
        });
        if let Some((claim, uid)) = shape.claim {
            metadata["ownerReferences"] = json!([{
                "apiVersion": "v1",
                "kind": "PersistentVolumeClaim",
                "name": claim,
                "uid": uid,
                "controller": false,
            }]);
            spec["volumes"] =
                json!([{"name": "workspace", "persistentVolumeClaim": {"claimName": claim}}]);
            spec["containers"][0]["volumeMounts"] =
                json!([{"name": "workspace", "mountPath": "/workspace"}]);
        }
        if let Some(node) = shape.node {
            spec["affinity"] = json!({"nodeAffinity": {"requiredDuringSchedulingIgnoredDuringExecution": {
                "nodeSelectorTerms": [{"matchExpressions": [{
                    "key": HOSTNAME_KEY, "operator": "In", "values": [node],
                }]}],
            }}});
        }
        json!({"apiVersion": "v1", "kind": "Pod", "metadata": metadata, "spec": spec})
    }
}

/// The script of the O3 probe: three connections, each bounded at 3 s, one `<name> <status>`
/// line each. `nc` with no input half-closes once connected and ends when the server closes, so
/// status 0 is a connection made and any other status one that failed or timed out.
pub fn egress_script(outside_host: &str, outside_port: u16) -> String {
    format!(
        "while read key value rest; do if [ \"$key\" = nameserver ]; then dns=\"$value\"; break; fi; done </etc/resolv.conf; \
         timeout 3 nc \"$KUBERNETES_SERVICE_HOST\" 443 </dev/null >/dev/null 2>&1; echo \"api $?\"; \
         timeout 3 nslookup kubernetes.default.svc.cluster.local \"$dns\" >/dev/null 2>&1; echo \"dns $?\"; \
         timeout 3 nc {outside_host} {outside_port} </dev/null >/dev/null 2>&1; echo \"outside $?\""
    )
}

/// The script of the O6 Pod: the three cgroup bounds the executor reads back (design 34 § 7.3).
pub const CGROUP_SCRIPT: &str = "for file in pids.max memory.max cpu.max; do printf '%s=' \"$file\"; cat /sys/fs/cgroup/$file; done";

/// The keeper script: a marker on the claim, then wait for deletion.
pub const KEEPER_SCRIPT: &str = "echo keeper > /workspace/keeper && exec sleep 3600";

/// The second Pod of O4: it reads the keeper's marker and writes its own beside it.
pub const SECOND_SCRIPT: &str = "cat /workspace/keeper && echo second > /workspace/second";

/// The inputs `namespace-lane check` renders the goldens from.
pub mod fixed {
    pub const NAMESPACE: &str = "substrate-namespace-lane";
    pub const UNLABELLED: &str = "substrate-namespace-lane-unlabelled";
    pub const DEPLOYMENT: &str = "lane";
    pub const NONCE: &str = "abcdefghijklmnop";
    pub const IMAGE: &str = "docker.io/library/busybox@sha256:bdf57e528e45e4433820e045b29b4597825a1c9e38353532d90a01445013f82e";
    pub const CLAIM_UID: &str = "00000000-0000-4000-8000-000000000001";
    pub const NODE: &str = "substrate-namespace-lane-worker";
    pub const OUTSIDE_HOST: &str = "172.18.0.1";
    pub const OUTSIDE_PORT: u16 = 40000;
}

/// Every object the lane renders from [`fixed`]'s inputs, by golden file stem, and whether it is
/// driver-side.
pub fn goldens() -> Vec<(String, Value, bool)> {
    let mut rendered: Vec<(String, Value, bool)> =
        provisioning(fixed::NAMESPACE, fixed::UNLABELLED)
            .into_iter()
            .map(|object| {
                let stem = format!(
                    "admin-{}-{}",
                    object["kind"].as_str().unwrap_or_default().to_lowercase(),
                    object["metadata"]["name"].as_str().unwrap_or_default()
                );
                (stem, object, false)
            })
            .collect();
    let probe = Probe {
        namespace: fixed::NAMESPACE,
        deployment: fixed::DEPLOYMENT,
        nonce: fixed::NONCE,
        image: fixed::IMAGE,
    };
    let claim = probe.name("pvc");
    let egress = egress_script(fixed::OUTSIDE_HOST, fixed::OUTSIDE_PORT);
    let shapes = [
        (
            "driver-keeper",
            PodShape {
                role: "keeper",
                script: KEEPER_SCRIPT,
                claim: Some((&claim, fixed::CLAIM_UID)),
                node: None,
                privileged: false,
            },
        ),
        (
            "driver-pinned-pod",
            PodShape {
                role: "pod",
                script: SECOND_SCRIPT,
                claim: Some((&claim, fixed::CLAIM_UID)),
                node: Some(fixed::NODE),
                privileged: false,
            },
        ),
        (
            "driver-egress-pod",
            PodShape {
                role: "pod",
                script: &egress,
                claim: None,
                node: None,
                privileged: false,
            },
        ),
        (
            "driver-cgroup-pod",
            PodShape {
                role: "pod",
                script: CGROUP_SCRIPT,
                claim: None,
                node: None,
                privileged: false,
            },
        ),
        (
            "driver-privileged-pod",
            PodShape {
                role: "pod",
                script: "true",
                claim: None,
                node: None,
                privileged: true,
            },
        ),
    ];
    rendered.push(("driver-claim".to_owned(), probe.claim(), true));
    for (stem, shape) in shapes {
        rendered.push((stem.to_owned(), probe.pod(&shape), true));
    }
    rendered
}

/// The deterministic text a golden holds: pretty JSON and one line feed.
pub fn golden_text(object: &Value) -> String {
    let mut text = serde_json::to_string_pretty(object).unwrap_or_default();
    text.push('\n');
    text
}
