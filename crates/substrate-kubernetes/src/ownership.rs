//! Whether an object the API holds is ours (design 34 § 4, ADR 0047 decision 4).
//!
//! An object is ours when its name has a ledger row, its deployment label is ours, and either its
//! uid equals the row's bound uid, or no uid is bound and its operation annotation equals the
//! row's: the second case adopts the uid of a create whose answer was lost. Anything else is a
//! collision: logged, counted, never modified, and the row stays as it was.

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};

use serde_json::Value;
use substrate_host::{BackendRecord, BackendRole};

use crate::names::{DEPLOYMENT_LABEL, OPERATION_ANNOTATION, operation_annotation};

/// Distinct foreign objects [`Collisions`] remembers so that one object met again is not counted
/// again. Past it, a foreign object not yet remembered is counted every time it is met: the
/// series then overcounts, the safe direction for an operator's alarm, and never misses an object.
pub(crate) const COLLISIONS_REMEMBERED: usize = 4096;

/// One object met under a name the driver would use and did not create (§ 4), told apart from
/// every other by its kind, its name and its uid (`None` when it carries none).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Foreign {
    kind: &'static str,
    name: String,
    uid: Option<String>,
}

impl Foreign {
    /// The object `object` in the collection of `role`'s objects.
    pub(crate) fn of(role: BackendRole, object: &Value) -> Self {
        let metadata = &object["metadata"];
        Self {
            kind: match role {
                BackendRole::Pvc => "PersistentVolumeClaim",
                BackendRole::Keeper | BackendRole::Pod => "Pod",
            },
            name: metadata["name"].as_str().unwrap_or_default().to_owned(),
            uid: metadata["uid"]
                .as_str()
                .filter(|uid| !uid.is_empty())
                .map(str::to_owned),
        }
    }
}

/// `substrate_kubernetes_collisions_total`: the foreign objects met since this process started,
/// each counted once however often it is met again (§ 4).
#[derive(Debug, Default)]
pub(crate) struct Collisions {
    total: AtomicU64,
    met: Mutex<BTreeSet<Foreign>>,
}

impl Collisions {
    /// Counts `object` unless it is already remembered, remembering at most `remembered`
    /// objects. An object the driver could not read (`None`) is counted every time. Returns
    /// whether it was counted.
    pub(crate) fn meet(&self, object: Option<Foreign>, remembered: usize) -> bool {
        let first = object.is_none_or(|object| {
            let mut met = self.met.lock().unwrap_or_else(PoisonError::into_inner);
            if met.contains(&object) {
                return false;
            }
            if met.len() < remembered {
                met.insert(object);
            }
            true
        });
        if first {
            self.total.fetch_add(1, Ordering::Relaxed);
        }
        first
    }

    /// The objects counted.
    pub(crate) fn total(&self) -> u64 {
        self.total.load(Ordering::Relaxed)
    }
}

/// What [`ownership`] decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ownership {
    /// Ours, with this uid. `adopt` is set when the row has no uid bound yet and the annotation
    /// proved the object: the caller binds the uid before it acts on the object.
    Ours { uid: String, adopt: bool },
    /// Not ours, for this reason: never modified or deleted.
    Collision(String),
}

/// Decides whether `object` is the one `row` records, for the deployment whose label value is
/// `deployment_label`. `row` is the ledger's row for the object's name, if any.
#[must_use]
pub fn ownership(object: &Value, row: Option<&BackendRecord>, deployment_label: &str) -> Ownership {
    let metadata = &object["metadata"];
    let name = metadata["name"].as_str().unwrap_or_default();
    let Some(row) = row.filter(|row| row.name == name) else {
        return Ownership::Collision(format!("{name:?} has no ledger row"));
    };
    if row.settled_at.is_some() {
        return Ownership::Collision(format!(
            "{name:?} has a settled row: this is another object"
        ));
    }
    if metadata["labels"][DEPLOYMENT_LABEL] != deployment_label {
        return Ownership::Collision(format!("{name:?} is labelled for another deployment"));
    }
    let Some(uid) = metadata["uid"].as_str().filter(|uid| !uid.is_empty()) else {
        return Ownership::Collision(format!("{name:?} carries no uid"));
    };
    match &row.uid {
        Some(bound) if bound == uid => Ownership::Ours {
            uid: uid.to_owned(),
            adopt: false,
        },
        Some(_) => Ownership::Collision(format!("{name:?} has another uid than the bound one")),
        None if metadata["annotations"][OPERATION_ANNOTATION]
            == operation_annotation(&row.operation_digest).as_str() =>
        {
            Ownership::Ours {
                uid: uid.to_owned(),
                adopt: true,
            }
        }
        None => Ownership::Collision(format!(
            "{name:?} carries another operation annotation than its row"
        )),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use substrate_host::BackendRole;

    use super::{COLLISIONS_REMEMBERED, Collisions, Foreign};

    fn object(name: &str, uid: Option<&str>) -> serde_json::Value {
        json!({"metadata": {"name": name, "uid": uid}})
    }

    /// `story:kubernetes-collisions-metric`: the series counts objects. Two foreign objects are
    /// two, whether they differ by name, by uid under one name, or by kind under one name; the
    /// same object met again is still one.
    #[test]
    fn two_different_foreign_objects_count_two_and_one_met_again_counts_once() {
        let pairs = [
            (
                Foreign::of(BackendRole::Pvc, &object("sbx-ws-a", Some("u1"))),
                Foreign::of(BackendRole::Pvc, &object("sbx-ws-b", Some("u2"))),
            ),
            (
                Foreign::of(BackendRole::Pod, &object("sbx-ex-a", Some("u1"))),
                Foreign::of(BackendRole::Pod, &object("sbx-ex-a", Some("u2"))),
            ),
            (
                Foreign::of(BackendRole::Pvc, &object("sbx-x", Some("u1"))),
                Foreign::of(BackendRole::Keeper, &object("sbx-x", Some("u1"))),
            ),
            (
                Foreign::of(BackendRole::Pod, &object("sbx-ex-a", None)),
                Foreign::of(BackendRole::Pod, &object("sbx-ex-a", Some("u1"))),
            ),
        ];
        for (first, second) in pairs {
            let collisions = Collisions::default();
            assert!(collisions.meet(Some(first.clone()), COLLISIONS_REMEMBERED));
            assert!(collisions.meet(Some(second.clone()), COLLISIONS_REMEMBERED));
            assert!(!collisions.meet(Some(first.clone()), COLLISIONS_REMEMBERED));
            assert!(!collisions.meet(Some(second.clone()), COLLISIONS_REMEMBERED));
            assert_eq!(collisions.total(), 2, "{first:?} and {second:?}");
        }
    }

    /// At the bound a new foreign object is still counted, every time it is met (an overcount),
    /// while the remembered ones stay counted once; an object that could not be read is counted
    /// every time.
    #[test]
    fn past_the_bound_a_new_object_is_still_counted() {
        let collisions = Collisions::default();
        let remembered = Foreign::of(BackendRole::Pod, &object("a", Some("u1")));
        let forgotten = Foreign::of(BackendRole::Pod, &object("b", Some("u2")));
        assert!(collisions.meet(Some(remembered.clone()), 1));
        assert!(collisions.meet(Some(forgotten.clone()), 1));
        assert!(collisions.meet(Some(forgotten), 1));
        assert!(!collisions.meet(Some(remembered), 1));
        assert!(collisions.meet(None, 1));
        assert!(collisions.meet(None, 1));
        assert_eq!(collisions.total(), 5);
    }
}
