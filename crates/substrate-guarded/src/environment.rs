//! The closed, non-secret shape every run's environment is held to, whatever adapter runs it.
//!
//! The host driver checks it on every exec start, and the confinement adapters that do not go
//! through the driver (`local`) check the same rule, so a credential-named variable reaches no
//! process: an unconfined one least of all.

use std::collections::BTreeMap;

use substrate_wire::BaselineEnvironment;

use crate::DriverError;

/// Most variables one start may set.
pub const MAX_SET: usize = 64;

/// Refuses `exec.environment-refused` unless every variable in `set` has an upper-case ASCII name
/// of at most 128 bytes that names no credential, and a value of at most 4096 bytes without NUL,
/// and there are at most [`MAX_SET`] of them.
pub fn check(set: &BTreeMap<String, String>) -> Result<(), DriverError> {
    if set.len() > MAX_SET {
        return Err(refused());
    }
    for (name, value) in set {
        if name.is_empty()
            || name.len() > 128
            || !name.bytes().enumerate().all(|(index, byte)| {
                if index == 0 {
                    byte.is_ascii_uppercase()
                } else {
                    byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_'
                }
            })
            || value.len() > 4096
            || value.contains('\0')
            || is_secretish_name(name)
        {
            return Err(refused());
        }
    }
    Ok(())
}

/// Whether `name` names a credential by one of the fragments a secret's name carries.
pub fn is_secretish_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    [
        "authorization",
        "bearer",
        "credential",
        "password",
        "proxy",
        "secret",
        "token",
    ]
    .iter()
    .any(|fragment| lower.contains(fragment))
}

/// The value a baseline variable carries in every run that admits it.
pub const fn baseline_value(name: BaselineEnvironment) -> &'static str {
    match name {
        BaselineEnvironment::Lang | BaselineEnvironment::LcAll => "C.UTF-8",
        BaselineEnvironment::Path => "/usr/bin:/bin",
        BaselineEnvironment::Term => "dumb",
        BaselineEnvironment::Tz => "UTC",
    }
}

pub fn refused() -> DriverError {
    DriverError::refused(
        "exec.environment-refused",
        "Exec environment is outside the closed non-secret shape.",
        "env",
    )
}
