//! The run spec an exec Pod's container command carries (design 34 § 7.3, § 13 D4).
//!
//! The spec is a closed JSON object, base64url-encoded without padding and split into parts of at
//! most [`MAX_PART_BYTES`], because Linux refuses one argument over `MAX_ARG_STRLEN` (128 KiB).
//! base64url has no `$`, so kubelet's `$(VAR)` expansion of a container command cannot touch it.

use std::collections::BTreeMap;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Serialize};

/// The longest argument part.
pub const MAX_PART_BYTES: usize = 96 * 1024;

/// What the exec Pod's PID 1 runs. `env` is the final environment the driver computed with the
/// host rule; the child receives exactly it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunSpec {
    pub argv: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub memory_bytes: u64,
    pub cpu_millicores: u64,
    pub output_bytes: u64,
}

/// Why a spec did not decode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpecError(pub &'static str);

impl std::fmt::Display for SpecError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "run spec: {}", self.0)
    }
}

impl std::error::Error for SpecError {}

impl RunSpec {
    /// The spec as container-command argument parts.
    ///
    /// # Panics
    ///
    /// Never: a spec of strings and integers always serializes, and base64url is ASCII.
    pub fn encode_parts(&self) -> Vec<String> {
        let json = serde_json::to_vec(self).expect("a run spec serializes");
        let encoded = URL_SAFE_NO_PAD.encode(json);
        // base64url is ASCII, so every byte boundary is a character boundary.
        encoded
            .as_bytes()
            .chunks(MAX_PART_BYTES)
            .map(|part| String::from_utf8(part.to_vec()).expect("base64url is ASCII"))
            .collect()
    }

    /// Decodes and validates the parts of one spec.
    ///
    /// # Errors
    ///
    /// No part, a part over [`MAX_PART_BYTES`] or outside base64url, a JSON object that is not
    /// the closed spec, an empty argv, a NUL in any argv element, environment name or value, or
    /// an environment name that is empty or holds `=`.
    pub fn decode_parts<S: AsRef<str>>(parts: &[S]) -> Result<Self, SpecError> {
        if parts.is_empty() {
            return Err(SpecError("no part"));
        }
        let mut encoded = String::new();
        for part in parts {
            let part = part.as_ref();
            if part.is_empty() || part.len() > MAX_PART_BYTES {
                return Err(SpecError("a part outside the part bound"));
            }
            encoded.push_str(part);
        }
        let json = URL_SAFE_NO_PAD
            .decode(encoded)
            .map_err(|_| SpecError("parts that are not unpadded base64url"))?;
        let spec: Self =
            serde_json::from_slice(&json).map_err(|_| SpecError("not the closed spec object"))?;
        spec.validate()?;
        Ok(spec)
    }

    fn validate(&self) -> Result<(), SpecError> {
        if self.argv.is_empty() || self.argv[0].is_empty() {
            return Err(SpecError("an empty argv"));
        }
        if self.argv.iter().any(|part| part.contains('\0')) {
            return Err(SpecError("a NUL in the argv"));
        }
        for (name, value) in &self.env {
            if name.is_empty() || name.contains(['=', '\0']) || value.contains('\0') {
                return Err(SpecError("an environment entry no process can carry"));
            }
        }
        Ok(())
    }
}
