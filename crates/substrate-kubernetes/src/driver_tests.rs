//! The driver core on the fake API server (design 34 § 12 U6): each startup refusal, the Lease
//! rules on an injected monotonic clock, the snapshot binding and the ownership rule. Every case
//! runs once, in seconds: time moves only when a case advances the clock.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use substrate_host::{
    BackendKind, BackendLedger, BackendName, BackendRecord, BackendRole, DriverError, UidBinding,
};
use substrate_kubernetes_client::fake::{FakeCluster, Fault};
use substrate_kubernetes_client::{Api, Method, kubeconfig};

use crate::lease::Clock;
use crate::names::{self, DEPLOYMENT_LABEL, MANAGED_BY, MANAGED_BY_LABEL, OPERATION_ANNOTATION};
use crate::rules::{DEFAULT_RESOURCE_RULES, ROLE_RULES};
use crate::{
    CAPABILITY_STALE, CONFIG_INVALID, EXTENSION, LEASE_FOREIGN, LEASE_HELD, LEASE_NAME,
    NamespaceDriver, POD_SECURITY_UNENFORCED, POLICY_NOT_DEFAULT_DENY, RBAC_EXCESS, Removal,
};

pub(crate) const DEPLOYMENT: &str = "deploy-a";
const TOKEN: &str = "fake-token-never-quoted";
const POLICY: &str = "default-deny";
const DIGEST: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

/// A monotonic clock that moves only when a case advances it.
pub(crate) struct ManualClock {
    base: Instant,
    /// The wall time at `base`: taken before any row a case reserves, so a row's `created_at` is
    /// never after the clock's start.
    wall_base: DateTime<Utc>,
    offset: Mutex<Duration>,
}

impl ManualClock {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            base: Instant::now(),
            wall_base: Utc::now(),
            offset: Mutex::new(Duration::ZERO),
        })
    }

    pub(crate) fn set(&self, at: Duration) {
        *self.offset.lock().unwrap_or_else(PoisonError::into_inner) = at;
    }
}

impl Clock for ManualClock {
    fn now(&self) -> Instant {
        self.base + *self.offset.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn wall(&self) -> DateTime<Utc> {
        let offset = *self.offset.lock().unwrap_or_else(PoisonError::into_inner);
        self.wall_base + chrono::Duration::from_std(offset).expect("a case's offset is small")
    }
}

/// The daemon's ledger, in memory: one row per reserved name.
#[derive(Default)]
pub(crate) struct MemoryLedger {
    rows: Mutex<BTreeMap<String, BackendRecord>>,
    binds: Mutex<Vec<(String, String)>>,
}

impl MemoryLedger {
    pub(crate) fn reserve(
        &self,
        kind: BackendKind,
        id: &str,
        role: BackendRole,
        name: &str,
        digest: &str,
    ) {
        self.rows
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(
                name.to_owned(),
                BackendRecord {
                    kind,
                    resource_id: id.to_owned(),
                    role,
                    name: name.to_owned(),
                    operation_digest: digest.to_owned(),
                    uid: None,
                    created_at: Utc::now(),
                    settled_at: None,
                },
            );
    }

    pub(crate) fn row(&self, name: &str) -> BackendRecord {
        self.rows
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(name)
            .cloned()
            .expect("the row is reserved")
    }
}

impl BackendLedger for MemoryLedger {
    fn recorded(&self, kind: BackendKind, id: &str) -> Result<Vec<BackendRecord>, DriverError> {
        Ok(self
            .rows
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .values()
            .filter(|row| row.kind == kind && row.resource_id == id)
            .cloned()
            .collect())
    }

    fn bind_uid(&self, name: &str, uid: &str) -> Result<UidBinding, DriverError> {
        self.binds
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((name.to_owned(), uid.to_owned()));
        let mut rows = self.rows.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(row) = rows.get_mut(name) else {
            return Ok(UidBinding::Unrecorded);
        };
        if row.settled_at.is_some() {
            return Ok(UidBinding::Settled);
        }
        match &row.uid {
            Some(bound) if bound != uid => Ok(UidBinding::Collision {
                bound: bound.clone(),
            }),
            _ => {
                row.uid = Some(uid.to_owned());
                Ok(UidBinding::Bound)
            }
        }
    }

    fn settle(&self, name: &str) -> Result<bool, DriverError> {
        let mut rows = self.rows.lock().unwrap_or_else(PoisonError::into_inner);
        Ok(rows.get_mut(name).is_some_and(|row| {
            row.settled_at = Some(Utc::now());
            true
        }))
    }

    /// As the store reserves them: one row per name, keyed by the nonce, whose operation digest
    /// is the nonce's.
    fn reserve_probe(&self, nonce: &str, names: &[BackendName]) -> Result<(), DriverError> {
        let digest = crate::policy::hex(&<sha2::Sha256 as sha2::Digest>::digest(nonce.as_bytes()));
        for name in names {
            self.reserve(BackendKind::Probe, nonce, name.role, &name.name, &digest);
        }
        Ok(())
    }

    fn unsettled(&self, after: Option<&str>) -> Result<Vec<BackendRecord>, DriverError> {
        Ok(self
            .rows
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .values()
            .filter(|row| row.settled_at.is_none() && after.is_none_or(|after| *row.name > *after))
            .cloned()
            .collect())
    }
}

impl MemoryLedger {
    /// Every row of `kind`, settled or not.
    pub(crate) fn rows_of(&self, kind: BackendKind) -> Vec<BackendRecord> {
        self.rows
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .values()
            .filter(|row| row.kind == kind)
            .cloned()
            .collect()
    }
}

/// Every required member of the extension, valid.
fn extension() -> Value {
    json!({
        "storage_class": "standard",
        "workspace_size_bytes": 1_073_741_824_u64,
        "executor_image": format!("registry.invalid/substrate/executor@sha256:{DIGEST}"),
        "exec_image": format!("registry.invalid:5000/busybox@sha256:{DIGEST}"),
        "default_deny_policy": POLICY,
        "egress_probe_address": "192.0.2.10:443",
        "pod_pids_limit": 1024,
        "container_log_max_bytes": 10_485_760,
    })
}

/// The fake's kubeconfig with `extension` as the namespace driver's context extension.
fn kubeconfig_with(fake: &FakeCluster, extension: Option<Value>) -> Vec<u8> {
    let mut config = fake.kubeconfig();
    if let Some(extension) = extension {
        config["contexts"][0]["context"]["extensions"] =
            json!([{"name": EXTENSION, "extension": extension}]);
    }
    serde_json::to_vec(&config).expect("the kubeconfig serializes")
}

/// A client of the fake with no driver settings: what an operator, or another daemon, does.
pub(crate) fn admin(fake: &FakeCluster) -> Api {
    let bytes = kubeconfig_with(fake, None);
    kubeconfig::parse(&bytes, |_, _| Ok(()))
        .expect("the fake's kubeconfig loads")
        .0
}

/// The Role of ADR 0047 decision 3 and the default rules every authenticated subject holds, as
/// a rules review states them.
fn exact_rules() -> Vec<Value> {
    let mut rules: Vec<Value> = ROLE_RULES
        .iter()
        .chain(DEFAULT_RESOURCE_RULES.iter())
        .map(|(group, resource, verbs)| {
            json!({"verbs": verbs, "apiGroups": [group], "resources": [resource]})
        })
        .collect();
    rules.push(json!({"verbs": ["get"], "apiGroups": [""], "resources": ["pods"], "resourceNames": ["one"]}));
    rules
}

fn default_deny() -> Value {
    json!({
        "metadata": {"name": POLICY},
        "spec": {"podSelector": {}, "policyTypes": ["Ingress", "Egress"]},
    })
}

/// A fake that every startup step passes: the exact rules, the default-deny policy and
/// `restricted` enforced.
pub(crate) fn passing_fake() -> FakeCluster {
    let fake = FakeCluster::start("substrate", TOKEN).expect("the fake starts");
    fake.set_rules(exact_rules());
    fake.seed("networkpolicies", default_deny())
        .expect("the policy is stored");
    fake.enforce_restricted(true);
    fake
}

pub(crate) fn driver(
    fake: &FakeCluster,
    clock: &Arc<ManualClock>,
) -> (NamespaceDriver, Arc<MemoryLedger>) {
    driver_as(fake, clock, DEPLOYMENT)
}

fn driver_as(
    fake: &FakeCluster,
    clock: &Arc<ManualClock>,
    deployment: &str,
) -> (NamespaceDriver, Arc<MemoryLedger>) {
    let ledger = Arc::new(MemoryLedger::default());
    let driver = driver_over(fake, clock, &ledger, deployment);
    (driver, ledger)
}

/// A driver over `ledger`: what a new process opens over the state an earlier one left.
pub(crate) fn driver_over(
    fake: &FakeCluster,
    clock: &Arc<ManualClock>,
    ledger: &Arc<MemoryLedger>,
    deployment: &str,
) -> NamespaceDriver {
    let bytes = kubeconfig_with(fake, Some(extension()));
    NamespaceDriver::parse_with_clock(
        &bytes,
        deployment,
        Arc::clone(ledger) as Arc<dyn BackendLedger>,
        Arc::clone(clock) as Arc<dyn Clock>,
    )
    .expect("the configuration loads")
}

/// A driver over any `ledger`, as the daemon's store-backed one stands in for it: a ledger whose
/// store write fails is not a [`MemoryLedger`] (adversary, D337 S6a).
pub(crate) fn driver_over_any(
    fake: &FakeCluster,
    clock: &Arc<ManualClock>,
    ledger: Arc<dyn BackendLedger>,
) -> NamespaceDriver {
    let bytes = kubeconfig_with(fake, Some(extension()));
    NamespaceDriver::parse_with_clock(
        &bytes,
        DEPLOYMENT,
        ledger,
        Arc::clone(clock) as Arc<dyn Clock>,
    )
    .expect("the configuration loads")
}

fn lease(fake: &FakeCluster) -> Value {
    fake.object("leases", LEASE_NAME).expect("the Lease exists")
}

fn seed_lease(fake: &FakeCluster, holder: &str) {
    fake.seed(
        "leases",
        json!({
            "metadata": {"name": LEASE_NAME},
            "spec": {"holderIdentity": holder, "leaseDurationSeconds": 30},
        }),
    )
    .expect("the Lease is stored");
}

/// Another holder renews the Lease: its `resourceVersion` moves.
async fn renew_as_other(fake: &FakeCluster) {
    let api = admin(fake);
    let mut held = lease(fake);
    held["spec"]["renewTime"] = json!("2026-10-29T10:00:00.000000Z");
    api.update(&format!("{}/{LEASE_NAME}", api.leases()), &held)
        .await
        .expect("the other holder renews");
}

/// Extensions the parser refuses: the case, the extension (`None`: absent), and the member the
/// refusal must name.
fn refused_extensions() -> Vec<(&'static str, Option<Value>, &'static str)> {
    let mut cases: Vec<(&str, Option<Value>, &str)> = Vec::new();
    let with = |member: &str, value: Value| {
        let mut extension = extension();
        extension[member] = value;
        Some(extension)
    };
    let without = |member: &str| {
        let mut extension = extension();
        extension.as_object_mut().expect("an object").remove(member);
        Some(extension)
    };
    cases.push(("no extension", None, EXTENSION));
    cases.push(("unknown member", with("node_name", json!("n")), "node_name"));
    cases.push((
        "missing storage class",
        without("storage_class"),
        "storage_class",
    ));
    cases.push((
        "tagged image",
        with("exec_image", json!("registry.invalid/busybox:1.37")),
        "exec_image",
    ));
    cases.push((
        "tag beside the digest",
        with(
            "executor_image",
            json!(format!("registry.invalid/executor:1.0@sha256:{DIGEST}")),
        ),
        "executor_image",
    ));
    cases.push((
        "size under 1 MiB",
        with("workspace_size_bytes", json!(1_048_575)),
        "workspace_size_bytes",
    ));
    cases.push((
        "probe interval under 60 s",
        with("probe_interval_seconds", json!(59)),
        "probe_interval_seconds",
    ));
    cases.push(("root uid", with("run_as_user", json!(0)), "run_as_user"));
    cases.push((
        "loopback probe address",
        with("egress_probe_address", json!("127.0.0.1:443")),
        "egress_probe_address",
    ));
    cases.push((
        "policy name not a DNS name",
        with("default_deny_policy", json!("Default/Deny")),
        "default_deny_policy",
    ));
    cases
}

/// Step 1 alone, for `deployment`.
fn load(bytes: &[u8], deployment: &str) -> Result<NamespaceDriver, crate::StartupRefusal> {
    NamespaceDriver::parse_with_clock(
        bytes,
        deployment,
        Arc::new(MemoryLedger::default()),
        ManualClock::new(),
    )
}

#[test]
fn a_configuration_the_extension_does_not_admit_is_refused_naming_the_member() {
    let fake = FakeCluster::start("substrate", TOKEN).expect("the fake starts");
    for (case, extension, member) in refused_extensions() {
        let bytes = kubeconfig_with(&fake, extension);
        let refusal = load(&bytes, DEPLOYMENT)
            .err()
            .unwrap_or_else(|| panic!("{case}: the configuration was admitted"));
        assert_eq!(refusal.code, CONFIG_INVALID, "{case}: {}", refusal.message);
        assert!(
            refusal.message.contains(member),
            "{case}: the refusal names {member}: {}",
            refusal.message
        );
        assert!(
            !refusal.message.contains(TOKEN),
            "{case}: the token is quoted"
        );
    }

    let mut doubled = fake.kubeconfig();
    doubled["contexts"][0]["context"]["extensions"] = json!([
        {"name": EXTENSION, "extension": extension()},
        {"name": EXTENSION, "extension": extension()},
    ]);
    let refusal = load(
        &serde_json::to_vec(&doubled).expect("serializes"),
        DEPLOYMENT,
    )
    .expect_err("an extension named twice is refused");
    assert_eq!(refusal.code, CONFIG_INVALID, "{}", refusal.message);

    let refusal = load(&kubeconfig_with(&fake, Some(extension())), "deploy/a")
        .expect_err("a deployment that is not a stable identifier is refused");
    assert_eq!(refusal.code, CONFIG_INVALID, "{}", refusal.message);
    assert!(
        fake.requests().is_empty(),
        "loading the configuration calls nothing"
    );
}

#[tokio::test]
async fn excess_rules_refuse_startup_before_the_lease_is_touched() {
    let fake = passing_fake();
    let clock = ManualClock::new();
    let (driver, _) = driver(&fake, &clock);

    let mut secrets = exact_rules();
    secrets.push(json!({"verbs": ["get"], "apiGroups": [""], "resources": ["secrets"]}));
    let mut wildcard = exact_rules();
    wildcard.push(json!({"verbs": ["*"], "apiGroups": [""], "resources": ["pods"]}));
    let mut extra_verb = exact_rules();
    extra_verb.push(json!({"verbs": ["update"], "apiGroups": ["coordination.k8s.io"], "resources": ["leases", "pods"]}));
    let mut unreadable = exact_rules();
    unreadable.push(json!({"verbs": ["get"], "resources": ["pods"]}));
    let mut nodes = exact_rules();
    nodes.push(json!({"verbs": ["get"], "apiGroups": [""], "resources": ["nodes"]}));
    let numbered = |prefix: &str| -> Vec<String> {
        (0..1_024).map(|index| format!("{prefix}{index}")).collect()
    };
    let mut product = exact_rules();
    product.push(
        json!({"verbs": numbered("v"), "apiGroups": numbered("g"), "resources": numbered("r")}),
    );
    for (case, rules) in [
        ("a verb on secrets", secrets),
        ("a wildcard verb", wildcard),
        ("a verb outside the table", extra_verb),
        ("a rule naming no apiGroups", unreadable),
        ("a resource outside the table", nodes),
        ("a rule stating a billion grants", product),
    ] {
        fake.set_rules(rules);
        let refusal = driver
            .open()
            .await
            .err()
            .unwrap_or_else(|| panic!("{case}: startup opened"));
        assert_eq!(refusal.code, RBAC_EXCESS, "{case}: {}", refusal.message);
        if case == "a rule stating a billion grants" {
            assert!(
                refusal.message.contains("1073741824 grant(s)") && refusal.message.len() < 4_096,
                "every grant is counted and only a bounded few are named: {}",
                refusal.message
            );
        }
    }
    assert!(
        !fake
            .requests()
            .iter()
            .any(|request| request.contains("/leases")),
        "the Lease was touched before the rules review refused: {:?}",
        fake.requests()
    );
    assert!(!driver.opened());

    fake.set_rules(exact_rules());
    driver.open().await.expect("the exact table opens");
    assert!(driver.opened());
}

#[tokio::test]
async fn a_lease_held_by_another_deployment_refuses_startup_whatever_its_age() {
    let fake = passing_fake();
    seed_lease(&fake, "deploy-b/aaaaaaaaaaaaaaaa");
    let before = lease(&fake);
    let clock = ManualClock::new();
    let (driver, _) = driver(&fake, &clock);
    for at in [0, 31, 3_600] {
        clock.set(Duration::from_secs(at));
        let refusal = driver.open().await.expect_err("a foreign Lease refuses");
        assert_eq!(
            refusal.code, LEASE_FOREIGN,
            "at {at} s: {}",
            refusal.message
        );
    }
    assert_eq!(lease(&fake), before, "the foreign Lease is never written");
    assert!(!driver.lease_held());
}

#[tokio::test]
async fn a_lease_is_taken_over_only_after_its_version_is_unchanged_for_thirty_seconds() {
    let fake = passing_fake();
    seed_lease(&fake, &format!("{DEPLOYMENT}/bbbbbbbbbbbbbbbb"));
    let clock = ManualClock::new();
    let (driver, _) = driver(&fake, &clock);

    let held = |at_ms: u64| {
        clock.set(Duration::from_millis(at_ms));
    };
    held(0);
    assert_eq!(driver.open().await.expect_err("held").code, LEASE_HELD);
    renew_as_other(&fake).await;
    held(30_000);
    let refusal = driver
        .open()
        .await
        .expect_err("renewed at 10 s: still held");
    assert_eq!(refusal.code, LEASE_HELD, "{}", refusal.message);
    held(59_999);
    let refusal = driver
        .open()
        .await
        .expect_err("29.999 s unchanged: still held");
    assert_eq!(refusal.code, LEASE_HELD, "{}", refusal.message);
    assert_eq!(
        lease(&fake)["spec"]["holderIdentity"],
        format!("{DEPLOYMENT}/bbbbbbbbbbbbbbbb")
    );

    held(60_000);
    driver.open().await.expect("30 s unchanged: taken over");
    let taken = lease(&fake);
    assert_eq!(taken["spec"]["holderIdentity"], driver.lease_holder());
    assert_eq!(taken["spec"]["leaseDurationSeconds"], 30);
    assert_eq!(taken["spec"]["leaseTransitions"], 1);
    assert!(driver.lease_held());
}

#[tokio::test]
async fn the_lease_is_held_only_while_the_last_renewal_is_under_twenty_seconds_old() {
    let fake = passing_fake();
    let clock = ManualClock::new();
    let (driver, _) = driver(&fake, &clock);
    driver.open().await.expect("an absent Lease is created");
    let admitted = driver.binding().expect("bound").config_generation();
    assert_eq!(
        lease(&fake)["spec"]["holderIdentity"],
        driver.lease_holder()
    );
    assert!(driver.lease_held());
    driver
        .check_binding(admitted)
        .await
        .expect("held and unchanged");

    clock.set(Duration::from_secs(10));
    driver.renew().await;
    let path = format!("PUT /apis/coordination.k8s.io/v1/namespaces/substrate/leases/{LEASE_NAME}");
    // More than the renewals below can meet: each renewal writes at most twice.
    for _ in 0..8 {
        fake.fault(Fault::Answer {
            method: Method::PUT,
            path: path.trim_start_matches("PUT ").to_owned(),
            status: 500,
            body: json!({"kind": "Status", "message": "etcd unavailable"}),
        });
    }
    for at in [20, 25] {
        clock.set(Duration::from_secs(at));
        driver.renew().await;
    }
    clock.set(Duration::from_millis(29_999));
    assert!(driver.lease_held(), "19.999 s after the last renewal");
    driver.check_binding(admitted).await.expect("still held");

    clock.set(Duration::from_secs(30));
    assert!(!driver.lease_held(), "20 s after the last renewal");
    let refusal = driver
        .check_binding(admitted)
        .await
        .expect_err("a lost Lease is stale");
    assert_eq!(refusal.code, CAPABILITY_STALE, "{}", refusal.message);
    assert_eq!(refusal.address.as_deref(), Some("capability_snapshot"));
}

#[tokio::test]
async fn a_policy_that_is_not_a_default_deny_refuses_startup() {
    let cases = [
        ("absent", None),
        (
            "a pod selector",
            Some(
                json!({"podSelector": {"matchLabels": {"a": "b"}}, "policyTypes": ["Ingress", "Egress"]}),
            ),
        ),
        (
            "an egress rule",
            Some(json!({"podSelector": {}, "policyTypes": ["Ingress", "Egress"], "egress": [{}]})),
        ),
        (
            "ingress only",
            Some(json!({"podSelector": {}, "policyTypes": ["Ingress"]})),
        ),
        (
            "a type named twice",
            Some(json!({"podSelector": {}, "policyTypes": ["Egress", "Egress"]})),
        ),
    ];
    for (case, spec) in cases {
        let fake = FakeCluster::start("substrate", TOKEN).expect("the fake starts");
        fake.set_rules(exact_rules());
        fake.enforce_restricted(true);
        if let Some(spec) = spec {
            fake.seed(
                "networkpolicies",
                json!({"metadata": {"name": POLICY}, "spec": spec}),
            )
            .expect("stored");
        }
        let clock = ManualClock::new();
        let (driver, _) = driver(&fake, &clock);
        let refusal = driver
            .open()
            .await
            .err()
            .unwrap_or_else(|| panic!("{case}: startup opened"));
        assert_eq!(
            refusal.code, POLICY_NOT_DEFAULT_DENY,
            "{case}: {}",
            refusal.message
        );
        assert!(!driver.opened(), "{case}");
    }
}

#[tokio::test]
async fn an_admitted_privileged_dry_run_refuses_startup() {
    let fake = passing_fake();
    fake.enforce_restricted(false);
    let clock = ManualClock::new();
    let (driver, _) = driver(&fake, &clock);
    let refusal = driver
        .open()
        .await
        .expect_err("an unenforced namespace refuses");
    assert_eq!(refusal.code, POD_SECURITY_UNENFORCED, "{}", refusal.message);
    assert!(
        fake.requests()
            .iter()
            .any(|request| request.starts_with("POST /api/v1/namespaces/substrate/pods?dryRun=All")),
        "{:?}",
        fake.requests()
    );
    assert!(
        admin(&fake)
            .list(
                &admin(&fake).pods(),
                "PodList",
                substrate_kubernetes_client::ListForm::Whole,
                10,
                1
            )
            .await
            .expect("listed")
            .items
            .is_empty(),
        "the dry run stored nothing"
    );

    fake.enforce_restricted(true);
    driver.open().await.expect("an enforcing namespace opens");
}

#[tokio::test]
async fn the_binding_moves_with_the_namespace_policy_set() {
    let fake = passing_fake();
    let clock = ManualClock::new();
    let (driver, _) = driver(&fake, &clock);
    driver.open().await.expect("opens");
    let binding = driver.binding().expect("bound at open");
    assert_ne!(binding.config_generation(), 0);
    driver
        .check_binding(binding.config_generation())
        .await
        .expect("unchanged");

    fake.seed(
        "networkpolicies",
        json!({"metadata": {"name": "allow-egress"}, "spec": {"podSelector": {}, "policyTypes": ["Egress"], "egress": [{}]}}),
    )
    .expect("an additive allow is stored");
    let refusal = driver
        .check_binding(binding.config_generation())
        .await
        .expect_err("an added policy is stale");
    assert_eq!(refusal.code, CAPABILITY_STALE, "{}", refusal.message);

    driver.open().await.expect("reopens over the new set");
    let moved = driver.binding().expect("bound again");
    assert_ne!(moved.digest(), binding.digest());
    assert_ne!(moved.config_generation(), binding.config_generation());
    driver
        .check_binding(moved.config_generation())
        .await
        .expect("the new binding holds");
}

/// An object carrying the managed-by and deployment labels and the operation annotation.
fn owned_object(name: &str, deployment: &str, operation: &str) -> Value {
    json!({
        "metadata": {
            "name": name,
            "labels": {MANAGED_BY_LABEL: MANAGED_BY, DEPLOYMENT_LABEL: deployment},
            "annotations": {OPERATION_ANNOTATION: format!("sha256:{operation}")},
        },
        "spec": {},
    })
}

/// The stored form of every `(resource, name)`, each of which must be present.
fn snapshot(fake: &FakeCluster, objects: &[(&str, &str)]) -> Vec<Value> {
    objects
        .iter()
        .map(|(resource, name)| fake.object(resource, name).expect("present"))
        .collect()
}

#[tokio::test]
async fn foreign_deployment_and_foreign_uid_objects_are_never_touched() {
    let fake = passing_fake();
    let clock = ManualClock::new();
    let (driver, ledger) = driver(&fake, &clock);
    driver.open().await.expect("opens");
    let ours = names::deployment_label(DEPLOYMENT);
    let theirs = names::deployment_label("deploy-b");
    let digest = "a".repeat(64);

    let foreign = names::workspace_pvc(DEPLOYMENT, "ws_foreign");
    let pvc = (BackendKind::Workspace, BackendRole::Pvc);
    ledger.reserve(pvc.0, "ws_foreign", pvc.1, &foreign, &digest);
    fake.seed(
        "persistentvolumeclaims",
        owned_object(&foreign, &theirs, &digest),
    )
    .expect("stored");

    let pod = (BackendKind::Exec, BackendRole::Pod);
    let replaced = names::exec_pod(DEPLOYMENT, "ex_replaced");
    ledger.reserve(pod.0, "ex_replaced", pod.1, &replaced, &digest);
    ledger
        .bind_uid(&replaced, "00000000-0000-4000-8000-ffffffffffff")
        .expect("bound");
    fake.seed("pods", owned_object(&replaced, &ours, &digest))
        .expect("stored");

    let unproved = names::exec_pod(DEPLOYMENT, "ex_unproved");
    ledger.reserve(pod.0, "ex_unproved", pod.1, &unproved, &digest);
    fake.seed("pods", owned_object(&unproved, &ours, &"b".repeat(64)))
        .expect("stored");

    let colliding = [
        ("persistentvolumeclaims", foreign.as_str()),
        ("pods", replaced.as_str()),
        ("pods", unproved.as_str()),
    ];
    let before = snapshot(&fake, &colliding);
    for name in [&foreign, &replaced, &unproved] {
        let removal = driver
            .remove_owned(&ledger.row(name), 1)
            .await
            .expect("answered");
        assert_eq!(removal, Removal::Collision, "{name}");
    }
    assert_eq!(
        snapshot(&fake, &colliding),
        before,
        "a collision is never modified"
    );
    assert!(
        !fake
            .requests()
            .iter()
            .any(|request| request.starts_with("DELETE")),
        "{:?}",
        fake.requests()
    );
    assert_eq!(driver.collisions_total(), 3);
    for name in [&foreign, &unproved] {
        assert_eq!(
            ledger.row(name).uid,
            None,
            "{name}: a collision binds nothing"
        );
        assert_eq!(ledger.row(name).settled_at, None, "{name}");
    }

    let adopted = names::exec_pod(DEPLOYMENT, "ex_adopted");
    ledger.reserve(pod.0, "ex_adopted", pod.1, &adopted, &digest);
    let stored = fake
        .seed("pods", owned_object(&adopted, &ours, &digest))
        .expect("a create whose answer was lost");
    let removal = driver
        .remove_owned(&ledger.row(&adopted), 1)
        .await
        .expect("answered");
    assert_eq!(removal, Removal::Absent);
    assert_eq!(
        ledger.row(&adopted).uid,
        stored["metadata"]["uid"].as_str().map(str::to_owned)
    );
    assert!(ledger.row(&adopted).settled_at.is_some());
    assert!(fake.object("pods", &adopted).is_none());
    assert_eq!(driver.collisions_total(), 3);
}

#[test]
fn names_labels_and_bounds_are_derived_from_the_deployment_and_the_id() {
    let hash = names::resource_hash(DEPLOYMENT, "ws_01");
    // Derived outside the crate, with sha256sum and coreutils base32: a name outlives a release,
    // so its derivation may never drift.
    assert_eq!(hash, "mmzaqi6nnhqe3youf65sclqmra");
    assert_eq!(
        names::deployment_label(DEPLOYMENT),
        "c453bs4zphyuzprofz7gvtqiby"
    );
    assert_eq!(hash.len(), 26);
    assert!(
        hash.bytes()
            .all(|byte| byte.is_ascii_lowercase() || (b'2'..=b'7').contains(&byte))
    );
    assert_eq!(hash, names::resource_hash(DEPLOYMENT, "ws_01"));
    assert_ne!(hash, names::resource_hash("deploy-b", "ws_01"));
    assert_ne!(hash, names::resource_hash(DEPLOYMENT, "ws_02"));
    assert_ne!(
        names::resource_hash("ab", "c"),
        names::resource_hash("a", "bc"),
        "the parts are length-prefixed"
    );
    assert_eq!(
        names::workspace_pvc(DEPLOYMENT, "ws_01"),
        format!("sbx-ws-{hash}")
    );
    let keeper = names::keeper_pod(DEPLOYMENT, "ws_01");
    assert_eq!(keeper, format!("sbx-ws-{hash}-keeper"));
    assert_eq!(keeper.len(), "sbx-ws-".len() + 26 + "-keeper".len());
    assert!(substrate_kubernetes_client::is_dns_label(&keeper));
    assert_eq!(
        names::exec_pod(DEPLOYMENT, "ex_01"),
        format!("sbx-ex-{}", names::resource_hash(DEPLOYMENT, "ex_01"))
    );
    assert_eq!(names::deployment_label(DEPLOYMENT).len(), 26);

    let nonce = names::probe_nonce();
    assert_eq!(nonce.len(), 16);
    assert_ne!(nonce, names::probe_nonce());
    let probe: Vec<String> = names::probe_names(&nonce)
        .into_iter()
        .map(|name| name.name)
        .collect();
    assert_eq!(
        probe,
        ["pvc", "keeper", "pod"].map(|role| format!("sbx-pr-{nonce}-{role}"))
    );

    let labels = names::labels(DEPLOYMENT, BackendKind::Exec, &hash, BackendRole::Pod);
    assert_eq!(labels.len(), 4);
    assert_eq!(labels[MANAGED_BY_LABEL], MANAGED_BY);
    assert_eq!(
        labels[DEPLOYMENT_LABEL],
        names::deployment_label(DEPLOYMENT)
    );
    assert_eq!(labels["b10x-substrate/exec"], hash.as_str());
    assert_eq!(labels["b10x-substrate/role"], "pod");
    assert_eq!(
        names::operation_annotation(&"c".repeat(64)),
        format!("sha256:{}", "c".repeat(64))
    );

    let fake = FakeCluster::start("substrate", TOKEN).expect("the fake starts");
    let (driver, _) = driver(&fake, &ManualClock::new());
    assert_eq!(
        driver.workspace_root_identity("ws_01"),
        names::workspace_pvc(DEPLOYMENT, "ws_01")
    );
    assert_eq!(
        driver.backend_names(BackendKind::Workspace, "ws_01"),
        vec![
            BackendName {
                role: BackendRole::Pvc,
                name: names::workspace_pvc(DEPLOYMENT, "ws_01")
            },
            BackendName {
                role: BackendRole::Keeper,
                name: keeper
            },
        ]
    );
    assert_eq!(
        driver.backend_names(BackendKind::Exec, "ex_01"),
        vec![BackendName {
            role: BackendRole::Pod,
            name: names::exec_pod(DEPLOYMENT, "ex_01")
        }]
    );
    assert!(driver.backend_names(BackendKind::Probe, "p").is_empty());
    assert_eq!(driver.dispatch_bound(), Duration::from_secs(90));
    assert!(driver.lease_holder().starts_with(&format!("{DEPLOYMENT}/")));
}

/// Design 34 § 6.3 and ADR 0047 decision 9: before the Pod create the driver compares the re-read
/// policy set with the binding of the snapshot the start was **admitted** under. A re-open (what
/// retaking the Lease runs, § 6.1) over a changed set binds the new set; a start admitted under
/// the first binding must still be stale. Red at `e67f1878`: `check_binding` took no admitted
/// binding, so it could only compare with the latest open, and this start passed; it now takes
/// the admitted `config_generation`.
#[tokio::test]
async fn a_start_admitted_before_a_reopen_over_a_changed_policy_set_is_stale() {
    let fake = passing_fake();
    let clock = ManualClock::new();
    let (driver, _) = driver(&fake, &clock);
    driver.open().await.expect("opens");
    let admitted = driver
        .binding()
        .expect("the start is admitted under this binding");

    fake.seed(
        "networkpolicies",
        json!({"metadata": {"name": "allow-egress"}, "spec": {"podSelector": {}, "policyTypes": ["Egress"], "egress": [{}]}}),
    )
    .expect("an additive allow is stored after the admission");
    driver
        .open()
        .await
        .expect("a re-open binds the new policy set");
    let current = driver.binding().expect("bound again");
    assert_ne!(current.config_generation(), admitted.config_generation());

    let refusal = driver
        .check_binding(admitted.config_generation())
        .await
        .expect_err(
        "the Pod create of a start admitted under the first binding must be refused: the policy \
         set it was admitted under is gone",
    );
    assert_eq!(refusal.code, CAPABILITY_STALE, "{}", refusal.message);
}

/// Design 34 § 5.1 and N9: an unbound row's absence is authoritative only past the N9 bound,
/// because an unanswered create may still be stored (`ApiError::Unanswered`). A removal that reads
/// an unbound row absent and settles it at once leaves a create that lands afterwards in the
/// namespace for good: the settled row makes it a collision, never deleted.
#[tokio::test]
async fn an_unbound_row_read_absent_still_removes_an_object_whose_create_lands_later() {
    let fake = passing_fake();
    let clock = ManualClock::new();
    let (driver, ledger) = driver(&fake, &clock);
    let ours = names::deployment_label(DEPLOYMENT);
    let digest = "d".repeat(64);
    let name = names::exec_pod(DEPLOYMENT, "ex_in_flight");
    ledger.reserve(
        BackendKind::Exec,
        "ex_in_flight",
        BackendRole::Pod,
        &name,
        &digest,
    );

    let first = driver
        .remove_owned(&ledger.row(&name), 1)
        .await
        .expect("answered");
    fake.seed("pods", owned_object(&name, &ours, &digest))
        .expect("the unanswered create is stored late");
    let second = driver
        .remove_owned(&ledger.row(&name), 1)
        .await
        .expect("answered");

    assert!(
        fake.object("pods", &name).is_none(),
        "the late object, ours by its operation annotation, is left in the namespace: first \
         removal {first:?}, second {second:?}, row settled at {:?}",
        ledger.row(&name).settled_at
    );
}

/// Design 34 § 5.1 and N9, the other side of the case above: an unbound row read absent stays
/// open, and its removal says so, until the N9 bound has passed since the row was reserved; past
/// it the absence is dispatch absence and the row is settled. A bound row read absent is physical
/// absence and is settled at once.
#[tokio::test]
async fn an_unbound_row_read_absent_is_settled_only_past_the_n9_bound() {
    let fake = passing_fake();
    let clock = ManualClock::new();
    let (driver, ledger) = driver(&fake, &clock);
    let digest = "e".repeat(64);
    let bound_name = names::exec_pod(DEPLOYMENT, "ex_bound");
    ledger.reserve(
        BackendKind::Exec,
        "ex_bound",
        BackendRole::Pod,
        &bound_name,
        &digest,
    );
    ledger
        .bind_uid(&bound_name, "00000000-0000-4000-8000-000000000001")
        .expect("bound");
    let removal = driver
        .remove_owned(&ledger.row(&bound_name), 1)
        .await
        .expect("answered");
    assert_eq!(
        removal,
        Removal::Absent,
        "a bound row's absence is physical"
    );
    assert!(ledger.row(&bound_name).settled_at.is_some());

    let name = names::exec_pod(DEPLOYMENT, "ex_unbound");
    ledger.reserve(
        BackendKind::Exec,
        "ex_unbound",
        BackendRole::Pod,
        &name,
        &digest,
    );
    let bound = driver.dispatch_bound();
    let short_of_it = bound
        .checked_sub(Duration::from_secs(1))
        .expect("the N9 bound is over a second");
    for at in [Duration::ZERO, short_of_it] {
        clock.set(at);
        let removal = driver
            .remove_owned(&ledger.row(&name), 1)
            .await
            .expect("answered");
        assert_eq!(removal, Removal::Unconfirmed, "at {at:?}");
        assert_eq!(ledger.row(&name).settled_at, None, "at {at:?}");
    }
    clock.set(bound + Duration::from_secs(1));
    let removal = driver
        .remove_owned(&ledger.row(&name), 1)
        .await
        .expect("answered");
    assert_eq!(removal, Removal::Absent, "past the N9 bound");
    assert!(ledger.row(&name).settled_at.is_some());
}

/// N4: a Lease labelled for another deployment is that deployment's, whatever its holder says.
/// An empty holder is not an invitation to take it, and a holder naming this deployment does not
/// make it this deployment's. An unlabelled free Lease is taken and carries this deployment's
/// labels from then on.
#[tokio::test]
async fn a_lease_labelled_for_another_deployment_is_foreign_even_with_an_empty_holder() {
    let theirs = names::deployment_label("deploy-b");
    for holder in ["", "deploy-a/cccccccccccccccc"] {
        let fake = passing_fake();
        fake.seed(
            "leases",
            json!({
                "metadata": {
                    "name": LEASE_NAME,
                    "labels": {MANAGED_BY_LABEL: MANAGED_BY, DEPLOYMENT_LABEL: theirs},
                },
                "spec": {"holderIdentity": holder, "leaseDurationSeconds": 30},
            }),
        )
        .expect("the Lease is stored");
        let before = lease(&fake);
        let clock = ManualClock::new();
        let (driver, _) = driver(&fake, &clock);
        for at in [0, 31] {
            clock.set(Duration::from_secs(at));
            let refusal = driver
                .open()
                .await
                .expect_err("a Lease labelled for another deployment refuses");
            assert_eq!(
                refusal.code, LEASE_FOREIGN,
                "holder {holder:?} at {at} s: {}",
                refusal.message
            );
        }
        assert_eq!(lease(&fake), before, "holder {holder:?}: never written");
        assert!(!driver.lease_held());
    }

    let fake = passing_fake();
    seed_lease(&fake, "");
    let clock = ManualClock::new();
    let (driver, _) = driver(&fake, &clock);
    driver
        .open()
        .await
        .expect("a free unlabelled Lease is taken");
    let taken = lease(&fake);
    assert_eq!(taken["spec"]["holderIdentity"], driver.lease_holder());
    assert_eq!(
        taken["metadata"]["labels"][DEPLOYMENT_LABEL],
        names::deployment_label(DEPLOYMENT)
    );
    assert_eq!(taken["metadata"]["labels"][MANAGED_BY_LABEL], MANAGED_BY);
}

/// N6: the Lease is checked after the policy read, so a read that outlasts the Lease cannot pass
/// on a check made before it. The read is held at the fake while the clock moves past
/// [`crate::lease::HELD_FOR`].
#[tokio::test]
async fn a_lease_lost_while_the_policy_read_is_in_flight_is_stale() {
    let fake = passing_fake();
    let clock = ManualClock::new();
    let (driver, _) = driver(&fake, &clock);
    driver.open().await.expect("opens");
    let admitted = driver.binding().expect("bound").config_generation();
    let list = format!("GET {}", driver.api().policies());
    fake.fault(Fault::Delay {
        method: Method::GET,
        path: driver.api().policies(),
        by: Duration::from_millis(500),
    });
    let sent = || {
        fake.requests()
            .iter()
            .filter(|request| request.starts_with(&list))
            .count()
    };
    let before = sent();
    let advance = async {
        for _ in 0..400 {
            if sent() > before {
                clock.set(Duration::from_secs(25));
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("the policy read was never sent");
    };
    let (checked, ()) = tokio::join!(driver.check_binding(admitted), advance);
    let refusal = checked.expect_err("the Lease lapsed while the policy read was in flight");
    assert_eq!(refusal.code, CAPABILITY_STALE, "{}", refusal.message);
    assert!(refusal.message.contains("Lease"), "{}", refusal.message);
}

#[tokio::test]
async fn the_default_grants_a_v1_37_cluster_shows_open_without_excess() {
    let fake = passing_fake();
    let mut rules = exact_rules();
    rules.push(json!({
        "verbs": ["get", "list", "watch"],
        "apiGroups": ["certificates.k8s.io"],
        "resources": ["clustertrustbundles"],
    }));
    fake.set_rules(rules);
    let clock = ManualClock::new();
    let (driver, _) = driver(&fake, &clock);
    driver
        .open()
        .await
        .expect("the grants the lane observed on v1.37 are the cluster's own defaults");
    assert!(driver.opened());
}

/// Adversary pass 2, `story:kubernetes-collisions-metric`: `remove_owned`'s 409 arm. The read
/// shows the row's own object (uid bound), the precondition then meets a foreign replacement
/// (another deployment, another uid), and `planned_get_failure` says whether the arm's one read
/// of the replacement answers. Returns the count moved over three removals, and checks that every
/// removal ends `Collision`, the replacement is untouched and the row is neither rebound nor
/// settled.
async fn removals_through_the_409_arm(planned_get_failure: bool) -> u64 {
    let fake = passing_fake();
    let clock = ManualClock::new();
    let (driver, ledger) = driver(&fake, &clock);
    driver.open().await.expect("opens");
    let digest = "a".repeat(64);
    let name = names::exec_pod(DEPLOYMENT, "ex_swapped");
    ledger.reserve(
        BackendKind::Exec,
        "ex_swapped",
        BackendRole::Pod,
        &name,
        &digest,
    );
    let bound = "00000000-0000-4000-8000-0000000000aa";
    ledger.bind_uid(&name, bound).expect("bound");
    let replacement = fake
        .seed(
            "pods",
            owned_object(&name, &names::deployment_label("deploy-b"), &digest),
        )
        .expect("a foreign replacement holds the name");
    let mut ours = owned_object(&name, &names::deployment_label(DEPLOYMENT), &digest);
    ours["metadata"]["uid"] = json!(bound);
    let path = format!("/api/v1/namespaces/substrate/pods/{name}");
    fake.fault(Fault::Answer {
        method: Method::GET,
        path: path.clone(),
        status: 200,
        body: ours,
    });
    if planned_get_failure {
        fake.fault(Fault::Answer {
            method: Method::GET,
            path,
            status: 500,
            body: json!({"kind": "Status", "apiVersion": "v1", "status": "Failure",
                "reason": "InternalError", "code": 500, "message": "etcd unavailable"}),
        });
    }
    let before = driver.collisions_total();
    for round in 0..3 {
        let removal = driver
            .remove_owned(&ledger.row(&name), 1)
            .await
            .expect("answered");
        assert_eq!(removal, Removal::Collision, "round {round}");
    }
    assert_eq!(
        fake.object("pods", &name),
        Some(replacement),
        "the replacement is never modified"
    );
    assert_eq!(ledger.row(&name).uid.as_deref(), Some(bound));
    assert_eq!(ledger.row(&name).settled_at, None);
    driver.collisions_total() - before
}

/// Adversary pass 2, `story:kubernetes-collisions-metric`: the 409 arm reads the replacement so
/// the next removal, which meets it under the uid rule, does not count it again. One foreign
/// object, three removals: one. With the arm's read dropped (`replacement = None`) the same
/// removals count two, which `the_409_arm_whose_read_fails_counts_its_replacement_twice` shows.
#[tokio::test]
async fn adversary_a_replacement_met_in_the_409_arm_is_one_object_across_removals() {
    assert_eq!(
        removals_through_the_409_arm(false).await,
        1,
        "story:kubernetes-collisions-metric: a replacement read in the 409 arm and met again by \
         the next removals is one object"
    );
}

/// Adversary pass 2, `story:kubernetes-collisions-metric`: the 409 arm's read of the replacement
/// fails. The removal still ends `Collision` and touches nothing; the replacement is counted
/// unread and then again under its uid by the next removal: two for one object, the overcount
/// `NamespaceDriver::collision`'s doc accepts for an object the driver could not read. This is
/// also what the 409 arm would count with its read removed.
#[tokio::test]
async fn adversary_the_409_arm_whose_read_fails_counts_its_replacement_twice() {
    assert_eq!(
        removals_through_the_409_arm(true).await,
        2,
        "story:kubernetes-collisions-metric: the documented overcount of an unread object"
    );
}

/// Adversary pass 2, `story:kubernetes-collisions-metric`: the remembered set at the driver's own
/// bound. The first `COLLISIONS_REMEMBERED` objects are counted once each and stay remembered;
/// object 4097 is counted every time it is met.
#[test]
fn adversary_the_remembered_set_holds_exactly_collisions_remembered_objects() {
    use crate::ownership::{COLLISIONS_REMEMBERED, Collisions, Foreign};
    let object = |index: usize| {
        Foreign::of(
            BackendRole::Pod,
            &json!({"metadata": {"name": format!("sbx-ex-{index}"), "uid": format!("u{index}")}}),
        )
    };
    let collisions = Collisions::default();
    for index in 0..COLLISIONS_REMEMBERED {
        assert!(collisions.meet(Some(object(index)), COLLISIONS_REMEMBERED));
    }
    for index in [0, COLLISIONS_REMEMBERED - 1] {
        assert!(
            !collisions.meet(Some(object(index)), COLLISIONS_REMEMBERED),
            "object {index} is remembered"
        );
    }
    let past = object(COLLISIONS_REMEMBERED);
    assert!(collisions.meet(Some(past.clone()), COLLISIONS_REMEMBERED));
    assert!(collisions.meet(Some(past), COLLISIONS_REMEMBERED));
    assert_eq!(
        collisions.total(),
        u64::try_from(COLLISIONS_REMEMBERED).expect("small") + 2
    );
}

/// Adversary pass 2, `story:kubernetes-collisions-metric`: one foreign object met by many tasks at
/// once is counted once, and distinct objects met at once are each counted.
#[test]
fn adversary_one_object_met_concurrently_is_counted_once() {
    use crate::ownership::{COLLISIONS_REMEMBERED, Collisions, Foreign};
    let collisions = Collisions::default();
    let same = Foreign::of(
        BackendRole::Pvc,
        &json!({"metadata": {"name": "sbx-ws-a", "uid": "u-same"}}),
    );
    std::thread::scope(|scope| {
        for thread in 0..16 {
            let collisions = &collisions;
            let same = same.clone();
            scope.spawn(move || {
                for _ in 0..200 {
                    collisions.meet(Some(same.clone()), COLLISIONS_REMEMBERED);
                }
                let own = Foreign::of(
                    BackendRole::Pvc,
                    &json!({"metadata": {"name": format!("sbx-ws-{thread}"), "uid": format!("u{thread}")}}),
                );
                collisions.meet(Some(own), COLLISIONS_REMEMBERED);
            });
        }
    });
    assert_eq!(collisions.total(), 1 + 16);
}

/// Adversary, D333: the kernel answers a TCP connect to a broadcast or multicast address
/// `ENETUNREACH` before any packet is sent (`tcp_v4_connect`, `tcp_v6_connect`), whatever network
/// plugin or `NetworkPolicy` is in force, so such an `egress_probe_address` can never test egress.
/// Since D334 `Unreachable` no longer counts as stopped, but an address that cannot test anything
/// is still a configuration error, refused as loopback and unspecified addresses are.
#[test]
fn adversary_d333_a_broadcast_or_multicast_egress_probe_address_is_refused() {
    use substrate_executor::probe::{ATTEMPT_BOUND, EgressOutcome, EgressTarget, attempt_tcp};

    let fake = FakeCluster::start("substrate", TOKEN).expect("the fake starts");
    for address in [
        "255.255.255.255:443",
        "224.0.0.1:443",
        "239.255.0.1:443",
        "[ff02::1]:443",
    ] {
        let attempt = attempt_tcp(
            EgressTarget::EgressProbe,
            Some(address.parse().expect("an address")),
            ATTEMPT_BOUND,
        );
        assert_eq!(
            attempt.outcome,
            EgressOutcome::Unreachable,
            "the precondition: {address} is unreachable with no policy at all: {attempt:?}"
        );
        assert!(
            !attempt.outcome.blocked(),
            "and it does not count as stopped (D334)"
        );

        let mut extension = extension();
        extension["egress_probe_address"] = json!(address);
        let refusal = load(&kubeconfig_with(&fake, Some(extension)), DEPLOYMENT)
            .err()
            .unwrap_or_else(|| {
                panic!(
                    "{address}: admitted as egress_probe_address, so the probe's third attempt is \
                     Unreachable on any cluster and tests nothing"
                )
            });
        assert_eq!(
            refusal.code, CONFIG_INVALID,
            "{address}: {}",
            refusal.message
        );
        assert!(
            refusal.message.contains("egress_probe_address"),
            "{address}: {}",
            refusal.message
        );
    }
}
