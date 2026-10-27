//! The `macos` adapter: Seatbelt (ADR 0037, ADR 0109 S1). A stub in this tree: `open` answers
//! `confinement.adapter-unserved` on every platform until `story:macos-adapter-on-seatbelt`
//! replaces this file.

use std::sync::Arc;

use super::{AdapterName, ConfinementAdapter, ConfinementError};

/// What opening `macos` takes. The adapter story adds what it needs here.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct MacosRequest {}

pub(crate) fn open(
    _request: MacosRequest,
) -> Result<Arc<dyn ConfinementAdapter>, ConfinementError> {
    super::unserved(
        AdapterName::Macos,
        "the macos adapter is not served by this build",
    )
}
