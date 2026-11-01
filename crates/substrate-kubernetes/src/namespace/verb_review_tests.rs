//! A bare exec refusal and the access review that names its verb
//! (`story:driver-names-a-bare-exec-refusals-verb`, design 34 § 7.1). On kind v1.37 the API
//! refuses an exec upgrade the Role lacks `pods/exec` `create` for with a 403 whose message ends
//! at "is forbidden: " (D349). The driver then asks a `SelfSubjectAccessReview` for `pods/exec`
//! `create` and `get` on the keeper and names the verb it denies, or that it did not decide, or,
//! when the review itself is refused or fails, that the verb is unknown. The handshake ends at
//! once either way. Every case runs once on the fake API server, in seconds.

use std::time::{Duration, Instant};

use serde_json::{Value, json};
use substrate_host::DriverError;
use substrate_kubernetes_client::Method;
use substrate_kubernetes_client::fake::{FakeCluster, Fault};

use crate::driver_tests::{ManualClock, driver, passing_fake};
use crate::rules::{DEFAULT_RESOURCE_RULES, ROLE_RULES};
use crate::{API_FAILED, EXECUTOR_HANDSHAKE_FAILED, NamespaceDriver};

/// A keeper name of the length the namespace lane's probe gives one.
const KEEPER: &str = "sbx-pr-gbyb7pajquihb757-keeper";
const ACCESS_REVIEWS: &str = "/apis/authorization.k8s.io/v1/selfsubjectaccessreviews";
/// What the driver's refusal said for the bare 403 the lane kept at `bb2b06fe`, the trailing
/// space dropped.
const BARE: &str =
    "the cluster API: exec answered 403: pods \"sbx-pr-gbyb7pajquihb757-keeper\" is forbidden:";
/// What the API's authorization answers an exec upgrade under a Role without `pods/exec`
/// `create` when it gives a reason (D342).
const REASONED: &str = "pods \"sbx-pr-gbyb7pajquihb757-keeper\" is forbidden: User \
     \"system:serviceaccount:substrate:substrate-driver\" cannot create resource \"pods/exec\" \
     in API group \"\" in the namespace \"substrate\"";
/// The longest a case lets one handshake take: well past the review's own bound, far short of
/// a retry until the create bound, which the manual clock never reaches.
const AT_ONCE: Duration = Duration::from_secs(15);

/// The rules review of ADR 0047 decision 3's Role and the default set, with `pods/exec` holding
/// only `exec_verbs`: the rules the fake's access reviews decide by.
fn rules_with_exec(exec_verbs: &[&str]) -> Vec<Value> {
    ROLE_RULES
        .iter()
        .chain(DEFAULT_RESOURCE_RULES.iter())
        .filter_map(|(group, resource, verbs)| {
            let verbs: Vec<&str> = if *resource == "pods/exec" {
                exec_verbs.to_vec()
            } else {
                verbs.to_vec()
            };
            (!verbs.is_empty())
                .then(|| json!({"verbs": verbs, "apiGroups": [group], "resources": [resource]}))
        })
        .collect()
}

/// A fake whose rules give `pods/exec` only `exec_verbs` and whose exec upgrades it refuses
/// `status` with `message`, and a driver over it that is not opened: the handshake needs none.
fn refusing(exec_verbs: &[&str], status: u16, message: &str) -> (FakeCluster, NamespaceDriver) {
    let fake = passing_fake();
    fake.set_rules(rules_with_exec(exec_verbs));
    fake.refuse_exec(status, message);
    let clock = ManualClock::new();
    let (driver, _) = driver(&fake, &clock);
    (fake, driver)
}

/// The keeper's handshake as a create awaits it: its error, once it ends, which must be within
/// [`AT_ONCE`] and after exactly one exec upgrade.
async fn handshake_error(fake: &FakeCluster, driver: &NamespaceDriver) -> DriverError {
    let started = Instant::now();
    let error = tokio::time::timeout(AT_ONCE, driver.await_handshake(KEEPER))
        .await
        .expect("a refused handshake ends at once, never at the create bound")
        .expect_err("the API refused the exec");
    assert!(started.elapsed() < AT_ONCE);
    let upgrades = fake
        .requests()
        .iter()
        .filter(|request| request.contains(&format!("/pods/{KEEPER}/exec?")))
        .count();
    assert_eq!(
        upgrades,
        1,
        "the refused handshake was retried: {:#?}",
        fake.requests()
    );
    assert_eq!(error.code, API_FAILED, "{}", error.message);
    error
}

/// The access reviews the fake received.
fn reviews(fake: &FakeCluster) -> usize {
    fake.requests()
        .iter()
        .filter(|request| **request == format!("POST {ACCESS_REVIEWS}"))
        .count()
}

/// Acceptance 1: a bare 403 has the driver review `pods/exec` `create` and then `get` on the
/// keeper, and the refusal names the verb the review denies after the API's own answer: `create`
/// for the Role the lane observed, both when both are missing, `get` when only it is.
#[tokio::test]
async fn a_bare_exec_refusal_names_the_verb_the_access_review_denies() {
    for (exec_verbs, named) in [
        (&["get"][..], "create"),
        (&[][..], "create and get"),
        (&["create"][..], "get"),
    ] {
        let (fake, driver) = refusing(
            exec_verbs,
            403,
            &format!("pods \"{KEEPER}\" is forbidden: "),
        );
        let error = handshake_error(&fake, &driver).await;
        assert_eq!(
            error.message,
            format!("{BARE} (a SelfSubjectAccessReview denies pods/exec {named})"),
            "a Role with pods/exec {exec_verbs:?}"
        );
        assert_eq!(reviews(&fake), 2, "{:#?}", fake.requests());
    }
}

/// Acceptance 1: a review that admits both verbs cannot say which one the API refused, and the
/// refusal says so after the API's own answer.
#[tokio::test]
async fn a_bare_exec_refusal_whose_review_admits_both_verbs_says_it_did_not_decide() {
    let (fake, driver) = refusing(
        &["get", "create"],
        403,
        &format!("pods \"{KEEPER}\" is forbidden: "),
    );
    let error = handshake_error(&fake, &driver).await;
    assert_eq!(
        error.message,
        format!(
            "{BARE} (a SelfSubjectAccessReview admits pods/exec create and get: it does not \
             decide the refused verb)"
        )
    );
    assert_eq!(reviews(&fake), 2, "{:#?}", fake.requests());
}

/// Acceptance 2: the review refused, its answer lost, a server error or an answer without
/// `status.allowed` keeps the API's refusal of the exec and names the verb unknown, with why the
/// review failed; the handshake still ends at once, and no second review is asked.
#[tokio::test]
async fn a_refused_or_failing_review_keeps_the_refusal_and_names_the_verb_unknown() {
    let refused = json!({
        "kind": "Status", "apiVersion": "v1", "metadata": {}, "status": "Failure",
        "message": "selfsubjectaccessreviews.authorization.k8s.io is forbidden: User \
                    \"system:serviceaccount:substrate:substrate-driver\" cannot create resource \
                    \"selfsubjectaccessreviews\" in API group \"authorization.k8s.io\" at the \
                    cluster scope",
        "reason": "Forbidden", "code": 403,
    });
    for (fault, why) in [
        (
            Fault::Answer {
                method: Method::POST,
                path: ACCESS_REVIEWS.to_owned(),
                status: 403,
                body: refused,
            },
            "access review answered 403: selfsubjectaccessreviews.authorization.k8s.io is \
             forbidden",
        ),
        (
            Fault::DropAnswer {
                method: Method::POST,
                path: ACCESS_REVIEWS.to_owned(),
            },
            "HTTP",
        ),
        (
            Fault::Answer {
                method: Method::POST,
                path: ACCESS_REVIEWS.to_owned(),
                status: 500,
                body: json!({"kind": "Status", "message": "etcd is unavailable", "code": 500}),
            },
            "access review answered 500: etcd is unavailable",
        ),
        (
            Fault::Answer {
                method: Method::POST,
                path: ACCESS_REVIEWS.to_owned(),
                status: 201,
                body: json!({"kind": "SelfSubjectAccessReview", "status": {}}),
            },
            "no status.allowed",
        ),
    ] {
        let (fake, driver) = refusing(&["get"], 403, &format!("pods \"{KEEPER}\" is forbidden: "));
        fake.fault(fault);
        let error = handshake_error(&fake, &driver).await;
        let unknown = format!(
            "{BARE} (the refused verb is unknown: the SelfSubjectAccessReview of pods/exec \
             create failed: "
        );
        assert!(
            error.message.starts_with(&unknown) && error.message.ends_with(')'),
            "the refusal is not kept with the verb unknown: {}",
            error.message
        );
        assert!(error.message.contains(why), "{why}: {}", error.message);
        assert_eq!(reviews(&fake), 1, "{:#?}", fake.requests());
    }
}

/// Acceptance 2: a review the API does not answer is a failure within the review's own bound,
/// not the client's 30 s per call: the handshake ends with the verb unknown long before the
/// review's answer would come.
#[tokio::test]
async fn a_review_the_api_does_not_answer_in_time_ends_the_handshake_with_the_verb_unknown() {
    let (fake, driver) = refusing(&["get"], 403, &format!("pods \"{KEEPER}\" is forbidden: "));
    fake.fault(Fault::Delay {
        method: Method::POST,
        path: ACCESS_REVIEWS.to_owned(),
        by: Duration::from_mins(1),
    });
    let started = Instant::now();
    let error = handshake_error(&fake, &driver).await;
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "the review held the handshake {:?}",
        started.elapsed()
    );
    assert_eq!(
        error.message,
        format!(
            "{BARE} (the refused verb is unknown: the SelfSubjectAccessReview did not answer \
             within 5 s)"
        )
    );
}

/// What already names its reason asks nothing more: a 403 with the API's reason after "is
/// forbidden:" and a 401 are the API's answer alone, with no access review.
#[tokio::test]
async fn a_refusal_that_gives_its_reason_asks_no_access_review() {
    for (status, message) in [(403, REASONED), (401, "Unauthorized")] {
        let (fake, driver) = refusing(&["get"], status, message);
        let error = handshake_error(&fake, &driver).await;
        assert_eq!(
            error.message,
            format!("the cluster API: exec answered {status}: {message}")
        );
        assert_eq!(reviews(&fake), 0, "{:#?}", fake.requests());
    }
}

/// The line the namespace lane reads: the probe's refusal under a Role without `pods/exec`
/// `create` is `kubernetes.executor-handshake-failed` with the API's bare answer and the verb
/// the review denies.
#[tokio::test]
async fn the_probe_refused_a_bare_exec_names_the_denied_verb_on_its_refusal() {
    let fake = passing_fake();
    let clock = ManualClock::new();
    let (driver, _) = driver(&fake, &clock);
    driver.open().await.expect("the driver opens");
    fake.set_rules(rules_with_exec(&["get"]));
    fake.refuse_exec(403, &format!("pods \"{KEEPER}\" is forbidden: "));
    let refused = tokio::time::timeout(AT_ONCE, driver.probe())
        .await
        .expect("the probe ends at once")
        .expect_err("the handshake is refused");
    assert_eq!(
        refused.code, EXECUTOR_HANDSHAKE_FAILED,
        "{}",
        refused.message
    );
    assert_eq!(
        refused.message,
        format!("{BARE} (a SelfSubjectAccessReview denies pods/exec create)")
    );
}
