//! Admission of a start's workspaces: its own, and each writable Git root it names (ADR 0038
//! kind 2).
//!
//! A Git root is admitted exactly as the start's own workspace is: looked up in the caller's
//! scope (same deployment and subject), Ready, under an active lease, and locked for the start.
//! Every refusal happens before dispatch, so the caller records it as a durable ledger row a retry
//! replays. What reaches the driver is the root name the store holds, never a path from the
//! request.

use axum::http::StatusCode;
use axum::response::Response;
use substrate_host::GitRootBinding;
use substrate_store::{Scope, WorkspaceAdmission};
use substrate_wire::{ErrorClass, ExecStartInput, WritableRoot};

use super::App;
use super::operations::driver_failure;
use super::responses::{
    conflict, failure, not_found_with_operation, store_failure, workspace_frozen_refusal,
};

/// Why a start's workspaces were not admitted.
pub(super) enum StartRefusal {
    /// A refusal the caller records in the operation ledger before returning it.
    Recorded(Response),
    /// A store failure, returned as is, as for the start's own workspace.
    Unrecorded(Response),
}

/// Every workspace a start involves, in lock order's input: its own, then each Git root.
pub(super) fn start_workspaces(input: &ExecStartInput) -> Vec<&str> {
    std::iter::once(input.workspace.as_str())
        .chain(input.writable_roots.iter().filter_map(|root| match root {
            WritableRoot::Git(root) => Some(root.workspace.as_str()),
            WritableRoot::Host(_) => None,
        }))
        .collect()
}

/// Admits the start's own workspace, then each writable Git root in request order. The caller
/// holds [`App::lock_workspaces`] over [`start_workspaces`] across this and the dispatch.
pub(super) async fn admit_start_workspaces(
    app: &App,
    scope: &Scope,
    request_id: &str,
    operation: &str,
    input: &ExecStartInput,
) -> Result<(String, Vec<GitRootBinding>), StartRefusal> {
    let root_name = match app.admit_workspace(scope, &input.workspace).await {
        Ok(WorkspaceAdmission::Missing) => {
            return Err(StartRefusal::Recorded(not_found_with_operation(
                request_id, operation,
            )));
        }
        Ok(WorkspaceAdmission::Frozen { .. }) => {
            return Err(StartRefusal::Recorded(workspace_frozen_refusal(
                request_id, operation,
            )));
        }
        Ok(WorkspaceAdmission::Admitted { root_name, .. }) => root_name,
        Err(error) => {
            return Err(StartRefusal::Unrecorded(store_failure(
                request_id,
                Some(operation),
                &error,
            )));
        }
    };
    let mut bindings = Vec::new();
    for root in &input.writable_roots {
        let WritableRoot::Git(root) = root else {
            continue;
        };
        let refuse = |response| Err(StartRefusal::Recorded(response));
        match app.admit_workspace(scope, &root.workspace).await {
            // Another subject's workspace is missing in this scope: the answer is identical to
            // one that never existed, so a Git root is no existence oracle.
            Ok(WorkspaceAdmission::Missing) => {
                return refuse(failure(
                    StatusCode::NOT_FOUND,
                    request_id,
                    Some(operation),
                    ErrorClass::Refused,
                    "resource.not-found",
                    "Resource was not found.",
                    Some("writable_roots"),
                    false,
                ));
            }
            Ok(WorkspaceAdmission::Frozen { .. }) => {
                return refuse(conflict(
                    request_id,
                    operation,
                    "workspace.not-ready",
                    "Workspace is not ready for this operation.",
                    "writable_roots",
                ));
            }
            Ok(WorkspaceAdmission::Admitted {
                root_name: git_root_name,
                resource,
            }) => {
                // An adopted directory is the operator's, whatever it holds: the daemon's record
                // decides first, the driver's record of a Git materialization second.
                if let Err(error) = app.driver.admit_git_root(
                    &root.workspace,
                    &git_root_name,
                    resource.origin.is_adopted(),
                ) {
                    return refuse(driver_failure(request_id, Some(operation), &error));
                }
                bindings.push(GitRootBinding {
                    workspace: root.workspace.clone(),
                    root_name: git_root_name,
                });
            }
            Err(error) => {
                return Err(StartRefusal::Unrecorded(store_failure(
                    request_id,
                    Some(operation),
                    &error,
                )));
            }
        }
    }
    Ok((root_name, bindings))
}
