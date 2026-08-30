//! Delegated context: who authorised this, on whose behalf, through which actor (ADR 0011).
//!
//! Substrate can say which local subject ran a process. It cannot say under whose authority, and
//! atlas objective O1 closes only when it can: "every effectful call in a run's record is
//! attributable to a declared grant; a call outside it is a named refusal, not a missing row".
//!
//! This module is the whole of substrate's side of that seam, and the boundary is deliberate:
//!
//! - **It verifies binding. It never evaluates a grant.** A document naming a grant that admits
//!   everything still meets substrate's own scope, ownership, capability, sandbox, limit and lease
//!   checks unchanged — "a higher-layer permit cannot override" a local check (design 06 § 2).
//!   Nothing here reads authority *out of* `grant_ref`; it is copied to the ledger and never
//!   compared against anything.
//! - **It calls nobody.** Verification is pure computation over the presented bytes and one
//!   configured trusted key. An availability dependency inside a confinement decision would turn an
//!   identity outage into an exec outage, which is why the opaque-token-plus-introspection shape
//!   was rejected (design 09 § 3).
//! - **The binding runs one way.** A document says which subject and deployment it may be presented
//!   under; a mismatch *refuses* rather than re-subjecting the request. Substrate's subject still
//!   comes from kernel peer credentials, and caller-written identity strings elsewhere in the body
//!   stay unread (design 06 § 2).
//! - **Which service signs is configuration.** A `kid` names a trusted key and its issuer origin.
//!   Identity-signed and connectors-signed are the same bytes here, so nothing in this file names
//!   either (ADR 0011).
//!
//! Every failure is a distinct named refusal (design 09 § 5). None degrades to "ran, unattributed":
//! a missing guarantee is a named refusal, never silent (AGENTS.md invariant 3).

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as BASE64URL;
use chrono::{DateTime, Utc};
use ed25519_dalek::{Signature, VerifyingKey};
use serde::Deserialize;
use substrate_wire::{
    DELEGATED_CONTEXT_AUDIENCE, DELEGATED_CONTEXT_CLOCK_SKEW_SECONDS, DELEGATED_CONTEXT_TYPE,
    ErrorClass, MAX_DELEGATED_CONTEXT_BYTES, MAX_DELEGATED_CONTEXT_LIFETIME_SECONDS,
};

/// The bound every claim string is parsed within, so a document cannot carry a payload.
const MAX_CLAIM_BYTES: usize = 512;

/// One configured trusted key: a `kid`, the issuer origin it vouches for, and public material.
///
/// Verifying material only. The daemon holds no signing key and mints no document; a deployment
/// that wants a different signer configures a different key here and changes no code.
#[derive(Debug, Clone)]
pub struct TrustedKey {
    pub kid: String,
    pub issuer: String,
    pub verifying_key: VerifyingKey,
}

/// What this deployment will accept, and whether it insists on one.
#[derive(Debug, Clone, Default)]
pub struct DelegatedContextPolicy {
    keys: Vec<TrustedKey>,
    required: bool,
}

impl DelegatedContextPolicy {
    /// The posture with no configured trust anchor: a context may not be presented at all.
    ///
    /// Presenting one here is `delegated-context.unknown-key`, not silent acceptance — a daemon
    /// that cannot verify must not record an attribution it did not check.
    pub fn none() -> Self {
        Self::default()
    }

    /// # Errors
    ///
    /// Returns an error when two keys share a `kid`, or when a context is required and no key is
    /// configured — a deployment that requires what it can never verify refuses every mutation, and
    /// that is a startup mistake rather than a runtime one.
    pub fn new(keys: Vec<TrustedKey>, required: bool) -> Result<Self, String> {
        for (index, key) in keys.iter().enumerate() {
            if keys[..index].iter().any(|other| other.kid == key.kid) {
                return Err(format!(
                    "delegated-context key {} is declared twice",
                    key.kid
                ));
            }
        }
        if required && keys.is_empty() {
            return Err(
                "a required delegated context needs at least one trusted key to verify it"
                    .to_owned(),
            );
        }
        Ok(Self { keys, required })
    }

    pub fn required(&self) -> bool {
        self.required
    }

    /// Verifies a presented document, or explains in one named refusal why it did not.
    ///
    /// `Ok(None)` is the untouched path: nothing presented where nothing is required, which is
    /// byte-for-byte the operation a `0.6.0` client sends.
    ///
    /// # Errors
    ///
    /// Returns the one named refusal that applies, in the order design 09 § 3 states the checks:
    /// bytes, shape, key, signature, issuer, audience, window, binding.
    pub fn verify(
        &self,
        presented: Option<&str>,
        subject: &str,
        deployment: &str,
        now: DateTime<Utc>,
    ) -> Result<Option<VerifiedContext>, ContextRefusal> {
        let Some(token) = presented else {
            if self.required {
                return Err(ContextRefusal::ABSENT);
            }
            return Ok(None);
        };
        if token.len() > MAX_DELEGATED_CONTEXT_BYTES {
            return Err(ContextRefusal::MALFORMED);
        }
        let (signing_input, signature) = token.rsplit_once('.').ok_or(ContextRefusal::MALFORMED)?;
        let (raw_header, raw_claims) = signing_input
            .split_once('.')
            .ok_or(ContextRefusal::MALFORMED)?;
        if raw_claims.contains('.') {
            return Err(ContextRefusal::MALFORMED);
        }
        let header: Header = decode_segment(raw_header)?;
        if header.alg != "EdDSA" || header.typ != DELEGATED_CONTEXT_TYPE {
            return Err(ContextRefusal::MALFORMED);
        }

        // The key is chosen before the signature is checked, so an unknown `kid` cannot be told
        // apart from a bad signature by timing what substrate did after it.
        let key = self
            .keys
            .iter()
            .find(|candidate| candidate.kid == header.kid)
            .ok_or(ContextRefusal::UNKNOWN_KEY)?;

        let signature = BASE64URL
            .decode(signature)
            .map_err(|_| ContextRefusal::MALFORMED)?;
        let signature: [u8; 64] = signature
            .try_into()
            .map_err(|_| ContextRefusal::MALFORMED)?;
        key.verifying_key
            .verify_strict(signing_input.as_bytes(), &Signature::from_bytes(&signature))
            .map_err(|_| ContextRefusal::SIGNATURE_INVALID)?;

        let claims: Claims = decode_segment(raw_claims)?;
        // The `kid` and the issuer are one trust anchor, not two: a key that does not vouch for the
        // issuer the document names has verified nothing this deployment configured.
        if claims.iss != key.issuer {
            return Err(ContextRefusal::UNKNOWN_KEY);
        }
        if claims.aud != DELEGATED_CONTEXT_AUDIENCE {
            return Err(ContextRefusal::AUDIENCE_MISMATCH);
        }
        // A window that is not a window is a malformed claim, not an expiry: `expired` says the
        // document was well-formed and this instant is outside it.
        if claims.iat > claims.nbf
            || claims.nbf > claims.exp
            || claims.exp.saturating_sub(claims.iat) > MAX_DELEGATED_CONTEXT_LIFETIME_SECONDS
        {
            return Err(ContextRefusal::MALFORMED);
        }
        let instant = now.timestamp();
        if instant + DELEGATED_CONTEXT_CLOCK_SKEW_SECONDS < claims.nbf
            || instant.saturating_sub(DELEGATED_CONTEXT_CLOCK_SKEW_SECONDS) > claims.exp
        {
            return Err(ContextRefusal::EXPIRED);
        }
        if claims.bound_subject != subject || claims.bound_deployment != deployment {
            return Err(ContextRefusal::SUBJECT_MISMATCH);
        }
        Ok(Some(VerifiedContext {
            grant_ref: claims.grant_ref,
            platform_principal: claims.sub,
        }))
    }
}

/// What a verified document contributes to the ledger row, and nothing more.
///
/// The grant-set revision, the tenant, the actor chain and the `jti` are verified as part of the
/// closed claim set and then dropped: design 09 § 8 decision 3 keeps the query surface to the two
/// named columns, and connectors retains the revision on its own decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedContext {
    pub grant_ref: String,
    pub platform_principal: String,
}

/// One named refusal, with the claim that failed and never its value (design 01 § 6.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContextRefusal {
    pub class: ErrorClass,
    pub code: &'static str,
    pub message: &'static str,
    /// The claim that failed. Never carries a value: the document's bytes enter no error body, no
    /// event, no log, no request hash and no resource observation (design 06 § 3).
    pub address: &'static str,
}

impl ContextRefusal {
    pub const ABSENT: Self = Self {
        class: ErrorClass::Refused,
        code: "delegated-context.absent",
        message: "Delegated context is required by this deployment and was not presented.",
        address: "delegated_context",
    };
    pub const MALFORMED: Self = Self {
        class: ErrorClass::Refused,
        code: "delegated-context.malformed",
        message: "Delegated context is not a well-formed delegated-context document.",
        address: "delegated_context",
    };
    pub const UNKNOWN_KEY: Self = Self {
        class: ErrorClass::Refused,
        code: "delegated-context.unknown-key",
        message: "Delegated context names no configured trusted key.",
        address: "kid",
    };
    pub const SIGNATURE_INVALID: Self = Self {
        class: ErrorClass::Refused,
        code: "delegated-context.signature-invalid",
        message: "Delegated context signature does not verify against the named trusted key.",
        address: "signature",
    };
    pub const AUDIENCE_MISMATCH: Self = Self {
        class: ErrorClass::Refused,
        code: "delegated-context.audience-mismatch",
        message: "Delegated context is not addressed to this audience.",
        address: "aud",
    };
    pub const SUBJECT_MISMATCH: Self = Self {
        class: ErrorClass::Refused,
        code: "delegated-context.subject-mismatch",
        message: "Delegated context is bound to a different authenticated subject.",
        address: "bound_subject",
    };
    pub const EXPIRED: Self = Self {
        class: ErrorClass::Refused,
        code: "delegated-context.expired",
        message: "Delegated context is outside its validity window.",
        address: "exp",
    };
    /// The one conflict rather than refusal: the operation exists and ran under another grant.
    ///
    /// `delegated_context` is outside the canonical request hash, so replaying an `op` with a
    /// *fresh* context is the same operation and returns the original outcome. Replaying it under a
    /// *different* grant is not, and first write wins on the recorded one (design 09 § 4).
    pub const GRANT_CONFLICT: Self = Self {
        class: ErrorClass::Conflict,
        code: "delegated-context.grant-conflict",
        message: "Operation id is already bound to a different declared grant.",
        address: "grant_ref",
    };
}

/// The closed JOSE header. Anything else in it is malformed, not ignored.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Header {
    alg: String,
    kid: String,
    typ: String,
}

/// The closed claim set (design 09 § 3).
///
/// `deny_unknown_fields` is the closure check: a document carrying a claim substrate does not know
/// is refused rather than read past, because an unread claim is exactly how an unverified
/// assumption travels.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Claims {
    /// The immediate service actor, in the RFC 8693 shape both siblings already emit.
    ///
    /// Verified as part of the closed set and then dropped: substrate's own `actor` column already
    /// holds the immediate actor it authenticated, and design 09 § 8 decision 3 keeps the ledger's
    /// new query surface to the two named columns.
    #[allow(dead_code)]
    act: Actor,
    aud: String,
    bound_deployment: String,
    bound_subject: String,
    exp: i64,
    grant_ref: String,
    /// Verified as part of the closed set and then dropped: substrate keeps two columns, and the
    /// revision stays on connectors' own decision (design 09 § 8 decision 3).
    #[allow(dead_code)]
    grant_revision: String,
    iat: i64,
    iss: String,
    #[allow(dead_code)]
    jti: String,
    nbf: i64,
    sub: String,
    #[allow(dead_code)]
    tenant: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Actor {
    #[allow(dead_code)]
    sub: String,
}

fn decode_segment<T: serde::de::DeserializeOwned>(segment: &str) -> Result<T, ContextRefusal> {
    let bytes = BASE64URL
        .decode(segment)
        .map_err(|_| ContextRefusal::MALFORMED)?;
    let value: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|_| ContextRefusal::MALFORMED)?;
    if !in_bounds(&value) {
        return Err(ContextRefusal::MALFORMED);
    }
    serde_json::from_value(value).map_err(|_| ContextRefusal::MALFORMED)
}

/// Every string in the decoded document is within bound before any of it is interpreted.
fn in_bounds(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::String(text) => text.len() <= MAX_CLAIM_BYTES,
        serde_json::Value::Object(members) => members.values().all(in_bounds),
        serde_json::Value::Array(items) => items.iter().all(in_bounds),
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::{ContextRefusal, DelegatedContextPolicy, TrustedKey};
    use base64::Engine as _;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD as BASE64URL;
    use chrono::{DateTime, TimeZone as _, Utc};
    use ed25519_dalek::{Signer as _, SigningKey};
    use serde_json::json;
    use sha2::{Digest as _, Sha256};

    const ISSUER: &str = "https://issuer.invalid";
    const KID: &str = "unit-key-1";

    /// A test-only signing key, derived from a literal sentence rather than committed as material.
    ///
    /// Nothing in the repository is a key blob: the seed is the SHA-256 of this English string, so
    /// there is no secret to leak and no fixture to rotate.
    fn signing_key() -> SigningKey {
        let seed: [u8; 32] =
            Sha256::digest(b"substrate unit-test delegated-context signing seed").into();
        SigningKey::from_bytes(&seed)
    }

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 8, 13, 12, 0, 0).unwrap()
    }

    fn policy(required: bool) -> DelegatedContextPolicy {
        DelegatedContextPolicy::new(
            vec![TrustedKey {
                kid: KID.to_owned(),
                issuer: ISSUER.to_owned(),
                verifying_key: signing_key().verifying_key(),
            }],
            required,
        )
        .expect("one key is a policy")
    }

    fn token(header: &serde_json::Value, claims: &serde_json::Value) -> String {
        let signing_input = format!(
            "{}.{}",
            BASE64URL.encode(serde_json::to_vec(header).unwrap()),
            BASE64URL.encode(serde_json::to_vec(claims).unwrap())
        );
        let signature = signing_key().sign(signing_input.as_bytes());
        format!("{signing_input}.{}", BASE64URL.encode(signature.to_bytes()))
    }

    fn header() -> serde_json::Value {
        json!({ "alg": "EdDSA", "kid": KID, "typ": "substrate-delegated-context+jwt" })
    }

    fn claims() -> serde_json::Value {
        let instant = now().timestamp();
        json!({
            "act": { "sub": "svc:actor" },
            "aud": "urn:b10x:substrate",
            "bound_deployment": "dep_unit",
            "bound_subject": "local:1000",
            "exp": instant + 60,
            "grant_ref": "grant:observability-read",
            "grant_revision": "rev_1",
            "iat": instant - 10,
            "iss": ISSUER,
            "jti": "jti_1",
            "nbf": instant - 10,
            "sub": "platform:principal-1",
            "tenant": "tenant_unit",
        })
    }

    /// One claim replaced, everything else the document design 09 section 3 describes.
    fn claims_with(member: &str, value: serde_json::Value) -> serde_json::Value {
        let mut claims = claims();
        claims[member] = value;
        claims
    }

    fn verify(policy: &DelegatedContextPolicy, presented: Option<&str>) -> ContextRefusal {
        policy
            .verify(presented, "local:1000", "dep_unit", now())
            .expect_err("expected a named refusal")
    }

    fn expect_each(cases: Vec<(String, ContextRefusal)>) {
        for (presented, expected) in cases {
            assert_eq!(
                verify(&policy(false), Some(&presented)),
                expected,
                "presented document did not produce {}",
                expected.code
            );
        }
    }

    #[test]
    fn a_verified_context_yields_the_grant_and_the_platform_principal() {
        let verified = policy(false)
            .verify(
                Some(&token(&header(), &claims())),
                "local:1000",
                "dep_unit",
                now(),
            )
            .expect("a well-formed context verifies")
            .expect("a presented context is Some");
        assert_eq!(verified.grant_ref, "grant:observability-read");
        assert_eq!(verified.platform_principal, "platform:principal-1");
    }

    #[test]
    fn omission_is_untouched_unless_the_deployment_requires_one() {
        assert_eq!(
            policy(false)
                .verify(None, "local:1000", "dep_unit", now())
                .expect("omission is not a refusal"),
            None
        );
        assert_eq!(verify(&policy(true), None), ContextRefusal::ABSENT);
    }

    /// Anything substrate cannot read as the closed document is one refusal: `malformed`.
    ///
    /// Deliberately not several. A caller learns that the document was rejected, never which of the
    /// ways it was wrong — the granular codes below are the ones a *relying party* needs to act on.
    #[test]
    fn a_document_substrate_cannot_read_is_malformed() {
        expect_each(vec![
            ("not-a-compact-jws".to_owned(), ContextRefusal::MALFORMED),
            (
                format!("{}.extra", token(&header(), &claims())),
                ContextRefusal::MALFORMED,
            ),
            (
                token(
                    &json!({ "alg": "none", "kid": KID, "typ": "substrate-delegated-context+jwt" }),
                    &claims(),
                ),
                ContextRefusal::MALFORMED,
            ),
            (
                token(
                    &json!({ "alg": "EdDSA", "kid": KID, "typ": "JWT" }),
                    &claims(),
                ),
                ContextRefusal::MALFORMED,
            ),
            // A claim substrate does not know is refused rather than read past: an unread claim is
            // how an unverified assumption travels.
            (
                token(
                    &header(),
                    &claims_with("extra", json!("a claim substrate does not know")),
                ),
                ContextRefusal::MALFORMED,
            ),
            (
                {
                    let mut claims = claims();
                    claims.as_object_mut().unwrap().remove("grant_ref");
                    token(&header(), &claims)
                },
                ContextRefusal::MALFORMED,
            ),
            // A lifetime beyond the bound is a malformed claim, not an expiry: the window is not a
            // window this deployment accepts at all.
            (
                token(
                    &header(),
                    &claims_with("exp", json!(now().timestamp() + 3600)),
                ),
                ContextRefusal::MALFORMED,
            ),
        ]);
    }

    /// A readable document substrate must not trust gets its own code, one per reason.
    #[test]
    fn a_readable_document_it_must_not_trust_is_named_separately() {
        let instant = now().timestamp();
        let mut stale = claims();
        stale["iat"] = json!(instant - 600);
        stale["nbf"] = json!(instant - 600);
        stale["exp"] = json!(instant - 400);
        expect_each(vec![
            (
                token(
                    &json!({ "alg": "EdDSA", "kid": "unconfigured", "typ": "substrate-delegated-context+jwt" }),
                    &claims(),
                ),
                ContextRefusal::UNKNOWN_KEY,
            ),
            // The `kid` and the issuer are one trust anchor: a key that does not vouch for the
            // issuer the document names has verified nothing this deployment configured.
            (
                token(
                    &header(),
                    &claims_with("iss", json!("https://elsewhere.invalid")),
                ),
                ContextRefusal::UNKNOWN_KEY,
            ),
            (
                token(&header(), &claims_with("aud", json!("urn:b10x:connectors"))),
                ContextRefusal::AUDIENCE_MISMATCH,
            ),
            (
                token(
                    &header(),
                    &claims_with("bound_subject", json!("local:4242")),
                ),
                ContextRefusal::SUBJECT_MISMATCH,
            ),
            (
                token(
                    &header(),
                    &claims_with("bound_deployment", json!("dep_elsewhere")),
                ),
                ContextRefusal::SUBJECT_MISMATCH,
            ),
            (token(&header(), &stale), ContextRefusal::EXPIRED),
            (
                {
                    let valid = token(&header(), &claims());
                    let (rest, signature) = valid.rsplit_once('.').unwrap();
                    let flipped = if signature.ends_with('A') { 'B' } else { 'A' };
                    format!("{rest}.{}{flipped}", &signature[..signature.len() - 1])
                },
                ContextRefusal::SIGNATURE_INVALID,
            ),
        ]);
    }

    #[test]
    fn a_deployment_with_no_trust_anchor_verifies_nothing() {
        assert_eq!(
            verify(
                &DelegatedContextPolicy::none(),
                Some(&token(&header(), &claims()))
            ),
            ContextRefusal::UNKNOWN_KEY
        );
        assert!(DelegatedContextPolicy::new(Vec::new(), true).is_err());
    }
}
