use std::fmt::Write as _;

use axum::Json;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse as _, Response};
use serde::Serialize;
use substrate_wire::{ErrorClass, ErrorDetail, Failure, Success, WireValidationError};

use crate::delegation::ContextRefusal;

use super::App;

pub(super) fn request_id(app: &App, headers: &HeaderMap) -> String {
    headers
        .get("x-request-id")
        .and_then(|value| value.to_str().ok())
        .filter(|value| {
            (8..=128).contains(&value.len())
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        })
        .map_or_else(|| app.authority.request_id(), ToOwned::to_owned)
}

pub(super) fn query_is_empty(raw_query: Option<&str>) -> bool {
    raw_query.is_none_or(str::is_empty)
}

pub(super) fn normalized_file_address(workspace_id: &str, path: &str) -> String {
    let mut address = format!("/v1/workspaces/{workspace_id}/files/");
    for byte in path.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~' | b'/') {
            address.push(char::from(byte));
        } else {
            let _ = write!(address, "%{byte:02X}");
        }
    }
    address
}

pub(super) fn path_refusal(
    request_id: &str,
    operation: Option<&str>,
    error: &WireValidationError,
) -> Response {
    let (code, message) = if matches!(error, WireValidationError::InvalidPathDepth) {
        (
            "workspace.path-depth",
            "Workspace path exceeds the configured component limit.",
        )
    } else {
        (
            "workspace.path-escape",
            "Workspace path is outside the confined root.",
        )
    };
    failure(
        StatusCode::UNPROCESSABLE_ENTITY,
        request_id,
        operation,
        ErrorClass::Refused,
        code,
        message,
        Some("path"),
        false,
    )
}

pub(super) fn schema_invalid(request_id: &str, operation: Option<&str>, address: &str) -> Response {
    failure(
        StatusCode::UNPROCESSABLE_ENTITY,
        request_id,
        operation,
        ErrorClass::Refused,
        "request.schema-invalid",
        "Request does not match the closed operation schema.",
        Some(address),
        false,
    )
}

pub(super) fn operation_ledger_capacity(request_id: &str) -> Response {
    failure(
        StatusCode::INSUFFICIENT_STORAGE,
        request_id,
        None,
        ErrorClass::Exhausted,
        "operation.ledger-capacity",
        "Operation ledger capacity is exhausted.",
        Some("operation-ledger"),
        false,
    )
}

pub(super) fn outcome_unknown(request_id: &str, operation: &str) -> Response {
    failure(
        StatusCode::INTERNAL_SERVER_ERROR,
        request_id,
        Some(operation),
        ErrorClass::Failed,
        "operation.outcome-unknown",
        "Operation was accepted but its terminal outcome is unknown.",
        Some("operation"),
        true,
    )
}

pub(super) fn conflict(
    request_id: &str,
    operation: &str,
    code: &str,
    message: &str,
    address: &str,
) -> Response {
    failure(
        StatusCode::CONFLICT,
        request_id,
        Some(operation),
        ErrorClass::Conflict,
        code,
        message,
        Some(address),
        false,
    )
}

/// One named delegated-context refusal, answered before dispatch (ADR 0011, design 09 section 5).
///
/// The `address` is the claim that failed and never its value, and the presented document's bytes
/// reach no error body, no event and no log (design 06 section 3): only the constant strings on
/// [`ContextRefusal`] are serialized here.
pub(super) fn delegated_context_refusal(
    request_id: &str,
    operation: Option<&str>,
    refusal: ContextRefusal,
) -> Response {
    let status = if matches!(refusal.class, ErrorClass::Conflict) {
        StatusCode::CONFLICT
    } else {
        StatusCode::UNPROCESSABLE_ENTITY
    };
    failure(
        status,
        request_id,
        operation,
        refusal.class,
        refusal.code,
        refusal.message,
        Some(refusal.address),
        false,
    )
}

pub(super) fn workspace_frozen_refusal(request_id: &str, operation: &str) -> Response {
    conflict(
        request_id,
        operation,
        "workspace.not-ready",
        "Workspace is not ready for this operation.",
        "workspace",
    )
}

pub(super) fn workspace_missing_refusal(request_id: &str, operation: &str) -> Response {
    not_found_with_operation(request_id, operation)
}

pub(super) fn not_found(request_id: &str) -> Response {
    not_found_at(request_id, "resource")
}

pub(super) fn not_found_at(request_id: &str, address: &str) -> Response {
    failure(
        StatusCode::NOT_FOUND,
        request_id,
        None,
        ErrorClass::Refused,
        "resource.not-found",
        "Resource was not found.",
        Some(address),
        false,
    )
}

pub(super) fn not_found_with_operation(request_id: &str, operation: &str) -> Response {
    failure(
        StatusCode::NOT_FOUND,
        request_id,
        Some(operation),
        ErrorClass::Refused,
        "resource.not-found",
        "Resource was not found.",
        Some("resource"),
        false,
    )
}

pub(super) fn store_failure(
    request_id: &str,
    operation: Option<&str>,
    error: &substrate_store::StoreError,
) -> Response {
    tracing::error!(%error, "state store failure");
    failure(
        StatusCode::INTERNAL_SERVER_ERROR,
        request_id,
        operation,
        ErrorClass::Failed,
        "state.store-failed",
        "Durable state operation failed.",
        Some("state"),
        true,
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) fn failure(
    status: StatusCode,
    request_id: &str,
    operation: Option<&str>,
    class: ErrorClass,
    code: &str,
    message: &str,
    address: Option<&str>,
    retriable: bool,
) -> Response {
    failure_detail(
        status,
        request_id,
        ErrorDetail {
            class,
            code: code.to_owned(),
            message: message.to_owned(),
            retriable,
            address: address.map(ToOwned::to_owned),
            operation: operation.map(ToOwned::to_owned),
        },
    )
}

pub(super) fn failure_detail(
    status: StatusCode,
    request_id: &str,
    detail: ErrorDetail,
) -> Response {
    let mut response = (status, Json(Failure::new(request_id, detail.clone()))).into_response();
    response.extensions_mut().insert(detail);
    response
}

pub(super) fn success<T: Serialize>(status: StatusCode, body: Success<T>) -> Response {
    (status, Json(body)).into_response()
}

/// The answer to a mutation whose row the store has just ended with `result`: the success at
/// `status` when the store records that result, else `operation.result-unrecorded` at 500, the
/// error the store wrote into the row instead (ADR 0053 decision 8). The store and this answer
/// ask the one rule, [`substrate_store::Store::records_success_result`], so the first answer and
/// every replay of the operation id agree.
pub(super) fn completed<T: Serialize>(
    store: &substrate_store::Store,
    status: StatusCode,
    request_id: &str,
    operation: &str,
    result: T,
) -> Response {
    if store.records_success_result(&result) {
        success(status, Success::mutation(request_id, operation, result))
    } else {
        failure_detail(
            StatusCode::INTERNAL_SERVER_ERROR,
            request_id,
            substrate_store::result_unrecorded(operation),
        )
    }
}

/// The message of `operation.result-limit` (ADR 0053 decision 2).
pub(super) const RESULT_LIMIT_MESSAGE: &str =
    "The result of this operation exceeds what the operation ledger can record.";

/// `operation.result-limit`: class `exhausted`, 429, not retriable, address `result` (ADR 0053
/// decision 2). The width a start's or file change's result can reach is over the bytes the
/// store records for a success result, so the mutation is refused before dispatch.
pub(super) fn result_limit(operation: &str) -> ErrorDetail {
    ErrorDetail {
        class: ErrorClass::Exhausted,
        code: substrate_wire::OPERATION_RESULT_LIMIT.to_owned(),
        message: RESULT_LIMIT_MESSAGE.to_owned(),
        retriable: false,
        address: Some("result".to_owned()),
        operation: Some(operation.to_owned()),
    }
}

/// ADR 0053 decision 1: a mutation whose result can be `widest` bytes of compact JSON is refused
/// [`result_limit`] at 429 when that is over what `store` records for a success result. The
/// caller records the refusal before dispatch.
pub(super) fn within_result_reserve(
    store: &substrate_store::Store,
    request_id: &str,
    operation: &str,
    widest: usize,
) -> Result<(), Response> {
    if u64::try_from(widest).is_ok_and(|widest| widest <= store.success_result_json_bytes()) {
        Ok(())
    } else {
        Err(failure_detail(
            StatusCode::TOO_MANY_REQUESTS,
            request_id,
            result_limit(operation),
        ))
    }
}
