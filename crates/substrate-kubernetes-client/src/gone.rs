//! Whether the container of a Pod a read found gone is known to have stopped
//! (design 28 § *A gone Pod*, `story:pod-force-deleted-keeps-its-policy`).
//!
//! A forced deletion, the Pod garbage collector's after a node stopped answering or an operator's,
//! removes a Pod without its kubelet, so a 404 alone says nothing of the container. Only a read
//! that showed the container terminated, a watch from the last `resourceVersion` read that replays
//! a terminated container, or a deletion that leaves the Pod unbound to any node is evidence.

use std::time::Duration;

use serde_json::Value;

use crate::RUN_CONTAINER;
use crate::api::{Api, WatchError, bounded, pod_name};

/// How long a watch of one Pod may take to show how its container ended, once a read found the Pod
/// gone. The API server replays the Pod's history from the `resourceVersion` last read at once; a
/// watch that shows nothing within this decides nothing.
pub const WATCH_LIMIT: Duration = Duration::from_secs(5);

/// How long [`Api::pod_history`] reads a replay that asks for bookmarks: the watch's own
/// `timeoutSeconds`, [`WATCH_LIMIT`], and 3 seconds more. An API server sends such a watch one
/// `BOOKMARK` shortly before its `timeoutSeconds` ends (observed on kind v1.37.0, 2026-11-01), so a
/// client bound equal to it could end the read just before the line that shows the replay complete;
/// the server closes the stream at its timeout, and the margin covers only its lateness. The bound
/// is fixed, so a sweep that waits on it stays bounded.
pub const REPLAY_LIMIT: Duration = WATCH_LIMIT.saturating_add(Duration::from_secs(3));

/// What has been read of one run's Pod, for [`pod_end`].
#[derive(Debug, Clone, Default)]
pub struct PodTrace {
    /// The newest `resourceVersion` read for the run: its Pod's, or until a Pod object is read the
    /// version of an object created before it. A watch from there replays the Pod's whole history
    /// while the API server still holds it.
    resource_version: Option<String>,
    /// The Pod's `uid`, from the first object read that carries one; an object with another is a
    /// different Pod and is not noted.
    uid: Option<String>,
    /// An object read showed the [`RUN_CONTAINER`] terminated in a Pod whose own
    /// `spec.restartPolicy` is `Never`. Only the kubelet writes that state, and the container of
    /// such a Pod does not run again; a Pod an admission webhook gave another policy may restart
    /// it, so its terminated state is not noted. A Pod's phase is no such evidence: the Pod garbage
    /// collector marks a Pod `Failed` before it force-deletes it.
    stopped: bool,
}

impl PodTrace {
    /// A trace starting at `version`: the `resourceVersion` of an object created before the Pod.
    pub fn since(version: Option<&str>) -> Self {
        Self {
            resource_version: version.map(str::to_owned),
            ..Self::default()
        }
    }

    /// A trace starting at `before`'s `resourceVersion`, with the Pod its create stored.
    pub fn created(before: &Value, pod: &Value) -> Self {
        let mut trace = Self::since(before["metadata"]["resourceVersion"].as_str());
        trace.note(pod);
        trace
    }

    /// Notes one object read of the run's Pod: a whole Pod or its metadata alone. `false` when the
    /// object is another Pod than the one first read, which is not noted.
    pub fn note(&mut self, pod: &Value) -> bool {
        let uid = pod["metadata"]["uid"].as_str();
        if let (Some(seen), Some(uid)) = (&self.uid, uid)
            && seen != uid
        {
            return false;
        }
        if self.uid.is_none() {
            self.uid = uid.map(str::to_owned);
        }
        if let Some(version) = pod["metadata"]["resourceVersion"].as_str() {
            self.resource_version = Some(version.to_owned());
        }
        if container_state(pod, RUN_CONTAINER)["terminated"].is_object()
            && pod["spec"]["restartPolicy"].as_str() == Some("Never")
        {
            self.stopped = true;
        }
        true
    }

    /// Whether a read showed the container terminated.
    pub fn stopped(&self) -> bool {
        self.stopped
    }

    /// The Pod's `uid`, once a read showed one.
    pub fn uid(&self) -> Option<&str> {
        self.uid.as_deref()
    }

    /// The newest `resourceVersion` read.
    pub fn resource_version(&self) -> Option<&str> {
        self.resource_version.as_deref()
    }
}

/// The `state` of the container `name` in `pod`'s status, or `Null`.
pub fn container_state<'a>(pod: &'a Value, name: &str) -> &'a Value {
    pod["status"]["containerStatuses"]
        .as_array()
        .and_then(|containers| containers.iter().find(|c| c["name"] == name))
        .map_or(&Value::Null, |container| &container["state"])
}

/// Whether `pod`, a whole Pod object, was never bound to a node. A binding sets `spec.nodeName`
/// and nothing unsets it, so the object a deletion leaves without one was never held by a kubelet
/// and no container of it ever ran. A read before the deletion is no such evidence: the Pod may be
/// bound after it.
pub fn never_bound(pod: &Value) -> bool {
    pod["spec"].is_object() && pod["spec"]["nodeName"].as_str().is_none_or(str::is_empty)
}

/// What the API shows of the container of a Pod a read found gone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PodEnd {
    /// The container stopped.
    Stopped,
    /// The API cannot show it stopped, and will not.
    Unproven(String),
    /// The API did not answer; asking again may tell.
    Unanswered(String),
}

/// What [`Api::pod_history`] shows of a Pod no read ever showed an object of.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PodHistory {
    /// The replay from the version read before the Pod ended in a `BOOKMARK` before any event of
    /// the Pod's name: the API server replayed every change since, and no Pod of that name was
    /// ever added.
    NeverCreated,
    /// What the replay's events of the name decided, as [`Api::quoted_watch_event`] decides them,
    /// or why the replay decides nothing.
    End(PodEnd),
}

/// One line of a watch of the Pod `name`: noted in `trace`, and the [`PodEnd`] it decides, if any:
/// a container shown terminated, a deletion that showed none, or an `ERROR` event, whose message
/// is quoted bounded as every API message is.
pub fn watch_event(line: &[u8], name: &str, trace: &mut PodTrace) -> Option<PodEnd> {
    decide(line, name, trace, bounded)
}

/// [`watch_event`], with `quote` turning an `ERROR` event's message into what the reason quotes:
/// a client clears its own credential from it before it bounds it.
fn decide(
    line: &[u8],
    name: &str,
    trace: &mut PodTrace,
    quote: impl Fn(&str) -> String,
) -> Option<PodEnd> {
    if line.trim_ascii().is_empty() {
        return None;
    }
    let Ok(event) = serde_json::from_slice::<Value>(line) else {
        return Some(PodEnd::Unproven(
            "the watch sent something other than JSON".to_owned(),
        ));
    };
    let kind = event["type"].as_str();
    if kind == Some("ERROR") {
        return Some(PodEnd::Unproven(format!(
            "the watch answered an error: {}",
            quote(event["object"]["message"].as_str().unwrap_or_default())
        )));
    }
    let object = &event["object"];
    if !matches!(kind, Some("ADDED" | "MODIFIED" | "DELETED"))
        || object["metadata"]["name"].as_str() != Some(name)
    {
        return None;
    }
    if !trace.note(object) {
        return None;
    }
    if trace.stopped {
        return Some(PodEnd::Stopped);
    }
    if kind != Some("DELETED") {
        return None;
    }
    Some(if never_bound(object) {
        PodEnd::Stopped
    } else {
        PodEnd::Unproven("the Pod was deleted with its container not shown terminated".to_owned())
    })
}

/// Whether the container of the Pod `name`, which a read just found gone, is known to have
/// stopped: a read showed it terminated, or a watch of the Pod from the last `resourceVersion` read
/// replays it terminated (the kubelet writes that state before it removes a Pod it was asked to
/// delete), or the deletion left the Pod unbound to any node ([`never_bound`]). Without a
/// `resourceVersion` to watch from, on a 410, a watch that shows nothing within [`WATCH_LIMIT`] or
/// a deletion of a bound Pod's container not shown terminated, it is unproven.
pub async fn pod_end(api: &Api, name: &str, trace: &PodTrace) -> PodEnd {
    if trace.stopped {
        return PodEnd::Stopped;
    }
    let Some(version) = &trace.resource_version else {
        return PodEnd::Unproven("no resourceVersion of the run was read to watch from".to_owned());
    };
    api.watch_pod(name, trace, version).await
}

impl Api {
    /// [`watch_event`] as this client's own replay decides it: an `ERROR` event's message is
    /// cleared of the bearer token and then bounded, so a driver that replays a gone Pod's watch
    /// itself quotes what this client would.
    pub fn quoted_watch_event(
        &self,
        line: &[u8],
        name: &str,
        trace: &mut PodTrace,
    ) -> Option<PodEnd> {
        decide(line, name, trace, |text| self.quote(text))
    }

    /// What a watch of the Pod `name` from the `resourceVersion` `trace` last read shows of its
    /// container within [`WATCH_LIMIT`]: the API server replays every change since, and the
    /// deletion last. Only a container shown terminated is [`PodEnd::Stopped`]. A `name` that is
    /// not a DNS-1123 subdomain reaches no field selector: it is unproven before any request.
    async fn watch_pod(&self, name: &str, trace: &PodTrace, version: &str) -> PodEnd {
        if let Err(invalid) = pod_name(name) {
            return PodEnd::Unproven(invalid.to_string());
        }
        let version: String = url::form_urlencoded::byte_serialize(version.as_bytes()).collect();
        let path = format!(
            "{}?watch=1&fieldSelector=metadata.name%3D{name}&resourceVersion={version}\
             &timeoutSeconds={}",
            self.pods(),
            WATCH_LIMIT.as_secs()
        );
        let mut trace = trace.clone();
        let end = tokio::time::timeout(WATCH_LIMIT, self.replay(&path, name, &mut trace))
            .await
            .unwrap_or_else(|_| {
                PodEnd::Unproven(format!(
                    "the watch showed no end of the container within {WATCH_LIMIT:?}"
                ))
            });
        match end {
            PodEnd::Stopped => PodEnd::Stopped,
            PodEnd::Unproven(why) => PodEnd::Unproven(self.redact(&why)),
            PodEnd::Unanswered(why) => PodEnd::Unanswered(self.redact(&why)),
        }
    }

    /// Reads the watch at `path` event by event until one decides [`PodEnd`], the stream ends, or
    /// it passes [`crate::MAX_API_BODY_BYTES`].
    async fn replay(&self, path: &str, name: &str, trace: &mut PodTrace) -> PodEnd {
        // The connection goes with the watch, however the read ends.
        let mut watch = match self.watch(path).await {
            Ok(watch) => watch,
            Err(error) => return unopened(error),
        };
        loop {
            match watch.next_line().await {
                Ok(Some(line)) => {
                    if let Some(decided) = self.quoted_watch_event(&line, name, trace) {
                        return decided;
                    }
                }
                Ok(None) => {
                    return PodEnd::Unproven(
                        "the watch ended without showing the Pod's deletion".to_owned(),
                    );
                }
                Err(WatchError::Bound) => {
                    return PodEnd::Unproven("the watch passed its byte bound".to_owned());
                }
                Err(_) => return PodEnd::Unanswered("the watch stream failed".to_owned()),
            }
        }
    }

    /// The replay of the Pod `name` from the `resourceVersion` `trace` last read, for a sweep of a
    /// policy whose run's Pod no read ever showed (`trace` carries no `uid`) and a read just found
    /// gone (`story:pod-orphaned-policy-is-swept`). The watch is [`pod_end`]'s, asking for
    /// bookmarks too (`allowWatchBookmarks=true`, `timeoutSeconds` [`WATCH_LIMIT`]), and is read
    /// for at most [`REPLAY_LIMIT`]. The API server replays every event of the name since that
    /// version first and then sends a `BOOKMARK`, so a `BOOKMARK` before any event of the name ends
    /// a complete replay that shows no Pod of it ever added: [`PodHistory::NeverCreated`]. Events of
    /// the name decide as [`Api::quoted_watch_event`] decides them, and a `BOOKMARK` after events
    /// of the name that decided nothing is unproven. So is a replay that does not end in a
    /// `BOOKMARK`: a 410, a refusal, an `ERROR` event, a line that is not JSON, the byte bound, or
    /// the stream ending or the bound passing first. One that goes unanswered is
    /// [`PodEnd::Unanswered`]. A run whose Pod a read showed is [`pod_end`]'s alone.
    pub async fn pod_history(&self, name: &str, trace: &PodTrace) -> PodHistory {
        if trace.stopped {
            return PodHistory::End(PodEnd::Stopped);
        }
        let Some(version) = &trace.resource_version else {
            return PodHistory::End(PodEnd::Unproven(
                "no resourceVersion of the run was read to watch from".to_owned(),
            ));
        };
        if let Err(invalid) = pod_name(name) {
            return PodHistory::End(PodEnd::Unproven(invalid.to_string()));
        }
        let version: String = url::form_urlencoded::byte_serialize(version.as_bytes()).collect();
        let path = format!(
            "{}?watch=1&fieldSelector=metadata.name%3D{name}&resourceVersion={version}\
             &allowWatchBookmarks=true&timeoutSeconds={}",
            self.pods(),
            WATCH_LIMIT.as_secs()
        );
        let mut trace = trace.clone();
        let history = tokio::time::timeout(REPLAY_LIMIT, self.history(&path, name, &mut trace))
            .await
            .unwrap_or_else(|_| {
                PodHistory::End(PodEnd::Unproven(format!(
                    "the watch sent no bookmark within {REPLAY_LIMIT:?}, so its replay may be \
                     incomplete"
                )))
            });
        match history {
            PodHistory::End(PodEnd::Unproven(why)) => {
                PodHistory::End(PodEnd::Unproven(self.redact(&why)))
            }
            PodHistory::End(PodEnd::Unanswered(why)) => {
                PodHistory::End(PodEnd::Unanswered(self.redact(&why)))
            }
            decided => decided,
        }
    }

    /// Reads the watch at `path`, which asks for bookmarks, event by event until one of `name`
    /// decides [`PodEnd`], a `BOOKMARK` ends the replay, the stream ends, or it passes
    /// [`crate::MAX_API_BODY_BYTES`].
    async fn history(&self, path: &str, name: &str, trace: &mut PodTrace) -> PodHistory {
        // The connection goes with the watch, however the read ends.
        let mut watch = match self.watch(path).await {
            Ok(watch) => watch,
            Err(error) => return PodHistory::End(unopened(error)),
        };
        let mut named = false;
        loop {
            let line = match watch.next_line().await {
                Ok(Some(line)) => line,
                Ok(None) => {
                    return PodHistory::End(PodEnd::Unproven(
                        "the watch ended without a bookmark, so its replay may be incomplete"
                            .to_owned(),
                    ));
                }
                Err(WatchError::Bound) => {
                    return PodHistory::End(PodEnd::Unproven(
                        "the watch passed its byte bound".to_owned(),
                    ));
                }
                Err(_) => {
                    return PodHistory::End(PodEnd::Unanswered(
                        "the watch stream failed".to_owned(),
                    ));
                }
            };
            if let Some(decided) = self.quoted_watch_event(&line, name, trace) {
                return PodHistory::End(decided);
            }
            // `quoted_watch_event` decided a line that is not JSON; what is left is blank or an
            // event.
            let Ok(event) = serde_json::from_slice::<Value>(&line) else {
                continue;
            };
            match event["type"].as_str() {
                Some("BOOKMARK") if named => {
                    return PodHistory::End(PodEnd::Unproven(
                        "the replay showed the Pod added and not its container stopped".to_owned(),
                    ));
                }
                Some("BOOKMARK") => return PodHistory::NeverCreated,
                Some("ADDED" | "MODIFIED" | "DELETED")
                    if event["object"]["metadata"]["name"].as_str() == Some(name) =>
                {
                    named = true;
                }
                _ => {}
            }
        }
    }
}

/// What a watch of a gone Pod that did not open shows: a 410 or a refusal will not tell, anything
/// else may when asked again.
fn unopened(error: WatchError) -> PodEnd {
    match error {
        WatchError::Expired => PodEnd::Unproven(
            "the watch answered 410: the history after the last read is gone".to_owned(),
        ),
        WatchError::Refused(status) => PodEnd::Unproven(format!(
            "the watch answered {status}: the credential may not watch pods"
        )),
        WatchError::Unanswered(error) => PodEnd::Unanswered(error),
        other => PodEnd::Unanswered(other.to_string()),
    }
}
