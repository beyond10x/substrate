//! Whether an object the API holds is ours (design 34 § 4, ADR 0047 decision 4).
//!
//! An object is ours when its name has a ledger row, its deployment label is ours, and either its
//! uid equals the row's bound uid, or no uid is bound and its operation annotation equals the
//! row's: the second case adopts the uid of a create whose answer was lost. Anything else is a
//! collision: logged, counted, never modified, and the row stays as it was.

use serde_json::Value;
use substrate_host::BackendRecord;

use crate::names::{DEPLOYMENT_LABEL, OPERATION_ANNOTATION, operation_annotation};

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
