//! Design 34 § 10.3's first observations, O1 to O4, O6 to O8 and O10, against a kind cluster with
//! busybox and `b10x-substrate-kubernetes-client` (§ 12 U5). Each runs once.
//!
//! What the driver would do is done with the driver's own credential, a token of the
//! `ServiceAccount` bound to exactly ADR 0047 decision 3's Role; only provisioning, the unlabelled
//! namespace's dry run and O3's control Pod use the admin credential.

use std::collections::BTreeSet;
use std::io::ErrorKind;
use std::net::{Ipv4Addr, SocketAddr, TcpListener};
use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, anyhow, bail, ensure};
use serde_json::{Value, json};
use substrate_kubernetes_client::{Api, ApiError, ListForm, Method};

use crate::chart_cluster::output;
use crate::driver_rules;
use crate::namespace_lane_render::{self as render, PodShape, Probe};

/// What one observation came to. A failure to run is an `Err`, never one of these.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Held(String),
    NotHeld(String),
}

/// How long a Pod may take to start or to end, image pull included.
const POD_BOUND: Duration = Duration::from_mins(3);
/// How long O1 waits for the garbage collector, and then for the claim.
const COLLECTION_BOUND: Duration = Duration::from_mins(1);
/// How long a deleted object may take to go.
const GONE_BOUND: Duration = Duration::from_mins(2);
/// The bytes O10 sends through `pods/exec`: more than one 64 KiB input message.
const EXEC_BYTES: usize = 200_000;

/// The default grants O8 expects beside decision 3's table, as `(verb, group, resource)`: the
/// driver's own `DEFAULT_RESOURCE_RULES` (`crates/substrate-kubernetes/src/rules.rs`), the one
/// source both the startup review and this observation read.
pub fn default_resource_rules() -> BTreeSet<(String, String, String)> {
    driver_rules::DEFAULT_RESOURCE_RULES
        .iter()
        .flat_map(|(group, resource, verbs)| {
            verbs.iter().map(move |verb| {
                (
                    (*verb).to_owned(),
                    (*group).to_owned(),
                    (*resource).to_owned(),
                )
            })
        })
        .collect()
}

/// The default non-resource grants O8 expects, as `(verb, url)`: the driver's own
/// `DEFAULT_NON_RESOURCE_RULES`.
pub fn default_non_resource_rules() -> BTreeSet<(String, String)> {
    driver_rules::DEFAULT_NON_RESOURCE_RULES
        .iter()
        .flat_map(|(verb, urls)| {
            urls.iter()
                .map(move |url| ((*verb).to_owned(), (*url).to_owned()))
        })
        .collect()
}

/// The cluster-scoped collections whose names provisioning records.
pub const INVENTORY: [(&str, &str); 6] = [
    ("/api/v1/namespaces", "NamespaceList"),
    ("/api/v1/persistentvolumes", "PersistentVolumeList"),
    (
        "/apis/rbac.authorization.k8s.io/v1/clusterroles",
        "ClusterRoleList",
    ),
    (
        "/apis/rbac.authorization.k8s.io/v1/clusterrolebindings",
        "ClusterRoleBindingList",
    ),
    ("/apis/storage.k8s.io/v1/storageclasses", "StorageClassList"),
    (
        "/apis/apiextensions.k8s.io/v1/customresourcedefinitions",
        "CustomResourceDefinitionList",
    ),
];

/// The kind network's IPv4 gateway: the host, as Pods reach it.
///
/// # Errors
///
/// `docker network inspect kind` fails or names no IPv4 gateway.
pub fn kind_gateway(docker: &Path, kubeconfig: &Path, stop: &AtomicBool) -> Result<Ipv4Addr> {
    let mut command = Command::new(docker);
    command.env("KUBECONFIG", kubeconfig).args([
        "network",
        "inspect",
        "kind",
        "--format",
        "{{json .IPAM.Config}}",
    ]);
    let config: Value = serde_json::from_str(output(&mut command, stop)?.trim())?;
    config
        .as_array()
        .into_iter()
        .flatten()
        .find_map(|entry| entry["Gateway"].as_str()?.parse::<Ipv4Addr>().ok())
        .ok_or_else(|| anyhow!("the kind network has no IPv4 gateway: {config}"))
}

/// A TCP listener at the kind gateway that counts the connections it accepts and closes each.
struct Counting {
    address: SocketAddr,
    count: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

impl Counting {
    fn bind(gateway: Ipv4Addr) -> Result<Self> {
        let listener = TcpListener::bind((gateway, 0))
            .with_context(|| format!("binding a listener at the kind gateway {gateway}"))?;
        listener.set_nonblocking(true)?;
        let address = listener.local_addr()?;
        let count = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let (counted, stopped) = (Arc::clone(&count), Arc::clone(&stop));
        let thread = thread::spawn(move || {
            while !stopped.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        counted.fetch_add(1, Ordering::SeqCst);
                        drop(stream);
                    }
                    Err(error) if error.kind() == ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(20));
                    }
                    Err(_) => thread::sleep(Duration::from_millis(20)),
                }
            }
        });
        Ok(Self {
            address,
            count,
            stop,
            thread: Some(thread),
        })
    }

    fn count(&self) -> usize {
        self.count.load(Ordering::SeqCst)
    }
}

impl Drop for Counting {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// One run's cluster, credentials and names.
pub struct Lane<'a> {
    pub admin: &'a Api,
    pub namespace: &'a str,
    pub unlabelled: &'a str,
    pub deployment: &'a str,
    pub image: &'a str,
    pub gateway: Ipv4Addr,
    pub stop: &'a AtomicBool,
}

fn answered(error: &ApiError) -> String {
    error.to_string()
}

impl Lane<'_> {
    async fn pause(&self) -> Result<()> {
        ensure!(!self.stop.load(Ordering::SeqCst), "interrupted by a signal");
        tokio::time::sleep(Duration::from_secs(1)).await;
        Ok(())
    }

    fn probe() -> ProbeNames {
        ProbeNames {
            nonce: render::nonce(ulid::Ulid::generate().0),
        }
    }

    fn shape<'b>(&'b self, names: &'b ProbeNames, namespace: &'b str) -> Probe<'b> {
        Probe {
            namespace,
            deployment: self.deployment,
            nonce: &names.nonce,
            image: self.image,
        }
    }

    /// § 10.2 step 1: the cluster-scoped inventory, the admin-side objects, the driver's 1 h
    /// token, written by `write_driver` into the driver's kubeconfig, whose client it returns.
    pub async fn provision(&self, write_driver: impl FnOnce(&str) -> Result<Api>) -> Result<Api> {
        let mut inventory = Vec::new();
        for (path, kind) in INVENTORY {
            let listed = self
                .admin
                .list(path, kind, ListForm::Metadata, 500, 20)
                .await
                .map_err(|error| anyhow!("{kind}: {}", answered(&error)))?;
            inventory.push(format!("{} {kind}", listed.items.len()));
        }
        println!(
            "namespace lane: cluster-scoped inventory before provisioning: {}",
            inventory.join(", ")
        );
        for object in render::provisioning(self.namespace, self.unlabelled) {
            let kind = object["kind"].as_str().unwrap_or_default();
            let collection = render::collection(kind, self.namespace)
                .ok_or_else(|| anyhow!("no collection for {kind}"))?;
            self.admin
                .create(&collection, &object)
                .await
                .map_err(|error| anyhow!("creating the {kind}: {}", answered(&error)))?;
        }
        let request = json!({
            "apiVersion": "authentication.k8s.io/v1",
            "kind": "TokenRequest",
            "spec": {"expirationSeconds": 3600},
        });
        let path = format!(
            "/api/v1/namespaces/{}/serviceaccounts/{}/token",
            self.namespace,
            render::DRIVER
        );
        let answer = self
            .admin
            .create(&path, &request)
            .await
            .map_err(|error| anyhow!("requesting the driver's token: {}", answered(&error)))?;
        let token = answer["status"]["token"]
            .as_str()
            .filter(|token| !token.is_empty())
            .ok_or_else(|| anyhow!("the token request answered no token"))?;
        let driver = write_driver(token)?;
        let quota = format!(
            "/api/v1/namespaces/{}/resourcequotas/{}",
            self.namespace,
            render::QUOTA
        );
        let deadline = Instant::now() + Duration::from_mins(1);
        loop {
            let read = self
                .admin
                .get(&quota)
                .await
                .map_err(|error| anyhow!("reading the quota: {}", answered(&error)))?;
            if read.is_some_and(|quota| {
                quota["status"]["hard"]["count/persistentvolumeclaims"].is_string()
            }) {
                break;
            }
            ensure!(
                Instant::now() < deadline,
                "the quota's status was not computed within 60 s"
            );
            self.pause().await?;
        }
        println!(
            "namespace lane: provisioned namespaces {} (restricted) and {}, ServiceAccount, Role, \
             RoleBinding, {} and quota {}",
            self.namespace,
            self.unlabelled,
            render::DEFAULT_DENY,
            render::QUOTA
        );
        Ok(driver)
    }

    /// Every observation, in an order that keeps the quota free for O7.
    pub async fn observe_all(&self, driver: &Api) -> Vec<(String, Result<Outcome, String>)> {
        let mut outcomes = Vec::new();
        let mut record = |name: &str, result: Result<Outcome>| {
            let result = result.map_err(|error| format!("{error:#}"));
            match &result {
                Ok(Outcome::Held(evidence)) => println!("namespace lane: {name}: held: {evidence}"),
                Ok(Outcome::NotHeld(evidence)) => {
                    println!("namespace lane: {name}: did not hold: {evidence}");
                }
                Err(error) => println!("namespace lane: {name}: did not run: {error}"),
            }
            outcomes.push((name.to_owned(), result));
        };
        record("O8", self.rules(driver).await);
        record("O2", self.dry_run(driver).await);
        record("O7", self.quota(driver).await);
        record("O1", self.collection(driver).await);
        record("O4", self.shared_claim(driver).await);
        record("O10", self.exec(driver).await);
        record("O6", self.cgroup(driver).await);
        record("O3", self.egress(driver).await);
        outcomes
    }

    /// Deletes the run's two namespaces (given-kubeconfig mode); each deletion that failed, which
    /// withholds the record.
    pub async fn remove_namespaces(&self) -> Vec<String> {
        let mut failures = Vec::new();
        for namespace in [self.namespace, self.unlabelled] {
            let path = format!("/api/v1/namespaces/{namespace}");
            if let Err(error) = self.admin.delete(&path, 0).await {
                let failure = format!("deleting namespace {namespace}: {error}");
                eprintln!("namespace lane: {failure}");
                failures.push(failure);
            }
        }
        failures
    }

    async fn wait_pod(
        &self,
        api: &Api,
        namespace: &str,
        name: &str,
        what: &str,
        until: impl Fn(&Value) -> bool,
    ) -> Result<Value> {
        let path = format!("/api/v1/namespaces/{namespace}/pods/{name}");
        let deadline = Instant::now() + POD_BOUND;
        loop {
            let pod = api
                .get(&path)
                .await
                .map_err(|error| anyhow!("reading Pod {name}: {}", answered(&error)))?
                .ok_or_else(|| anyhow!("Pod {name} is gone"))?;
            if until(&pod) {
                return Ok(pod);
            }
            if exit_code(&pod).is_some() {
                bail!(
                    "Pod {name} ended before it was {what}: {}",
                    pod_summary(&pod)
                );
            }
            if Instant::now() >= deadline {
                bail!(
                    "Pod {name} was not {what} within {} s: {}",
                    POD_BOUND.as_secs(),
                    pod_summary(&pod)
                );
            }
            self.pause().await?;
        }
    }

    async fn gone(&self, api: &Api, path: &str, bound: Duration) -> Result<Option<Duration>> {
        let started = Instant::now();
        loop {
            match api.get(path).await {
                Ok(None) => return Ok(Some(started.elapsed())),
                Ok(Some(_)) => {}
                Err(error) => bail!("reading {path}: {}", answered(&error)),
            }
            if started.elapsed() >= bound {
                return Ok(None);
            }
            self.pause().await?;
        }
    }

    async fn remove(&self, api: &Api, path: &str) {
        let _ = api.delete(path, 1).await;
        if !matches!(self.gone(api, path, GONE_BOUND).await, Ok(Some(_))) {
            eprintln!(
                "namespace lane: {path} was not gone within {} s",
                GONE_BOUND.as_secs()
            );
        }
    }

    async fn log(&self, api: &Api, name: &str, container: &str) -> Result<String> {
        let (status, body) = api
            .log(name, container, 65_536)
            .await
            .map_err(|error| anyhow!("reading {name}'s log: {}", answered(&error)))?;
        ensure!(status == 200, "reading {name}'s log answered {status}");
        Ok(String::from_utf8_lossy(&body).into_owned())
    }

    /// O8: the driver's rules review is decision 3's table plus the fixed default set.
    async fn rules(&self, driver: &Api) -> Result<Outcome> {
        let status = driver
            .rules_review()
            .await
            .map_err(|error| anyhow!("rules review: {}", answered(&error)))?;
        rules_outcome(&status)
    }

    /// O2: a `privileged` Pod's dry run is refused by Pod Security in the `restricted` namespace
    /// and admitted in the unlabelled one.
    async fn dry_run(&self, driver: &Api) -> Result<Outcome> {
        let names = Self::probe();
        let shape = PodShape {
            role: "pod",
            script: "true",
            claim: None,
            node: None,
            privileged: true,
        };
        let restricted = self.shape(&names, self.namespace).pod(&shape);
        let (status, body) = driver
            .call(
                Method::POST,
                &format!("{}?dryRun=All", driver.pods()),
                Some(&restricted),
                substrate_kubernetes_client::MAX_API_BODY_BYTES,
            )
            .await
            .map_err(|error| anyhow!("restricted dry run: {error}"))?;
        let refusal = driver.status_message(&body);
        let unlabelled = self.shape(&names, self.unlabelled).pod(&shape);
        let (admitted, answer) = self
            .admin
            .call(
                Method::POST,
                &format!("/api/v1/namespaces/{}/pods?dryRun=All", self.unlabelled),
                Some(&unlabelled),
                substrate_kubernetes_client::MAX_API_BODY_BYTES,
            )
            .await
            .map_err(|error| anyhow!("unlabelled dry run: {error}"))?;
        let stored = self
            .admin
            .get(&format!(
                "/api/v1/namespaces/{}/pods/{}",
                self.unlabelled,
                names.name("pod")
            ))
            .await
            .map_err(|error| anyhow!("reading the dry-run Pod: {}", answered(&error)))?;
        let evidence = format!(
            "restricted: {status} {refusal:?}; unlabelled: {admitted}{}; stored afterwards: {}",
            if (200..=202).contains(&admitted) {
                String::new()
            } else {
                format!(" {:?}", self.admin.status_message(&answer))
            },
            stored.is_some()
        );
        if status == 403
            && refusal.contains("PodSecurity")
            && (200..=202).contains(&admitted)
            && stored.is_none()
        {
            Ok(Outcome::Held(evidence))
        } else {
            Ok(Outcome::NotHeld(evidence))
        }
    }

    /// O7: a create over the `ResourceQuota` answers 403 with a message starting `exceeded quota:`.
    async fn quota(&self, driver: &Api) -> Result<Outcome> {
        let mut created = Vec::new();
        let mut outcome = None;
        for _ in 0..=render::QUOTA_CLAIMS {
            let names = Self::probe();
            let claim = self.shape(&names, self.namespace).claim();
            let (status, body) = driver
                .call(
                    Method::POST,
                    &driver.claims(),
                    Some(&claim),
                    substrate_kubernetes_client::MAX_API_BODY_BYTES,
                )
                .await
                .map_err(|error| anyhow!("creating a claim: {error}"))?;
            match status {
                200..=202 => created.push(names.name("pvc")),
                403 => {
                    let message = driver.status_message(&body);
                    let reason = serde_json::from_slice::<Value>(&body)
                        .map(|status| status["reason"].clone())
                        .unwrap_or_default();
                    let evidence = format!(
                        "claim {} answered 403, reason {reason}, message {message:?}, after {} \
                         admitted",
                        created.len() + 1,
                        created.len()
                    );
                    outcome = Some(if message.starts_with("exceeded quota:") {
                        Outcome::Held(evidence)
                    } else {
                        Outcome::NotHeld(evidence)
                    });
                    break;
                }
                other => {
                    outcome = Some(Outcome::NotHeld(format!(
                        "claim {} answered {other} {:?}",
                        created.len() + 1,
                        driver.status_message(&body)
                    )));
                    break;
                }
            }
        }
        for name in &created {
            self.remove(driver, &format!("{}/{name}", driver.claims()))
                .await;
        }
        Ok(outcome.unwrap_or_else(|| {
            Outcome::NotHeld(format!(
                "{} claims were admitted under a quota of {}",
                created.len(),
                render::QUOTA_CLAIMS
            ))
        }))
    }

    /// O1: deleting a claim a running Pod owner-references and mounts: background collection
    /// removes the Pod, and the claim goes once no Pod uses it.
    async fn collection(&self, driver: &Api) -> Result<Outcome> {
        let names = Self::probe();
        let probe = self.shape(&names, self.namespace);
        let claim = driver
            .create(&driver.claims(), &probe.claim())
            .await
            .map_err(|error| anyhow!("creating the claim: {}", answered(&error)))?;
        let uid = claim["metadata"]["uid"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        let claim_name = names.name("pvc");
        let claim_path = format!("{}/{claim_name}", driver.claims());
        let pod_name = names.name("keeper");
        let pod_path = format!("{}/{pod_name}", driver.pods());
        let result = async {
            driver
                .create(
                    &driver.pods(),
                    &probe.pod(&PodShape {
                        role: "keeper",
                        script: render::KEEPER_SCRIPT,
                        claim: Some((&claim_name, &uid)),
                        node: None,
                        privileged: false,
                    }),
                )
                .await
                .map_err(|error| anyhow!("creating the Pod: {}", answered(&error)))?;
            self.wait_pod(driver, self.namespace, &pod_name, "running", running)
                .await?;
            driver
                .delete(&claim_path, 0)
                .await
                .map_err(|error| anyhow!("deleting the claim: {}", answered(&error)))?;
            let pod_gone = self.gone(driver, &pod_path, COLLECTION_BOUND).await?;
            let Some(pod_after) = pod_gone else {
                let pod = driver
                    .get(&pod_path)
                    .await
                    .ok()
                    .flatten()
                    .unwrap_or_default();
                let claim = driver
                    .get(&claim_path)
                    .await
                    .ok()
                    .flatten()
                    .unwrap_or_default();
                return Ok(Outcome::NotHeld(format!(
                    "{} s after the claim's background deletion the Pod is still present ({}, \
                     deletionTimestamp {}) and the claim is held (deletionTimestamp {}, \
                     finalizers {})",
                    COLLECTION_BOUND.as_secs(),
                    pod_summary(&pod),
                    pod["metadata"]["deletionTimestamp"],
                    claim["metadata"]["deletionTimestamp"],
                    claim["metadata"]["finalizers"]
                )));
            };
            let claim_gone = self.gone(driver, &claim_path, COLLECTION_BOUND).await?;
            Ok(match claim_gone {
                Some(claim_after) => Outcome::Held(format!(
                    "the garbage collector removed the Pod {} s after the claim's background \
                     deletion, and the claim went {} s later",
                    pod_after.as_secs(),
                    claim_after.as_secs()
                )),
                None => Outcome::NotHeld(format!(
                    "the Pod went {} s after the claim's deletion, but the claim was still present \
                     {} s later",
                    pod_after.as_secs(),
                    COLLECTION_BOUND.as_secs()
                )),
            })
        }
        .await;
        self.remove(driver, &pod_path).await;
        self.remove(driver, &claim_path).await;
        result
    }

    /// O4: two Pods on one node mount one `ReadWriteOnce` claim of `standard`.
    async fn shared_claim(&self, driver: &Api) -> Result<Outcome> {
        let names = Self::probe();
        let probe = self.shape(&names, self.namespace);
        let claim = driver
            .create(&driver.claims(), &probe.claim())
            .await
            .map_err(|error| anyhow!("creating the claim: {}", answered(&error)))?;
        let uid = claim["metadata"]["uid"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        let claim_name = names.name("pvc");
        let keeper = names.name("keeper");
        let second = names.name("pod");
        let result = async {
            driver
                .create(
                    &driver.pods(),
                    &probe.pod(&PodShape {
                        role: "keeper",
                        script: render::KEEPER_SCRIPT,
                        claim: Some((&claim_name, &uid)),
                        node: None,
                        privileged: false,
                    }),
                )
                .await
                .map_err(|error| anyhow!("creating the keeper: {}", answered(&error)))?;
            let running_keeper = self
                .wait_pod(driver, self.namespace, &keeper, "running", running)
                .await?;
            let node = running_keeper["spec"]["nodeName"]
                .as_str()
                .ok_or_else(|| anyhow!("the running keeper names no node"))?
                .to_owned();
            driver
                .create(
                    &driver.pods(),
                    &probe.pod(&PodShape {
                        role: "pod",
                        script: render::SECOND_SCRIPT,
                        claim: Some((&claim_name, &uid)),
                        node: Some(&node),
                        privileged: false,
                    }),
                )
                .await
                .map_err(|error| anyhow!("creating the second Pod: {}", answered(&error)))?;
            let ended = self
                .wait_pod(driver, self.namespace, &second, "ended", |pod| {
                    exit_code(pod).is_some()
                })
                .await;
            let ended = match ended {
                Ok(pod) => pod,
                Err(error) => return Ok(Outcome::NotHeld(format!("{error:#}"))),
            };
            let log = self.log(driver, &second, render::RUN_CONTAINER).await?;
            let evidence = format!(
                "keeper and second Pod on node {node} (second on {}), claim {claim_name}; the \
                 second exited {:?} and read {:?}",
                ended["spec"]["nodeName"],
                exit_code(&ended),
                log.trim()
            );
            Ok(
                if exit_code(&ended) == Some(0)
                    && log.trim() == "keeper"
                    && ended["spec"]["nodeName"] == node.as_str()
                {
                    Outcome::Held(evidence)
                } else {
                    Outcome::NotHeld(evidence)
                },
            )
        }
        .await;
        for pod in [&second, &keeper] {
            self.remove(driver, &format!("{}/{pod}", driver.pods()))
                .await;
        }
        self.remove(driver, &format!("{}/{claim_name}", driver.claims()))
            .await;
        result
    }

    /// O10: `pods/exec` accepts `v5.channel.k8s.io` and its close channel: `cat` echoes every
    /// input byte and ends once the input is closed.
    async fn exec(&self, driver: &Api) -> Result<Outcome> {
        let names = Self::probe();
        let probe = self.shape(&names, self.namespace);
        let name = names.name("pod");
        let result = async {
            driver
                .create(
                    &driver.pods(),
                    &probe.pod(&PodShape {
                        role: "pod",
                        script: "exec sleep 600",
                        claim: None,
                        node: None,
                        privileged: false,
                    }),
                )
                .await
                .map_err(|error| anyhow!("creating the Pod: {}", answered(&error)))?;
            self.wait_pod(driver, self.namespace, &name, "running", running)
                .await?;
            let input: Vec<u8> = (0..EXEC_BYTES)
                .map(|index| u8::try_from(index % 251).unwrap_or_default())
                .collect();
            match echo(driver, &name, &input).await {
                Ok((true, evidence)) => Ok(Outcome::Held(format!(
                    "{} chosen; {evidence}",
                    substrate_kubernetes_client::EXEC_PROTOCOL
                ))),
                Ok((false, evidence)) => Ok(Outcome::NotHeld(evidence)),
                Err(error) => {
                    let admin = match echo(self.admin, &name, &input).await {
                        Ok((_, evidence)) => evidence,
                        Err(error) => format!("failed too: {error}"),
                    };
                    Ok(Outcome::NotHeld(format!(
                        "the driver's exec over {} failed: {error}; the same exec with the \
                         admin's credentials: {admin}",
                        substrate_kubernetes_client::EXEC_PROTOCOL
                    )))
                }
            }
        }
        .await;
        self.remove(driver, &format!("{}/{name}", driver.pods()))
            .await;
        result
    }

    /// O6: what `pids.max` (and `memory.max`, `cpu.max`) read inside a container under
    /// `podPidsLimit: 1024`.
    async fn cgroup(&self, driver: &Api) -> Result<Outcome> {
        let names = Self::probe();
        let probe = self.shape(&names, self.namespace);
        let name = names.name("pod");
        let result = async {
            driver
                .create(
                    &driver.pods(),
                    &probe.pod(&PodShape {
                        role: "pod",
                        script: render::CGROUP_SCRIPT,
                        claim: None,
                        node: None,
                        privileged: false,
                    }),
                )
                .await
                .map_err(|error| anyhow!("creating the Pod: {}", answered(&error)))?;
            let ended = self
                .wait_pod(driver, self.namespace, &name, "ended", |pod| {
                    exit_code(pod).is_some()
                })
                .await?;
            let log = self.log(driver, &name, render::RUN_CONTAINER).await?;
            ensure!(
                exit_code(&ended) == Some(0),
                "the Pod exited {:?}: {log:?}",
                exit_code(&ended)
            );
            let read: Vec<&str> = log.lines().collect();
            Ok(Outcome::Held(format!(
                "inside the container under podPidsLimit 1024 and limits {} / {}: {}",
                render::MEMORY,
                render::CPU,
                read.join(", ")
            )))
        }
        .await;
        self.remove(driver, &format!("{}/{name}", driver.pods()))
            .await;
        result
    }

    /// O3: kind's default plugin enforces the default-deny policy: the three probe connections
    /// fail within 3 s, while the same Pod in the unlabelled namespace, under no policy, makes
    /// all three.
    async fn egress(&self, driver: &Api) -> Result<Outcome> {
        let listener = Counting::bind(self.gateway)?;
        let script = render::egress_script(&self.gateway.to_string(), listener.address.port());
        let names = Self::probe();
        let name = names.name("pod");
        let shape = PodShape {
            role: "pod",
            script: &script,
            claim: None,
            node: None,
            privileged: false,
        };
        let probe = self.shape(&names, self.namespace).pod(&shape);
        let control = self.shape(&names, self.unlabelled).pod(&shape);
        let control_pods = format!("/api/v1/namespaces/{}/pods", self.unlabelled);
        let result = async {
            driver
                .create(&driver.pods(), &probe)
                .await
                .map_err(|error| anyhow!("creating the probe Pod: {}", answered(&error)))?;
            let ended = |pod: &Value| exit_code(pod).is_some();
            self.wait_pod(driver, self.namespace, &name, "ended", ended)
                .await?;
            let probe_log = self.log(driver, &name, render::RUN_CONTAINER).await?;
            let after_probe = listener.count();
            self.admin
                .create(&control_pods, &control)
                .await
                .map_err(|error| anyhow!("creating the control Pod: {}", answered(&error)))?;
            self.wait_pod(self.admin, self.unlabelled, &name, "ended", ended)
                .await?;
            let (status, body) = self
                .admin
                .call(
                    Method::GET,
                    &format!(
                        "{control_pods}/{name}/log?container={}",
                        render::RUN_CONTAINER
                    ),
                    None,
                    65_536,
                )
                .await
                .map_err(|error| anyhow!("reading the control log: {error}"))?;
            ensure!(status == 200, "reading the control log answered {status}");
            let control_log = String::from_utf8_lossy(&body).into_owned();
            let after_control = listener.count();
            let probe_statuses = statuses(&probe_log)?;
            let control_statuses = statuses(&control_log)?;
            let evidence = format!(
                "probe under {}: {probe_statuses:?}, outside listener saw {after_probe}; control \
                 under no policy: {control_statuses:?}, listener saw {} more",
                render::DEFAULT_DENY,
                after_control - after_probe
            );
            ensure!(
                control_statuses.iter().all(|(_, status)| *status == 0)
                    && after_control > after_probe,
                "inconclusive: the control Pod, under no policy, did not make all three \
                 connections, so a failure under the policy proves nothing: {evidence}"
            );
            Ok(
                if probe_statuses.iter().all(|(_, status)| *status != 0) && after_probe == 0 {
                    Outcome::Held(evidence)
                } else {
                    Outcome::NotHeld(evidence)
                },
            )
        }
        .await;
        self.remove(driver, &format!("{}/{name}", driver.pods()))
            .await;
        self.remove(self.admin, &format!("{control_pods}/{name}"))
            .await;
        result
    }
}

/// The nonce of one probe.
struct ProbeNames {
    nonce: String,
}

impl ProbeNames {
    fn name(&self, role: &str) -> String {
        format!("sbx-pr-{}-{role}", self.nonce)
    }
}

/// O8's verdict on a `SelfSubjectRulesReview` status: held when the rules are exactly decision 3's
/// table plus the fixed default set and the review is complete. An incomplete review lists only
/// what the authorizer could evaluate, so it proves nothing about the whole rule set: a grant
/// beyond the table still means O8 did not hold, and anything else is inconclusive, an error,
/// which leaves the run without a record.
///
/// # Errors
///
/// The review is incomplete, or its `incomplete` member is not a boolean, and it shows no grant
/// beyond the table.
pub fn rules_outcome(status: &Value) -> Result<Outcome> {
    let mut resources = BTreeSet::new();
    for rule in status["resourceRules"].as_array().into_iter().flatten() {
        for verb in strings(&rule["verbs"]) {
            for group in strings(&rule["apiGroups"]) {
                for resource in strings(&rule["resources"]) {
                    let names = strings(&rule["resourceNames"]);
                    if names.is_empty() {
                        resources.insert((
                            verb.clone(),
                            group.clone(),
                            resource.clone(),
                            String::new(),
                        ));
                    }
                    for name in names {
                        resources.insert((verb.clone(), group.clone(), resource.clone(), name));
                    }
                }
            }
        }
    }
    let mut urls = BTreeSet::new();
    for rule in status["nonResourceRules"].as_array().into_iter().flatten() {
        for verb in strings(&rule["verbs"]) {
            for url in strings(&rule["nonResourceURLs"]) {
                urls.insert((verb.clone(), url));
            }
        }
    }
    let mut expected = BTreeSet::new();
    for grant in render::role_grants() {
        for verb in &grant.verbs {
            expected.insert((
                verb.clone(),
                grant.group.clone(),
                grant.resource.clone(),
                String::new(),
            ));
        }
    }
    for (verb, group, resource) in default_resource_rules() {
        expected.insert((verb, group, resource, String::new()));
    }
    let expected_urls = default_non_resource_rules();
    let extra: Vec<_> = resources.difference(&expected).collect();
    let missing: Vec<_> = expected.difference(&resources).collect();
    let extra_urls: Vec<_> = urls.difference(&expected_urls).collect();
    let missing_urls: Vec<_> = expected_urls.difference(&urls).collect();
    let incomplete = match &status["incomplete"] {
        Value::Null | Value::Bool(false) => false,
        Value::Bool(true) => true,
        other => bail!("O8 is inconclusive: the rules review's incomplete member is {other}"),
    };
    let evidence = format!(
        "{} resource rules and {} non-resource rules reviewed, incomplete {incomplete}; beyond \
         decision 3 and the default set: {extra:?} {extra_urls:?}; missing: {missing:?} \
         {missing_urls:?}",
        resources.len(),
        urls.len()
    );
    if !(extra.is_empty() && extra_urls.is_empty()) {
        return Ok(Outcome::NotHeld(evidence));
    }
    if incomplete {
        bail!(
            "O8 is inconclusive: the rules review is incomplete ({}), so it proves nothing about \
             the whole rule set: {evidence}",
            status["evaluationError"]
                .as_str()
                .unwrap_or("no evaluation error given")
        );
    }
    if missing.is_empty() && missing_urls.is_empty() {
        Ok(Outcome::Held(evidence))
    } else {
        Ok(Outcome::NotHeld(evidence))
    }
}

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| item.as_str().map(str::to_owned))
        .collect()
}

/// O10's exchange: `cat` over `pods/exec` echoes `input`; whether it came back whole with exit 0,
/// and the evidence line.
async fn echo(api: &Api, pod: &str, input: &[u8]) -> Result<(bool, String), ApiError> {
    let output = api
        .exec(pod, render::RUN_CONTAINER, &["cat"], input, EXEC_BYTES * 2)
        .await?;
    let equal = output.stdout == input;
    let evidence = format!(
        "{} sent, {} echoed (equal {equal}), stderr {} bytes, status {}",
        input.len(),
        output.stdout.len(),
        output.stderr.len(),
        output.status.clone().unwrap_or_default()
    );
    Ok((equal && output.exit_code() == Some(0), evidence))
}

fn running(pod: &Value) -> bool {
    pod["status"]["containerStatuses"][0]["state"]["running"].is_object()
}

fn exit_code(pod: &Value) -> Option<i64> {
    pod["status"]["containerStatuses"][0]["state"]["terminated"]["exitCode"].as_i64()
}

fn pod_summary(pod: &Value) -> String {
    format!(
        "phase {}, node {}, container state {}, conditions {}",
        pod["status"]["phase"],
        pod["spec"]["nodeName"],
        pod["status"]["containerStatuses"][0]["state"],
        pod["status"]["conditions"]
    )
}

/// The `<name> <status>` lines of the egress script, exactly `api`, `dns` and `outside`.
fn statuses(log: &str) -> Result<Vec<(String, i64)>> {
    let read: Vec<(String, i64)> = log
        .lines()
        .filter_map(|line| {
            let (name, status) = line.trim().split_once(' ')?;
            Some((name.to_owned(), status.parse().ok()?))
        })
        .collect();
    let named: Vec<&str> = read.iter().map(|(name, _)| name.as_str()).collect();
    ensure!(
        named == ["api", "dns", "outside"],
        "the egress script wrote {log:?}, not one status each for api, dns and outside"
    );
    Ok(read)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_egress_statuses_are_read_exactly() {
        assert_eq!(
            statuses("api 143\ndns 1\noutside 143\n").expect("three"),
            [
                ("api".to_owned(), 143),
                ("dns".to_owned(), 1),
                ("outside".to_owned(), 143)
            ]
        );
        assert!(statuses("api 0\ndns 0\n").is_err());
        assert!(statuses("nc: bad option\napi 0\ndns 0\noutside 0\n").is_ok());
    }

    /// A review status holding exactly decision 3's table and the default set.
    fn exact_review() -> Value {
        let mut resource_rules: Vec<Value> = render::role_grants()
            .iter()
            .map(|grant| {
                json!({"verbs": grant.verbs, "apiGroups": [grant.group], "resources": [grant.resource]})
            })
            .collect();
        for (verb, group, resource) in default_resource_rules() {
            resource_rules
                .push(json!({"verbs": [verb], "apiGroups": [group], "resources": [resource]}));
        }
        let urls: Vec<String> = default_non_resource_rules()
            .into_iter()
            .map(|(_, url)| url)
            .collect();
        json!({
            "resourceRules": resource_rules,
            "nonResourceRules": [{"verbs": ["get"], "nonResourceURLs": urls}],
            "incomplete": false,
        })
    }

    #[test]
    fn o8_holds_only_on_a_complete_review_and_an_incomplete_one_is_never_held() {
        let exact = exact_review();
        assert!(matches!(rules_outcome(&exact), Ok(Outcome::Held(_))));
        let mut incomplete = exact.clone();
        incomplete["incomplete"] = json!(true);
        incomplete["evaluationError"] = json!("webhook authorizer unreachable");
        let inconclusive = rules_outcome(&incomplete).expect_err("incomplete is inconclusive");
        assert!(
            format!("{inconclusive:#}").contains("incomplete"),
            "{inconclusive:#}"
        );
        let mut odd = exact.clone();
        odd["incomplete"] = json!("yes");
        assert!(rules_outcome(&odd).is_err());
        let mut widened = incomplete.clone();
        widened["resourceRules"]
            .as_array_mut()
            .expect("rules")
            .push(json!({"verbs": ["create"], "apiGroups": [""], "resources": ["secrets"]}));
        assert!(matches!(rules_outcome(&widened), Ok(Outcome::NotHeld(_))));
        let mut short = exact;
        short["nonResourceRules"] = json!([]);
        assert!(matches!(rules_outcome(&short), Ok(Outcome::NotHeld(_))));
    }
}
