//! Guarded workspace file access, the driver error type and the exec environment rule, shared by
//! every Substrate driver and by a binary that must not link the host driver (design 34 § 2).
//!
//! `b10x-substrate-host` re-exports [`DriverError`], [`DriverErrorClass`] and [`Listed`] at its
//! crate root, so every existing path to them compiles unchanged.

#![allow(unsafe_code)]
#![deny(unsafe_op_in_unsafe_fn)]
#![allow(
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    reason = "moved whole from a private module of substrate-host (design 34 § 2); every error is a \
              DriverError whose code is a named refusal of the wire contract, and every panic is a query \
              shape the caller has validated or an integer width the platform guarantees"
)]

pub mod environment;
#[cfg(target_os = "linux")]
pub mod fs;

use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriverErrorClass {
    Refused,
    Conflict,
    Unserved,
    Exhausted,
    Failed,
    NotFound,
}

#[derive(Debug, Error)]
#[error("{code}: {message}")]
pub struct DriverError {
    pub class: DriverErrorClass,
    pub code: &'static str,
    pub message: String,
    pub address: Option<String>,
    pub retriable: bool,
}

impl DriverError {
    pub fn refused(code: &'static str, message: impl Into<String>, address: &str) -> Self {
        Self {
            class: DriverErrorClass::Refused,
            code,
            message: message.into(),
            address: Some(address.to_owned()),
            retriable: false,
        }
    }

    pub fn not_found() -> Self {
        Self {
            class: DriverErrorClass::NotFound,
            code: "resource.not-found",
            message: "Resource was not found.".to_owned(),
            address: Some("resource".to_owned()),
            retriable: false,
        }
    }

    pub fn unserved(code: &'static str, message: impl Into<String>, address: &str) -> Self {
        Self {
            class: DriverErrorClass::Unserved,
            code,
            message: message.into(),
            address: Some(address.to_owned()),
            retriable: false,
        }
    }

    pub fn exhausted(code: &'static str, message: impl Into<String>, address: &str) -> Self {
        Self {
            class: DriverErrorClass::Exhausted,
            code,
            message: message.into(),
            address: Some(address.to_owned()),
            retriable: true,
        }
    }

    pub fn failed(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            class: DriverErrorClass::Failed,
            code,
            message: message.into(),
            address: None,
            retriable: true,
        }
    }
}

/// A listing and the number of entries it omitted because the wire cannot name them (ADR 0043).
///
/// The count travels beside the wire object, not in it, because the directory page and the tree
/// are closed objects; the daemon answers it as `substrate_wire::OMITTED_ENTRIES_HEADER`. A
/// result that lists nothing, such as a file read, omits nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listed<T> {
    pub value: T,
    pub omitted_entries: u64,
}

impl<T> Listed<T> {
    /// A result that omitted no entry.
    pub fn complete(value: T) -> Self {
        Self {
            value,
            omitted_entries: 0,
        }
    }
}
